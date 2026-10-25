use std::collections::VecDeque;
use std::sync::Arc;

use anyhow::ensure;
use tokio::sync::Notify;

use super::{Delivery, EventBatch, Payload};

struct BufferedEvent {
    arrival: u64,
    wire_bytes: usize,
    batch: EventBatch,
}

pub struct EventQueue {
    queue: VecDeque<BufferedEvent>,
    budget: usize,
    pub bytes: usize,
    pub dropped: u64,
    dropped_arrival: u64,
    next_arrival: u64,
    next_sequence: u64,
    pending: Option<(u64, Delivery)>,
    pub changed: Arc<Notify>,
}

impl EventQueue {
    pub fn new(budget: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            budget,
            bytes: 0,
            dropped: 0,
            dropped_arrival: 0,
            next_arrival: 0,
            next_sequence: 0,
            pending: None,
            changed: Arc::new(Notify::new()),
        }
    }

    pub fn push(&mut self, batch: EventBatch, wire_bytes: usize) {
        self.next_arrival += 1;
        self.bytes += wire_bytes;
        self.queue.push_back(BufferedEvent {
            arrival: self.next_arrival,
            wire_bytes,
            batch,
        });

        while self.bytes > self.budget {
            let front = self
                .queue
                .pop_front()
                .expect("over budget implies queued item");

            if self.dropped == 0 {
                self.dropped_arrival = front.arrival;
            }

            self.bytes -= front.wire_bytes;
            self.dropped = self.dropped.saturating_add(1);
        }

        self.changed.notify_one();
    }

    pub fn boundary(&self) -> u64 {
        self.next_arrival
    }

    pub fn delivery(&mut self, boundary: Option<u64>) -> Option<Delivery> {
        if let Some((arrival, delivery)) = &self.pending {
            return if boundary.is_none_or(|end| *arrival <= end) {
                Some(delivery.clone())
            } else {
                None
            };
        }

        let (arrival, payload) =
            if self.dropped != 0 && boundary.is_none_or(|end| self.dropped_arrival <= end) {
                let packets = std::mem::take(&mut self.dropped);

                (
                    boundary.unwrap_or(self.next_arrival),
                    Payload::Dropped { packets },
                )
            } else {
                let event = self.queue.front()?;

                if boundary.is_some_and(|end| event.arrival > end) {
                    return None;
                }

                let event = self.queue.pop_front()?;

                self.bytes -= event.wire_bytes;

                (event.arrival, Payload::Events { batch: event.batch })
            };

        self.next_sequence += 1;

        let delivery = Delivery {
            sequence: self.next_sequence,
            payload,
        };

        self.pending = Some((arrival, delivery.clone()));

        Some(delivery)
    }

    pub fn ack(&mut self, sequence: u64) -> anyhow::Result<()> {
        ensure!(
            self.pending
                .as_ref()
                .is_some_and(|(_, d)| d.sequence == sequence),
            "ACK does not match in-flight delivery"
        );
        self.pending = None;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::SuspendPolicy;

    fn batch() -> EventBatch {
        EventBatch {
            received_at_ms: 0,
            suspend_policy: SuspendPolicy::None,
            events: vec![],
        }
    }

    #[test]
    fn retention_eviction_and_dropped_snapshot() {
        let mut queue = EventQueue::new(10);
        queue.push(batch(), 8);
        let first = queue.delivery(None).unwrap();
        queue.push(batch(), 8);
        queue.push(batch(), 8);
        assert_eq!(queue.delivery(None).unwrap().sequence, first.sequence);
        assert!(queue.ack(first.sequence + 1).is_err());
        queue.ack(first.sequence).unwrap();
        let dropped = queue.delivery(None).unwrap();
        assert!(matches!(dropped.payload, Payload::Dropped { packets: 1 }));
        queue.push(batch(), 8);
        queue.ack(dropped.sequence).unwrap();
        assert!(matches!(
            queue.delivery(None).unwrap().payload,
            Payload::Dropped { packets: 1 }
        ));
        assert!(queue.bytes <= 10);
    }

    #[test]
    fn drain_has_fixed_boundary() {
        let mut queue = EventQueue::new(100);
        queue.push(batch(), 8);
        let end = queue.boundary();
        queue.push(batch(), 8);
        let first = queue.delivery(Some(end)).unwrap();
        queue.ack(first.sequence).unwrap();
        assert!(queue.delivery(Some(end)).is_none());
        assert!(queue.delivery(None).is_some());
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::events::SuspendPolicy;

    #[test]
    fn new_overflow_does_not_extend_drain_forever() {
        let mut queue = EventQueue::new(10);
        let batch = EventBatch {
            received_at_ms: 0,
            suspend_policy: SuspendPolicy::None,
            events: vec![],
        };
        queue.push(batch.clone(), 8);
        let boundary = queue.boundary();
        queue.push(batch.clone(), 8);
        let notification = queue.delivery(Some(boundary)).unwrap();
        assert!(matches!(
            notification.payload,
            Payload::Dropped { packets: 1 }
        ));
        queue.ack(notification.sequence).unwrap();
        queue.push(batch, 8);
        assert!(queue.delivery(Some(boundary)).is_none());
        assert!(matches!(
            queue.delivery(None).unwrap().payload,
            Payload::Dropped { packets: 1 }
        ));
    }
}

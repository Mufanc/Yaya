mod queue;

pub use queue::EventQueue;

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

use crate::wire::{self, IdSizes, Reader};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Location {
    pub type_tag: u8,
    pub type_id: String,
    pub method_id: String,
    pub code_index: String,
}

impl Location {
    pub fn read(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: wire::id(reader.read_id(sizes.reference_type)?),
            method_id: wire::id(reader.read_id(sizes.method)?),
            code_index: wire::id(reader.read_u64()?),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Breakpoint {
        request_id: u32,
        thread: String,
        location: Location,
    },
    StepCompleted {
        request_id: u32,
        thread: String,
        location: Location,
    },
    ThreadStarted {
        request_id: u32,
        thread: String,
    },
    ThreadDied {
        request_id: u32,
        thread: String,
    },
    ClassPrepared {
        request_id: u32,
        thread: String,
        type_tag: u8,
        type_id: String,
        signature: String,
        status: u32,
    },
    ClassUnloaded {
        request_id: u32,
        signature: String,
    },
    VmStarted {
        request_id: u32,
        thread: String,
    },
    VmDied {
        request_id: u32,
    },
    Unknown {
        packet_id: u32,
        command_set: u8,
        command: u8,
        #[serde(with = "wire::hex_bytes", rename = "payload_hex")]
        payload: Vec<u8>,
        diagnostic: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuspendPolicy {
    None,
    EventThread,
    All,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventBatch {
    pub received_at_ms: u64,
    pub suspend_policy: SuspendPolicy,
    pub events: Vec<Event>,
}

impl EventBatch {
    pub fn decode(packet_id: u32, set: u8, command: u8, bytes: &[u8], sizes: IdSizes) -> Self {
        let received_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let policy = if (set, command) == (64, 100) {
            match bytes.first() {
                Some(0) => SuspendPolicy::None,
                Some(1) => SuspendPolicy::EventThread,
                Some(2) => SuspendPolicy::All,
                _ => SuspendPolicy::Unknown,
            }
        } else {
            SuspendPolicy::Unknown
        };
        let decoded = Self::parse(set, command, bytes, sizes);
        let events = match decoded {
            Ok(events) => events,
            Err(error) => vec![Event::Unknown {
                packet_id,
                command_set: set,
                command,
                payload: bytes.to_vec(),
                diagnostic: error.to_string(),
            }],
        };

        Self {
            received_at_ms,
            suspend_policy: policy,
            events,
        }
    }

    fn parse(set: u8, command: u8, bytes: &[u8], sizes: IdSizes) -> anyhow::Result<Vec<Event>> {
        ensure!((set, command) == (64, 100), "unsupported target command");

        let mut reader = Reader::new(bytes);
        let policy = reader.read_u8()?;

        ensure!(policy <= 2, "invalid suspend policy");

        let count = reader.read_count()?;
        let mut events = Vec::new();

        for _ in 0..count {
            let kind = reader.read_u8()?;
            let request_id = reader.read_u32()?;
            let event = match kind {
                1 | 2 => {
                    let thread = wire::id(reader.read_id(sizes.object)?);
                    let location = Location::read(&mut reader, sizes)?;

                    if kind == 1 {
                        Event::StepCompleted {
                            request_id,
                            thread,
                            location,
                        }
                    } else {
                        Event::Breakpoint {
                            request_id,
                            thread,
                            location,
                        }
                    }
                }
                6 => Event::ThreadStarted {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                7 => Event::ThreadDied {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                8 => Event::ClassPrepared {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                    type_tag: reader.read_u8()?,
                    type_id: wire::id(reader.read_id(sizes.reference_type)?),
                    signature: reader.read_string()?,
                    status: reader.read_u32()?,
                },
                9 => Event::ClassUnloaded {
                    request_id,
                    signature: reader.read_string()?,
                },
                90 => Event::VmStarted {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                99 => Event::VmDied { request_id },
                _ => bail!("unsupported event kind {kind}; entire Composite retained"),
            };

            events.push(event);
        }

        reader.finish()?;

        Ok(events)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Events { batch: EventBatch },
    Dropped { packets: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Delivery {
    pub sequence: u64,
    pub payload: Payload,
}

impl From<crate::jdwp::location::Location> for Location {
    fn from(location: crate::jdwp::location::Location) -> Self {
        Self {
            type_tag: location.type_tag,
            type_id: crate::wire::id(location.type_id),
            method_id: crate::wire::id(location.method_id),
            code_index: crate::wire::id(location.code_index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_composite_keeps_entire_payload() {
        let payload = [2, 0, 0, 0, 1, 255, 0, 0, 0, 1];
        let sizes = IdSizes {
            field: 8,
            method: 8,
            object: 8,
            reference_type: 8,
            frame: 8,
        };
        let batch = EventBatch::decode(1, 64, 100, &payload, sizes);
        assert_eq!(batch.suspend_policy, SuspendPolicy::All);
        assert!(matches!(&batch.events[0], Event::Unknown { payload: raw, .. } if raw == &payload));
    }
}

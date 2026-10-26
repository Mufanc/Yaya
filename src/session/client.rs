use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use clap::Args;
use tokio::net::UnixStream;
use tokio::time::{self, Instant};

use super::protocol::{ClientMessage, PROTOCOL, ServerMessage};
use super::transport::{self, Socket, Transport};
use crate::cli::Command;
use crate::commands::{CommandResult, Data};
use crate::events::{Delivery, Payload, SuspendPolicy};
use crate::jdwp::Failure;
use crate::reporter::Reporter;

pub async fn connect(id: &str) -> anyhow::Result<Socket<ClientMessage, ServerMessage>> {
    let stream = UnixStream::connect(super::socket_path(id)?)
        .await
        .map_err(|error| Failure::new("session_unavailable", error.to_string()))?;

    Ok(Socket::new(stream))
}

pub async fn stop(
    connection: &mut impl Transport<Outgoing = ClientMessage, Incoming = ServerMessage>,
    id: &str,
) -> anyhow::Result<Data> {
    time::timeout(Duration::from_secs(5), async {
        connection
            .send(ClientMessage::Stop { session: id.into() })
            .await?;

        match transport::receive(connection).await? {
            ServerMessage::Stopped => Ok(Data::Confirmed {
                operation: "daemon_stop".into(),
            }),
            ServerMessage::Error { error } => Err(error.into()),
            _ => bail!("invalid daemon stop response"),
        }
    })
    .await
    .context("daemon stop timeout; shutdown outcome is unknown")?
}

pub async fn execute<
    S: Args,
    T: Transport<Outgoing = ClientMessage<S>, Incoming = ServerMessage>,
>(
    connection: &mut T,
    id: &str,
    command: Command<S>,
    reporter: &mut Reporter,
) -> anyhow::Result<&'static str> {
    time::timeout(Duration::from_secs(5), async {
        connection
            .send(ClientMessage::Hello {
                version: PROTOCOL,
                session: id.into(),
            })
            .await?;

        match transport::receive(connection).await? {
            ServerMessage::Ready { version, session } => ensure!(
                version == PROTOCOL && session == id,
                "invalid server handshake"
            ),
            ServerMessage::Error { error } => return Err(error.into()),
            _ => bail!("invalid server handshake"),
        }

        Ok(())
    })
    .await
    .context("session handshake timeout")??;

    if let Command::Events { wait, collect, .. } = command {
        return events(connection, reporter, wait, collect).await;
    }

    let output = command.output_path().map(Path::to_path_buf);

    connection.send(ClientMessage::Execute { command }).await?;

    match transport::receive(connection).await? {
        ServerMessage::Result { result } => {
            let data = match (result, output) {
                (CommandResult::File { bytes }, Some(path)) => {
                    super::file::save(&path, &bytes).await?
                }
                (CommandResult::File { .. }, None) => {
                    bail!("file response without an output path")
                }
                (_, Some(_)) => bail!("expected a file response"),
                (CommandResult::Data { data }, None) => data,
            };

            reporter.result(data)?;
        }
        ServerMessage::Error { error } => return Err(error.into()),
        _ => bail!("unexpected command response"),
    }

    Ok("completed")
}

fn event_reason(
    delivery: &Delivery,
    wait: Option<u64>,
    collect: Option<u64>,
) -> Option<&'static str> {
    if let Payload::Events { batch } = &delivery.payload {
        if wait.is_some() {
            return Some("event");
        }

        if collect.is_some() && batch.suspend_policy != SuspendPolicy::None {
            return Some("paused");
        }
    }

    None
}

async fn events<S: Args, T: Transport<Outgoing = ClientMessage<S>, Incoming = ServerMessage>>(
    connection: &mut T,
    reporter: &mut Reporter,
    wait: Option<u64>,
    collect: Option<u64>,
) -> anyhow::Result<&'static str> {
    connection
        .send(ClientMessage::Subscribe {
            drain: wait.is_none() && collect.is_none(),
        })
        .await?;

    let deadline = Instant::now() + Duration::from_secs(wait.or(collect).unwrap_or(86400));
    let interrupt = tokio::signal::ctrl_c();

    tokio::pin!(interrupt);

    loop {
        let delivery = tokio::select! {
            biased;
            _ = time::sleep_until(deadline) => return Ok("timeout"),
            _ = &mut interrupt => return Ok("interrupted"),
            message = transport::receive(connection) => match message? {
                ServerMessage::Delivery { delivery } => Some(delivery),
                ServerMessage::Drained => None,
                ServerMessage::Error { error } => return Err(error.into()),
                _ => bail!("unexpected event response"),
            },
        };
        let Some(delivery) = delivery else {
            return Ok("completed");
        };
        let sequence = delivery.sequence;
        let reason = event_reason(&delivery, wait, collect);

        reporter.event(delivery)?;
        connection.send(ClientMessage::Ack { sequence }).await?;

        if let Some(reason) = reason {
            return Ok(reason);
        }

        // Local queues can remain ready forever; let signals, stdin and JDWP I/O run.
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::cli::CurrentSession;
    use crate::events::EventBatch;

    use super::*;

    struct FakeTransport {
        deliveries: VecDeque<Delivery>,
        acknowledged: Vec<u64>,
        advance_on_ack: bool,
        subscribed_drain: Option<bool>,
    }

    impl FakeTransport {
        fn new(payloads: Vec<Payload>) -> Self {
            Self {
                deliveries: payloads
                    .into_iter()
                    .enumerate()
                    .map(|(index, payload)| Delivery {
                        sequence: index as u64,
                        payload,
                    })
                    .collect(),
                acknowledged: Vec::new(),
                advance_on_ack: false,
                subscribed_drain: None,
            }
        }
    }

    impl Transport for FakeTransport {
        type Outgoing = ClientMessage<CurrentSession>;
        type Incoming = ServerMessage;

        async fn send(&mut self, message: Self::Outgoing) -> anyhow::Result<()> {
            match message {
                ClientMessage::Subscribe { drain } => self.subscribed_drain = Some(drain),
                ClientMessage::Ack { sequence } => {
                    assert_eq!(self.deliveries.pop_front().unwrap().sequence, sequence);
                    self.acknowledged.push(sequence);
                    if self.advance_on_ack {
                        time::advance(Duration::from_secs(1)).await;
                    }
                }
                _ => unreachable!("event tests do not send commands"),
            }
            Ok(())
        }

        async fn receive(&mut self) -> anyhow::Result<Option<ServerMessage>> {
            Ok(Some(match self.deliveries.front().cloned() {
                Some(delivery) => ServerMessage::Delivery { delivery },
                None => ServerMessage::Drained,
            }))
        }
    }

    fn batch(policy: SuspendPolicy) -> Payload {
        Payload::Events {
            batch: EventBatch {
                received_at_ms: 0,
                suspend_policy: policy,
                events: Vec::new(),
            },
        }
    }

    #[tokio::test(start_paused = true)]
    async fn collect_checks_deadline_even_when_more_deliveries_are_ready() {
        let mut reporter = Reporter::new();
        reporter.set_session("s_abc123");
        let mut connection =
            FakeTransport::new(vec![batch(SuspendPolicy::None), batch(SuspendPolicy::None)]);
        connection.advance_on_ack = true;
        assert_eq!(
            events(&mut connection, &mut reporter, None, Some(1))
                .await
                .unwrap(),
            "timeout"
        );
        assert_eq!(connection.acknowledged, [0]);
        assert_eq!(connection.deliveries.len(), 1);
        reporter.finish(Ok("timeout")).unwrap();
    }

    #[tokio::test]
    async fn wait_acks_dropped_notice_before_returning_first_batch() {
        let mut reporter = Reporter::new();
        reporter.set_session("s_abc123");
        let mut connection = FakeTransport::new(vec![
            Payload::Dropped { packets: 3 },
            batch(SuspendPolicy::None),
        ]);
        assert_eq!(
            events(&mut connection, &mut reporter, Some(1), None)
                .await
                .unwrap(),
            "event"
        );
        assert_eq!(connection.acknowledged, [0, 1]);
        assert_eq!(connection.subscribed_drain, Some(false));
        reporter.finish(Ok("event")).unwrap();
    }

    #[tokio::test]
    async fn collect_stops_on_pause_and_drain_stops_at_end() {
        let mut reporter = Reporter::new();
        reporter.set_session("s_abc123");
        let mut connection = FakeTransport::new(vec![
            batch(SuspendPolicy::EventThread),
            batch(SuspendPolicy::None),
        ]);
        assert_eq!(
            events(&mut connection, &mut reporter, None, Some(1))
                .await
                .unwrap(),
            "paused"
        );
        assert_eq!(connection.acknowledged, [0]);
        assert_eq!(
            events(&mut connection, &mut reporter, None, None)
                .await
                .unwrap(),
            "completed"
        );
        assert_eq!(connection.acknowledged, [0, 1]);
        assert_eq!(connection.subscribed_drain, Some(true));
        reporter.finish(Ok("completed")).unwrap();
    }
}

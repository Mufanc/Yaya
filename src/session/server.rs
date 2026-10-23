use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, ensure};
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::time;

use crate::cli::Command;
use crate::commands::Core;
use crate::events::Delivery;
use crate::jdwp::{Failure, JdwpClient};

use super::protocol::{ClientMessage, PROTOCOL, ServerMessage};
use super::transport::{self, Transport};

pub struct Server {
    core: Arc<Mutex<Core>>,
    client: Arc<JdwpClient>,
    session: String,
    active: Semaphore,
    stop: watch::Sender<bool>,
}

impl Server {
    pub fn new(core: Core) -> Arc<Self> {
        let (stop, _) = watch::channel(false);

        Arc::new(Self {
            client: core.client.clone(),
            session: core.session.clone(),
            core: Arc::new(Mutex::new(core)),
            active: Semaphore::new(1),
            stop,
        })
    }

    pub fn stopped(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    pub async fn reject<S: clap::Args, T>(mut connection: T)
    where
        T: Transport<Incoming = ClientMessage<S>, Outgoing = ServerMessage>,
    {
        // Consume Hello before closing a socket, so unread input cannot reset the busy reply.
        let _ = time::timeout(Duration::from_millis(100), connection.receive()).await;
        let _ = time::timeout(
            Duration::from_millis(100),
            connection.send(ServerMessage::Error {
                error: Failure::new("session_busy", "another client is connected"),
            }),
        )
        .await;
    }

    pub async fn serve<S, T>(self: Arc<Self>, mut connection: T) -> anyhow::Result<()>
    where
        S: clap::Args + Send + 'static,
        T: Transport<Incoming = ClientMessage<S>, Outgoing = ServerMessage>,
    {
        let Ok(_permit) = self.active.try_acquire() else {
            Self::reject(connection).await;
            return Ok(());
        };
        let hello =
            time::timeout(Duration::from_secs(5), transport::receive(&mut connection)).await??;

        ensure!(
            matches!(hello, ClientMessage::Hello { version: PROTOCOL, session } if session == self.session),
            "IPC session/version mismatch"
        );
        connection
            .send(ServerMessage::Ready {
                version: PROTOCOL,
                session: self.session.clone(),
            })
            .await?;

        let mut disconnected = self.client.disconnected.subscribe();

        if disconnected.borrow().is_some() {
            connection
                .send(ServerMessage::Error {
                    error: Failure::new("disconnected", "target disconnected"),
                })
                .await?;

            return Ok(());
        }

        let request = tokio::select! {
            request = transport::receive(&mut connection) => request?,
            _ = disconnected.changed() => {
                connection.send(ServerMessage::Error { error: Failure::new("disconnected", "target disconnected") }).await?;
                return Ok(());
            }
        };

        match request {
            ClientMessage::Execute { command } => {
                let detach = matches!(command, Command::Detach { .. });
                let core = self.core.clone();
                let stop = self.stop.clone();
                let mut job = tokio::spawn(async move {
                    let result = core.lock().await.execute(command).await;

                    if detach {
                        stop.send_replace(true);
                    }

                    result
                });
                let result = tokio::select! {
                    result = &mut job => result?,
                    _ = connection.receive() => return Ok(()),
                };
                let response = match result {
                    Ok(data) => ServerMessage::Result { data },
                    Err(error) => ServerMessage::Error {
                        error: Failure::from_error(&error),
                    },
                };

                connection.send(response).await?;
            }
            ClientMessage::Subscribe { drain } => {
                let boundary = drain.then(|| self.client.queue.lock().unwrap().boundary());

                loop {
                    let delivery = tokio::select! {
                        delivery = self.next_delivery(boundary) => match delivery {
                            Ok(delivery) => delivery,
                            Err(error) => {
                                connection.send(ServerMessage::Error { error: Failure::from_error(&error) }).await?;
                                break;
                            }
                        },
                        _ = connection.receive() => break,
                    };
                    let Some(delivery) = delivery else {
                        connection.send(ServerMessage::Drained).await?;
                        break;
                    };
                    let sequence = delivery.sequence;

                    connection
                        .send(ServerMessage::Delivery { delivery })
                        .await?;

                    let message = tokio::select! {
                        message = transport::receive(&mut connection) => message?,
                        _ = disconnected.changed() => {
                            connection.send(ServerMessage::Error { error: Failure::new("disconnected", "target disconnected") }).await?;
                            return Ok(());
                        }
                    };

                    ensure!(
                        matches!(message, ClientMessage::Ack { sequence: ack } if ack == sequence),
                        "invalid ACK"
                    );
                    self.client.queue.lock().unwrap().ack(sequence)?;
                }
            }
            _ => bail!("expected command or subscription"),
        }

        Ok(())
    }

    async fn next_delivery(&self, boundary: Option<u64>) -> anyhow::Result<Option<Delivery>> {
        let changed = self.client.queue.lock().unwrap().changed.clone();
        let mut disconnected = self.client.disconnected.subscribe();

        loop {
            if disconnected.borrow().is_some() {
                return Err(Failure::new("disconnected", "target disconnected").into());
            }

            let delivery = self.client.queue.lock().unwrap().delivery(boundary);

            if delivery.is_some() || boundary.is_some() {
                return Ok(delivery);
            }

            tokio::select! {
                _ = changed.notified() => (),
                _ = disconnected.changed() => return Err(Failure::new("disconnected", "target disconnected").into()),
            }
        }
    }
}

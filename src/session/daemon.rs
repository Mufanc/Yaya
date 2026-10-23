mod startup;

pub use startup::Attach;

use startup::Message;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use anyhow::{Context, ensure};
use tokio::net::UnixListener;
use tokio::time::{self, Instant};

use crate::commands::Core;
use crate::jdwp::{Failure, JdwpClient};

use super::protocol::{ClientMessage, ServerMessage};
use super::server::Server;
use super::transport::{Socket, Transport};

const IDLE: Duration = Duration::from_secs(30 * 60);

async fn run(
    id: String,
    host: String,
    port: u16,
    mut report: Socket<Message, Message>,
) -> anyhow::Result<()> {
    let startup = async {
        let path = super::socket_path(&id)?;
        let listener = UnixListener::bind(&path)?;

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;

        let client = JdwpClient::connect(&host, port).await?;
        anyhow::Ok((listener, client))
    };
    let initialized = time::timeout(Duration::from_secs(45), startup)
        .await
        .context("daemon initialization timeout")
        .and_then(|result| result);
    let (listener, client) = match initialized {
        Ok(value) => value,
        Err(error) => {
            let _ = report
                .send(Message::Failed(Failure::from_error(&error)))
                .await;

            return Err(error);
        }
    };
    let handshake = async {
        report.send(Message::Ready).await?;
        ensure!(
            matches!(report.receive().await?, Some(Message::Accepted)),
            "attach did not accept daemon"
        );

        anyhow::Ok(())
    };

    if let Err(error) = time::timeout(Duration::from_secs(5), handshake)
        .await
        .context("attach acknowledgement timeout")
        .and_then(|result| result)
    {
        client.close().await;
        return Err(error);
    }

    drop(report);

    let server = Server::new(Core {
        client: client.clone(),
        session: id,
    });
    let mut stopped = server.stopped();
    let mut disconnected = client.disconnected.subscribe();
    let mut active: Option<tokio::task::JoinHandle<()>> = None;
    let mut idle_since = Instant::now();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    loop {
        if client.disconnected.borrow().is_some() || *stopped.borrow() {
            break;
        }

        tokio::select! {
            biased;
            _ = async { if let Some(task) = active.as_mut() { let _ = task.await; } else { std::future::pending::<()>().await; } } => {
                active = None;
                idle_since = Instant::now();
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted?;

                let connection = Socket::<ServerMessage, ClientMessage>::new(stream);

                if active.is_some() {
                    Server::reject(connection).await;
                } else {
                    let server = server.clone();

                    active = Some(tokio::spawn(async move {
                        let _ = server.serve(connection).await;
                    }));
                }
            }
            _ = time::sleep_until(idle_since + IDLE), if active.is_none() => break,
            _ = disconnected.changed() => break,
            _ = stopped.changed() => break,
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }

    if client.disconnected.borrow().is_none() && !*stopped.borrow() {
        client.close().await;
    }

    if let Some(mut task) = active
        && time::timeout(Duration::from_millis(250), &mut task)
            .await
            .is_err()
    {
        task.abort();
    }

    Ok(())
}

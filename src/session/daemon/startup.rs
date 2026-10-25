use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use daemonize::{Daemonize, Outcome};
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream as AsyncUnixStream;
use tokio::time;

use crate::commands::Data;
use crate::jdwp::Failure;
use crate::reporter::Reporter;
use crate::session::transport::{Socket, Transport};
use crate::session::{self, Cleanup};

#[derive(Deserialize, Serialize)]
pub(super) enum Message {
    Ready,
    Failed(Failure),
    Accepted,
}

pub struct Attach {
    id: String,
    socket: UnixStream,
    parent: bool,
    cleanup: Cleanup,
}

impl Attach {
    pub fn fork() -> anyhow::Result<Self> {
        let (id, cleanup) = session::allocate()?;
        let (parent_socket, child_socket) = UnixStream::pair()?;

        session::socket_path(&id)?;
        parent_socket.set_nonblocking(true)?;
        child_socket.set_nonblocking(true)?;

        let (socket, parent) = match Daemonize::new().umask(0o077).execute() {
            Outcome::Parent(result) => {
                drop(child_socket);
                ensure!(result?.first_child_exit_code == 0, "daemonize child failed");
                (parent_socket, true)
            }
            Outcome::Child(result) => {
                drop(parent_socket);
                result?;
                (child_socket, false)
            }
        };

        Ok(Self {
            id,
            socket,
            parent,
            cleanup,
        })
    }

    pub async fn run(self, host: String, port: u16) -> anyhow::Result<bool> {
        let Self {
            id,
            socket,
            parent,
            cleanup,
        } = self;

        if parent {
            let mut reporter = Reporter::new();

            reporter.set_session(&id);

            let result = async {
                let socket = AsyncUnixStream::from_std(socket)?;
                let mut channel = Socket::new(socket);
                let message = time::timeout(Duration::from_secs(50), channel.receive())
                    .await
                    .context("daemon startup timeout")??
                    .context("daemon exited before ready")?;

                match message {
                    Message::Ready => {
                        channel.send(Message::Accepted).await?;
                        std::mem::forget(cleanup);
                        reporter.result(Data::Attached {
                            endpoint: format!("{host}:{port}"),
                        })?;

                        Ok("completed")
                    }
                    Message::Failed(error) => Err(error.into()),
                    _ => bail!("invalid daemon startup response"),
                }
            }
            .await;

            reporter.finish(result)
        } else {
            let socket = AsyncUnixStream::from_std(socket)?;
            let channel = Socket::new(socket);
            let result = super::run(id, host, port, channel).await;

            drop(cleanup);
            result.map(|()| true)
        }
    }
}

mod class;
mod data;
mod execution;
mod frame;
mod object;

use std::sync::Arc;

use anyhow::bail;

use crate::cli::{Command, Jdwp};
use crate::jdwp::{JdwpClient, virtual_machine};
use crate::wire;

pub use data::Data;

pub struct Core {
    pub client: Arc<JdwpClient>,
    pub session: String,
}

impl Core {
    pub async fn execute<S: clap::Args>(&self, command: Command<S>) -> anyhow::Result<Data> {
        let client = &self.client;
        let sizes = client.sizes;

        match command {
            Command::Status { .. } => {
                let queue = client.queue.lock().unwrap();

                Ok(Data::Status {
                    endpoint: client.endpoint.clone(),
                    queued_wire_bytes: queue.bytes,
                    dropped_packets: queue.dropped,
                    idsizes: sizes,
                })
            }

            Command::Detach { .. } => {
                client.request(virtual_machine::Dispose).await?;

                Ok(confirmed("dispose"))
            }

            Command::Jdwp {
                command: Jdwp::Idsizes,
                ..
            } => Ok(Data::Idsizes { sizes }),
            Command::Jdwp {
                command:
                    Jdwp::Raw {
                        address,
                        payload_hex,
                    },
                ..
            } => {
                let payload = wire::unhex(&payload_hex)?;
                let reply = client
                    .raw(address.command_set, address.command, payload)
                    .await?;

                Ok(Data::Raw {
                    jdwp_request_id: reply.jdwp_request_id,
                    error_code: reply.error_code,
                    payload_hex: wire::hex(&reply.payload),
                })
            }

            Command::Class { command, .. } => self.class(command).await,
            Command::Object { command, .. } => self.object(command).await,
            Command::Break { command, .. } => self.breakpoint(command).await,
            Command::Threads { .. } => self.threads().await,
            Command::Stack { thread, .. } => self.stack(thread).await,
            Command::Frame { command, .. } => self.frame(command).await,
            Command::Step {
                thread,
                into,
                over,
                out,
                ..
            } => self.step(thread, into, over, out).await,
            Command::Resume { thread, all, .. } => self.resume(thread, all).await,
            _ => bail!("command is not valid on an attached session"),
        }
    }
}

fn parse_id(value: &str) -> anyhow::Result<u64> {
    wire::number(value).map_err(anyhow::Error::msg)
}

fn confirmed(operation: &str) -> Data {
    Data::Confirmed {
        operation: operation.into(),
    }
}

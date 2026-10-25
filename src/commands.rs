mod class;
mod data;
mod ddm;
mod execution;
mod frame;
mod object;

use std::sync::Arc;

use anyhow::{Error, bail};
use clap::Args as ClapArgs;
use serde::{Deserialize, Serialize};

use crate::cli::{Command, Jdwp};
use crate::jdwp::JdwpClient;
use crate::jdwp::virtual_machine::Dispose;
use crate::wire;

pub use data::Data;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandResult {
    Data {
        data: Data,
    },
    File {
        #[serde(with = "wire::byte_payload")]
        bytes: Vec<u8>,
    },
}

pub struct Core {
    pub client: Arc<JdwpClient>,
    pub session: String,
}

impl Core {
    pub async fn execute<S: ClapArgs>(&self, command: Command<S>) -> anyhow::Result<CommandResult> {
        let client = &self.client;
        let sizes = client.sizes;

        let data = match command {
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
                client.request(Dispose).await?;

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
                    payload: reply.payload,
                })
            }

            Command::Ddm { command, .. } => self.ddm(command).await,
            Command::Ui { command, .. } => return self.ui(command).await,
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
        }?;

        Ok(CommandResult::Data { data })
    }
}

fn parse_id(value: &str) -> anyhow::Result<u64> {
    wire::number(value).map_err(Error::msg)
}

fn confirmed(operation: &str) -> Data {
    Data::Confirmed {
        operation: operation.into(),
    }
}

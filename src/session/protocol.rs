use serde::{Deserialize, Serialize};

use crate::cli::Command;
use crate::commands::Data;
use crate::events::Delivery;
use crate::jdwp::Failure;

pub(super) const PROTOCOL: u32 = 4;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[serde(bound(serialize = "", deserialize = "S: Default"))]
pub(crate) enum ClientMessage<S: clap::Args = crate::cli::SessionArgs> {
    Hello { version: u32, session: String },
    Execute { command: Command<S> },
    Subscribe { drain: bool },
    Ack { sequence: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ServerMessage {
    Ready { version: u32, session: String },
    Result { data: Data },
    Error { error: Failure },
    Delivery { delivery: Delivery },
    Drained,
}

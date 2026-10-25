use anyhow::{bail, ensure};

use crate::wire::{IdSizes, Writer};

use super::Command;
use super::location::Location;

const COMMAND_SET: u8 = 15;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum EventKind {
    SingleStep = 1,
    Breakpoint = 2,
    Exception = 4,
    ClassPrepare = 8,
    FieldAccess = 20,
    FieldModification = 21,
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum SuspendPolicy {
    None = 0,
    EventThread = 1,
    All = 2,
}

impl SuspendPolicy {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "thread" => Ok(Self::EventThread),
            "all" => Ok(Self::All),
            _ => bail!("invalid suspend policy"),
        }
    }
}

#[derive(Clone, Copy)]
#[repr(i32)]
pub enum StepSize {
    Min = 0,
}

#[derive(Clone, Copy)]
#[repr(i32)]
pub enum StepDepth {
    Into = 0,
    Over = 1,
    Out = 2,
}

pub enum Modifier {
    Count(u32),
    ThreadOnly(u64),
    ClassMatch(String),
    ExceptionOnly {
        exception: u64,
        caught: bool,
        uncaught: bool,
    },
    FieldOnly {
        declaring: u64,
        field: u64,
    },
    LocationOnly(Location),
    Step {
        thread: u64,
        size: StepSize,
        depth: StepDepth,
    },
}

pub struct Set {
    pub kind: EventKind,
    pub suspend_policy: SuspendPolicy,
    pub modifiers: Vec<Modifier>,
}

impl Command for Set {
    type Response = u32;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_u8(self.kind as u8);
        writer.write_u8(self.suspend_policy as u8);
        writer.write_count(self.modifiers.len())?;

        for modifier in &self.modifiers {
            match modifier {
                Modifier::Count(count) => {
                    ensure!(*count > 0, "event count must be positive");

                    writer.write_u8(1);
                    writer.write_i32((*count).try_into()?);
                }
                Modifier::ThreadOnly(thread) => {
                    writer.write_u8(3);
                    writer.write_id(*thread, sizes.object)?;
                }
                Modifier::ClassMatch(pattern) => {
                    writer.write_u8(5);
                    writer.write_string(pattern)?;
                }
                Modifier::ExceptionOnly {
                    exception,
                    caught,
                    uncaught,
                } => {
                    writer.write_u8(8);
                    writer.write_id(*exception, sizes.reference_type)?;
                    writer.write_u8(u8::from(*caught));
                    writer.write_u8(u8::from(*uncaught));
                }
                Modifier::FieldOnly { declaring, field } => {
                    writer.write_u8(9);
                    writer.write_id(*declaring, sizes.reference_type)?;
                    writer.write_id(*field, sizes.field)?;
                }
                Modifier::LocationOnly(location) => {
                    writer.write_u8(7);
                    location.write(writer, sizes)?;
                }
                Modifier::Step {
                    thread,
                    size,
                    depth,
                } => {
                    writer.write_u8(10);
                    writer.write_id(*thread, sizes.object)?;
                    writer.write_i32(*size as i32);
                    writer.write_i32(*depth as i32);
                }
            }
        }

        Ok(())
    }
}

pub struct Clear {
    pub kind: EventKind,
    pub request_id: u32,
}

impl Command for Clear {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, _sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_u8(self.kind as u8);
        writer.write_u32(self.request_id);

        Ok(())
    }
}

pub struct ClearAllBreakpoints;

impl Command for ClearAllBreakpoints {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 3;
}

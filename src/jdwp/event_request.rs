use crate::wire::{IdSizes, Writer};

use super::Command;
use super::location::Location;

const COMMAND_SET: u8 = 15;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum EventKind {
    SingleStep = 1,
    Breakpoint = 2,
}

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum SuspendPolicy {
    None = 0,
    EventThread = 1,
    All = 2,
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
                    writer.write_u8(1);
                    writer.write_i32((*count).try_into()?);
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

    const COMMAND_SET: u8 = self::COMMAND_SET;
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

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 3;
}

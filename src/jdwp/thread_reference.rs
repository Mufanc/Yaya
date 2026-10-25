use crate::wire::{IdSizes, Reader, Writer};

use super::location::Location;
use super::{Command, Response};

const COMMAND_SET: u8 = 11;

pub struct ThreadStatus {
    pub status: u32,
    pub suspend_status: u32,
}

pub struct Frame {
    pub id: u64,
    pub location: Location,
}

pub struct Name {
    pub thread: u64,
}

impl Command for Name {
    type Response = String;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)
    }
}

pub struct Resume {
    pub thread: u64,
}

impl Command for Resume {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 3;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)
    }
}

pub struct Status {
    pub thread: u64,
}

impl Command for Status {
    type Response = ThreadStatus;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 4;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)
    }
}

pub struct Frames {
    pub thread: u64,
    pub start: u32,
    pub length: i32,
}

impl Command for Frames {
    type Response = Vec<Frame>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 6;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)?;
        writer.write_i32(self.start.try_into()?);
        writer.write_i32(self.length);

        Ok(())
    }
}

impl Response for ThreadStatus {
    fn decode(reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            status: reader.read_u32()?,
            suspend_status: reader.read_u32()?,
        })
    }
}

impl Response for Frame {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            id: reader.read_id(sizes.frame)?,
            location: Location::read(reader, sizes)?,
        })
    }
}

pub struct Suspend {
    pub thread: u64,
}

impl Command for Suspend {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)
    }
}

pub struct SuspendCount {
    pub thread: u64,
}

impl Command for SuspendCount {
    type Response = u32;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 12;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)
    }
}

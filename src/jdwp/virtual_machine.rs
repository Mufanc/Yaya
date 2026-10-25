use super::{Command, ObjectId, Response};
use crate::wire::{IdSizes as TargetIdSizes, Reader, Writer};

const COMMAND_SET: u8 = 1;

#[derive(Debug)]
pub struct LoadedClass {
    pub type_tag: u8,
    pub type_id: u64,
    pub signature: String,
    pub status: u32,
}

#[derive(Debug)]
pub struct MatchingClass {
    pub type_tag: u8,
    pub type_id: u64,
    pub status: u32,
}

pub struct AllClasses;

impl Command for AllClasses {
    type Response = Vec<LoadedClass>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 3;
}

pub struct ClassesBySignature {
    pub signature: String,
}

impl Command for ClassesBySignature {
    type Response = Vec<MatchingClass>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, _sizes: TargetIdSizes) -> anyhow::Result<()> {
        writer.write_string(&self.signature)?;

        Ok(())
    }
}

pub struct AllThreads;

impl Command for AllThreads {
    type Response = Vec<ObjectId>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 4;
}

pub struct Dispose;

impl Command for Dispose {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 6;
}

pub struct Resume;

impl Command for Resume {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 9;
}

pub struct IdSizes;

impl Command for IdSizes {
    type Response = TargetIdSizes;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 7;
}

impl Response for LoadedClass {
    fn decode(reader: &mut Reader<'_>, sizes: TargetIdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: reader.read_id(sizes.reference_type)?,
            signature: reader.read_string()?,
            status: reader.read_u32()?,
        })
    }
}

impl Response for MatchingClass {
    fn decode(reader: &mut Reader<'_>, sizes: TargetIdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: reader.read_id(sizes.reference_type)?,
            status: reader.read_u32()?,
        })
    }
}

pub struct Suspend;

impl Command for Suspend {
    type Response = ();

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 8;
}

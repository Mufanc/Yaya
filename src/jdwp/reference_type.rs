use anyhow::ensure;

use crate::wire::{IdSizes, Reader, Writer};

use super::value::{self, Value};
use super::{Command, ObjectId, ReferenceTypeId, Response};

const COMMAND_SET: u8 = 2;

pub const CLASS_TAG: u8 = 1;
pub const INTERFACE_TAG: u8 = 2;
pub const STATIC_MODIFIER: u32 = 0x0008;

#[derive(Debug)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub signature: String,
    pub modifiers: u32,
}

fn members(reader: &mut Reader<'_>, id_size: usize) -> anyhow::Result<Vec<Member>> {
    (0..reader.read_count()?)
        .map(|_| {
            Ok(Member {
                id: reader.read_id(id_size)?,
                name: reader.read_string()?,
                signature: reader.read_string()?,
                modifiers: reader.read_u32()?,
            })
        })
        .collect()
}

pub struct Signature {
    pub type_id: u64,
}

impl Command for Signature {
    type Response = String;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)
    }
}

pub struct ClassLoader {
    pub type_id: u64,
}

impl Command for ClassLoader {
    type Response = ObjectId;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)
    }
}

pub struct Fields {
    pub type_id: u64,
}

impl Command for Fields {
    type Response = FieldMembers;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 4;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)
    }
}

pub struct Methods {
    pub type_id: u64,
}

impl Command for Methods {
    type Response = MethodMembers;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 5;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)
    }
}

pub struct Interfaces {
    pub type_id: u64,
}

impl Command for Interfaces {
    type Response = Vec<ReferenceTypeId>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 10;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)
    }
}

pub struct Instances {
    pub type_id: u64,
    pub limit: u32,
}

impl Command for Instances {
    type Response = Vec<Value>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 16;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        ensure!(
            (1..=i32::MAX as u32).contains(&self.limit),
            "invalid instance limit"
        );
        writer.write_id(self.type_id, sizes.reference_type)?;
        writer.write_i32(self.limit.try_into()?);

        Ok(())
    }
}

pub struct GetValues {
    pub type_id: u64,
    pub fields: Vec<u64>,
}

impl Command for GetValues {
    type Response = Vec<Value>;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 6;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.type_id, sizes.reference_type)?;
        writer.write_count(self.fields.len())?;

        for field in &self.fields {
            writer.write_id(*field, sizes.field)?;
        }

        Ok(())
    }

    fn decode_response(
        &self,
        reader: &mut Reader<'_>,
        sizes: IdSizes,
    ) -> anyhow::Result<Self::Response> {
        value::decode_values(reader, sizes, self.fields.len(), "field")
    }
}

pub struct FieldMembers(pub Vec<Member>);

impl Response for FieldMembers {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self(members(reader, sizes.field)?))
    }
}

pub struct MethodMembers(pub Vec<Member>);

impl Response for MethodMembers {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self(members(reader, sizes.method)?))
    }
}

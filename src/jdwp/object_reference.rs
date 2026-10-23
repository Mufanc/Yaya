use crate::wire::{IdSizes, Reader, Writer};

use super::value::Value;
use super::{Command, Response};

const COMMAND_SET: u8 = 9;

pub struct ObjectType {
    pub type_tag: u8,
    pub type_id: u64,
}

pub struct ReferenceType {
    pub object: u64,
}

impl Command for ReferenceType {
    type Response = ObjectType;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.object, sizes.object)?;

        Ok(())
    }
}

pub struct GetValues {
    pub object: u64,
    pub fields: Vec<u64>,
}

impl Command for GetValues {
    type Response = Vec<Value>;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.object, sizes.object)?;
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
        let values = Self::Response::decode(reader, sizes)?;

        anyhow::ensure!(
            values.len() == self.fields.len(),
            "field reply count mismatch"
        );

        Ok(values)
    }
}

pub struct SetValues {
    pub object: u64,
    pub fields: Vec<(u64, Value)>,
}

impl Command for SetValues {
    type Response = ();

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 3;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.object, sizes.object)?;
        writer.write_count(self.fields.len())?;

        for (field, value) in &self.fields {
            writer.write_id(*field, sizes.field)?;
            value.write_untagged(writer, sizes)?;
        }

        Ok(())
    }
}

impl Response for ObjectType {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: reader.read_id(sizes.reference_type)?,
        })
    }
}

use crate::wire::{IdSizes, Writer};

use super::value::Value;
use super::{Command, ReferenceTypeId};

const COMMAND_SET: u8 = 3;

pub struct Superclass {
    pub class: u64,
}

impl Command for Superclass {
    type Response = ReferenceTypeId;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.class, sizes.reference_type)?;

        Ok(())
    }
}

pub struct SetValues {
    pub class: u64,
    pub fields: Vec<(u64, Value)>,
}

impl Command for SetValues {
    type Response = ();

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.class, sizes.reference_type)?;
        writer.write_count(self.fields.len())?;

        for (field, value) in &self.fields {
            writer.write_id(*field, sizes.field)?;
            value.write_untagged(writer, sizes)?;
        }

        Ok(())
    }
}

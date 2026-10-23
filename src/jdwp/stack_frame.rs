use crate::wire::{IdSizes, Reader, Writer};

use super::value::Value;
use super::{Command, Response};

const COMMAND_SET: u8 = 16;

pub struct Slot {
    pub index: u32,
    pub tag: u8,
}

pub struct GetValues {
    pub thread: u64,
    pub frame: u64,
    pub slots: Vec<Slot>,
}

impl Command for GetValues {
    type Response = Vec<Value>;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)?;
        writer.write_id(self.frame, sizes.frame)?;
        writer.write_count(self.slots.len())?;

        for slot in &self.slots {
            anyhow::ensure!(
                slot.index <= i32::MAX as u32,
                "slot exceeds signed JDWP integer"
            );
            writer.write_i32(slot.index.try_into()?);
            writer.write_u8(slot.tag);
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
            values.len() == self.slots.len(),
            "register reply count mismatch"
        );

        Ok(values)
    }
}

pub struct SetValues {
    pub thread: u64,
    pub frame: u64,
    pub slots: Vec<(u32, Value)>,
}

impl Command for SetValues {
    type Response = ();

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.thread, sizes.object)?;
        writer.write_id(self.frame, sizes.frame)?;
        writer.write_count(self.slots.len())?;

        for (slot, value) in &self.slots {
            anyhow::ensure!(*slot <= i32::MAX as u32, "slot exceeds signed JDWP integer");
            writer.write_i32((*slot).try_into()?);
            writer.write_u8(value.tag());
            value.write_untagged(writer, sizes)?;
        }

        Ok(())
    }
}

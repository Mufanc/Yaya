use crate::wire::{IdSizes, Writer};

use super::Command;

const COMMAND_SET: u8 = 10;

pub struct Value {
    pub string: u64,
}

impl Command for Value {
    type Response = String;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.string, sizes.object)?;
        Ok(())
    }
}

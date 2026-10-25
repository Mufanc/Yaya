use anyhow::ensure;

use crate::wire::{IdSizes, Reader, Writer};

use super::value::Value;
use super::{Command, Response};

const COMMAND_SET: u8 = 13;

pub struct Length {
    pub array: u64,
}

impl Command for Length {
    type Response = u32;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.array, sizes.object)?;
        Ok(())
    }
}

pub struct GetValues {
    pub array: u64,
    pub start: u32,
    pub length: u32,
}

impl Command for GetValues {
    type Response = Region;

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 2;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.array, sizes.object)?;
        writer.write_i32(self.start.try_into()?);
        writer.write_i32(self.length.try_into()?);
        Ok(())
    }

    fn decode_response(&self, reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Region> {
        let region = Region::decode(reader, sizes)?;

        ensure!(
            region.values.len() == self.length as usize,
            "array reply count mismatch"
        );

        Ok(region)
    }
}

pub struct SetValues {
    pub array: u64,
    pub start: u32,
    pub values: Vec<Value>,
}

impl Command for SetValues {
    type Response = ();

    const COMMAND_SET: u8 = self::COMMAND_SET;
    const COMMAND: u8 = 3;

    fn encode_request(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_id(self.array, sizes.object)?;
        writer.write_i32(self.start.try_into()?);
        writer.write_count(self.values.len())?;

        for value in &self.values {
            value.write_untagged(writer, sizes)?;
        }

        Ok(())
    }
}

pub struct Region {
    pub tag: u8,
    pub values: Vec<Value>,
}

impl Response for Region {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        let tag = reader.read_u8()?;
        let objects = matches!(tag, b'[' | b'L' | b's' | b't' | b'g' | b'l' | b'c');

        ensure!(
            objects || matches!(tag, b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D'),
            "invalid array component tag {tag}"
        );

        let count = reader.read_count()?;
        let mut values = Vec::new();

        for _ in 0..count {
            let value = if objects {
                let value = Value::decode(reader, sizes)?;

                ensure!(
                    matches!(value, Value::Object { .. }),
                    "array object element has primitive tag"
                );

                value
            } else {
                Value::read_untagged(tag, reader, sizes)?
            };

            values.push(value);
        }

        Ok(Self { tag, values })
    }
}

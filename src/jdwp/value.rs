use super::Response;
use anyhow::bail;

use crate::wire::{self, IdSizes, Reader, Writer};

#[derive(Debug)]
pub enum Value {
    Boolean(bool),
    Byte(i8),
    Char(u16),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Void,
    Object { tag: u8, id: u64 },
}

impl Value {
    pub fn parse(tag: u8, text: &str) -> anyhow::Result<Self> {
        Ok(match tag {
            b'Z' => Self::Boolean(match text {
                "true" => true,
                "false" => false,
                _ => bail!("boolean must be true or false"),
            }),
            b'B' => Self::Byte(text.parse()?),
            b'C' => Self::Char(text.parse()?),
            b'S' => Self::Short(text.parse()?),
            b'I' => Self::Int(text.parse()?),
            b'J' => Self::Long(text.parse()?),
            b'F' => Self::Float(text.parse()?),
            b'D' => Self::Double(text.parse()?),
            b'L' => Self::Object {
                tag,
                id: if text == "null" {
                    0
                } else {
                    wire::number(text).map_err(anyhow::Error::msg)?
                },
            },
            _ => bail!("unsupported write tag"),
        })
    }

    pub fn tag(&self) -> u8 {
        match self {
            Self::Boolean(_) => b'Z',
            Self::Byte(_) => b'B',
            Self::Char(_) => b'C',
            Self::Short(_) => b'S',
            Self::Int(_) => b'I',
            Self::Long(_) => b'J',
            Self::Float(_) => b'F',
            Self::Double(_) => b'D',
            Self::Void => b'V',
            Self::Object { tag, .. } => *tag,
        }
    }

    pub fn write_untagged(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        match self {
            Self::Boolean(value) => writer.write_u8(u8::from(*value)),
            Self::Byte(value) => writer.write_i8(*value),
            Self::Char(value) => writer.write_u16(*value),
            Self::Short(value) => writer.write_i16(*value),
            Self::Int(value) => writer.write_i32(*value),
            Self::Long(value) => writer.write_i64(*value),
            Self::Float(value) => writer.write_u32(value.to_bits()),
            Self::Double(value) => writer.write_u64(value.to_bits()),
            Self::Object { id, .. } => writer.write_id(*id, sizes.object)?,
            Self::Void => bail!("cannot write a void value"),
        }

        Ok(())
    }
}

impl Response for Value {
    fn decode(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        let tag = reader.read_u8()?;

        Ok(match tag {
            b'Z' => Self::Boolean(reader.read_u8()? != 0),
            b'B' => Self::Byte(reader.read_i8()?),
            b'C' => Self::Char(reader.read_u16()?),
            b'S' => Self::Short(reader.read_i16()?),
            b'I' => Self::Int(reader.read_i32()?),
            b'J' => Self::Long(reader.read_i64()?),
            b'F' => Self::Float(f32::from_bits(reader.read_u32()?)),
            b'D' => Self::Double(f64::from_bits(reader.read_u64()?)),
            b'V' => Self::Void,
            b'[' | b'L' | b's' | b't' | b'g' | b'l' | b'c' => Self::Object {
                tag,
                id: reader.read_id(sizes.object)?,
            },
            _ => bail!("unsupported value tag {tag}"),
        })
    }
}

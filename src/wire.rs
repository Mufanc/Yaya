use anyhow::ensure;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct IdSizes {
    pub field: usize,
    pub method: usize,
    pub object: usize,
    pub reference_type: usize,
    pub frame: usize,
}

impl IdSizes {
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut reader = Reader::new(bytes);
        let mut sizes = [0; 5];

        for size in &mut sizes {
            *size = reader.read_i32()?.try_into()?;
            ensure!((1..=8).contains(size), "unsupported ID width {size}");
        }

        reader.finish()?;

        Ok(Self {
            field: sizes[0],
            method: sizes[1],
            object: sizes[2],
            reference_type: sizes[3],
            frame: sizes[4],
        })
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn read_bytes(&mut self, length: usize) -> anyhow::Result<&'a [u8]> {
        ensure!(length <= self.bytes.len(), "truncated JDWP payload");

        let (head, tail) = self.bytes.split_at(length);

        self.bytes = tail;

        Ok(head)
    }

    pub fn read_u8(&mut self) -> anyhow::Result<u8> {
        Ok(u8::from_be_bytes(self.read_bytes(1)?.try_into()?))
    }

    pub fn read_i8(&mut self) -> anyhow::Result<i8> {
        Ok(i8::from_be_bytes(self.read_bytes(1)?.try_into()?))
    }

    pub fn read_u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_be_bytes(self.read_bytes(2)?.try_into()?))
    }

    pub fn read_i16(&mut self) -> anyhow::Result<i16> {
        Ok(i16::from_be_bytes(self.read_bytes(2)?.try_into()?))
    }

    pub fn read_u32(&mut self) -> anyhow::Result<u32> {
        Ok(u32::from_be_bytes(self.read_bytes(4)?.try_into()?))
    }

    pub fn read_i32(&mut self) -> anyhow::Result<i32> {
        Ok(i32::from_be_bytes(self.read_bytes(4)?.try_into()?))
    }

    pub fn read_u64(&mut self) -> anyhow::Result<u64> {
        Ok(u64::from_be_bytes(self.read_bytes(8)?.try_into()?))
    }

    pub fn read_i64(&mut self) -> anyhow::Result<i64> {
        Ok(i64::from_be_bytes(self.read_bytes(8)?.try_into()?))
    }

    pub fn read_count(&mut self) -> anyhow::Result<usize> {
        let count = self.read_i32()?;

        ensure!(
            count >= 0 && count as usize <= self.bytes.len(),
            "invalid JDWP item count"
        );

        Ok(count as usize)
    }

    pub fn read_id(&mut self, size: usize) -> anyhow::Result<u64> {
        ensure!((1..=8).contains(&size), "invalid ID width");

        let mut bytes = [0; 8];

        bytes[8 - size..].copy_from_slice(self.read_bytes(size)?);

        Ok(u64::from_be_bytes(bytes))
    }

    pub fn read_string(&mut self) -> anyhow::Result<String> {
        let length = self.read_i32()?;

        ensure!(length >= 0, "negative JDWP string length");

        Ok(String::from_utf8(
            self.read_bytes(length as usize)?.to_vec(),
        )?)
    }

    pub fn finish(self) -> anyhow::Result<()> {
        ensure!(self.bytes.is_empty(), "trailing JDWP payload bytes");
        Ok(())
    }
}

#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn write_u8(&mut self, value: u8) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_i8(&mut self, value: i8) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_i16(&mut self, value: i16) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_i32(&mut self, value: i32) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_i64(&mut self, value: i64) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub fn write_count(&mut self, count: usize) -> anyhow::Result<()> {
        self.write_i32(count.try_into()?);
        Ok(())
    }

    pub fn write_id(&mut self, value: u64, size: usize) -> anyhow::Result<()> {
        ensure!((1..=8).contains(&size), "invalid ID width");
        ensure!(
            size == 8 || value < (1u64 << (size * 8)),
            "ID exceeds target width"
        );
        self.write_bytes(&value.to_be_bytes()[8 - size..]);

        Ok(())
    }

    pub fn write_string(&mut self, value: &str) -> anyhow::Result<()> {
        self.write_count(value.len())?;
        self.write_bytes(value.as_bytes());
        Ok(())
    }
}

pub fn number(value: &str) -> Result<u64, String> {
    let result = if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    };

    result.map_err(|_| format!("invalid unsigned integer: {value}"))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(value: &str) -> anyhow::Result<Vec<u8>> {
    ensure!(
        value.len().is_multiple_of(2) && value.is_ascii(),
        "hex requires pairs of ASCII digits"
    );

    (0..value.len())
        .step_by(2)
        .map(|i| Ok(u8::from_str_radix(&value[i..i + 2], 16)?))
        .collect()
}

pub fn id(value: u64) -> String {
    format!("0x{value:x}")
}

pub fn value_tag(name: &str) -> Result<u8, String> {
    match name {
        "boolean" => Ok(b'Z'),
        "byte" => Ok(b'B'),
        "char" => Ok(b'C'),
        "short" => Ok(b'S'),
        "int" => Ok(b'I'),
        "long" => Ok(b'J'),
        "float" => Ok(b'F'),
        "double" => Ok(b'D'),
        "object" => Ok(b'L'),
        _ => Err(format!("unsupported value type: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_values_and_lengths() {
        let mut writer = Writer::default();
        writer.write_i8(i8::MIN);
        writer.write_i16(i16::MIN);
        writer.write_i32(-1);
        writer.write_i64(i64::MIN);
        writer.write_string("你好").unwrap();
        writer.write_bytes(&[0xaa]);
        let bytes = writer.into_bytes();
        assert_eq!(&bytes[..7], &[0x80, 0x80, 0, 0xff, 0xff, 0xff, 0xff]);
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_i8().unwrap(), i8::MIN);
        assert_eq!(reader.read_i16().unwrap(), i16::MIN);
        assert_eq!(reader.read_i32().unwrap(), -1);
        assert_eq!(reader.read_i64().unwrap(), i64::MIN);
        assert_eq!(reader.read_string().unwrap(), "你好");
        assert_eq!(reader.read_bytes(1).unwrap(), &[0xaa]);
        reader.finish().unwrap();
        assert!(Reader::new(&[0xff; 4]).read_count().is_err());
        assert!(Reader::new(&[0xff; 4]).read_string().is_err());
        assert!(Reader::new(&[0; 3]).read_i32().is_err());
        assert_eq!(Reader::new(&[0; 4]).read_count().unwrap(), 0);
        assert!(
            Writer::default()
                .write_count(i32::MAX as usize + 1)
                .is_err()
        );
    }

    #[test]
    fn bounds_and_precision() {
        assert!(Reader::new(&[0]).read_u32().is_err());
        assert!(unhex("f").is_err());
        assert!(unhex("zz").is_err());
        let mut writer = Writer::default();
        assert!(writer.write_id(256, 1).is_err());
        writer.write_id(u64::MAX, 8).unwrap();
        assert_eq!(
            Reader::new(&writer.into_bytes()).read_id(8).unwrap(),
            u64::MAX
        );
        assert_eq!(number("0x19").unwrap(), 25);
    }
}

pub mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = String::deserialize(deserializer)?;
        super::unhex(&value).map_err(serde::de::Error::custom)
    }
}

pub mod hierarchy;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use super::{Command, Failure, Response};
use crate::wire::{self, IdSizes, Reader, Writer};

const COMMAND_SET: u8 = 199;
const DUMP_HIERARCHY: i32 = 1;
const CAPTURE_VIEW: i32 = 1;

pub enum Request {
    Features,
    Windows,
    Dump { window: String },
    Capture { window: String, view: String },
    Raw { chunk: String, payload: Vec<u8> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub chunk: String,
    #[serde(with = "wire::byte_payload")]
    pub payload: Vec<u8>,
}

pub struct Chunks(pub Vec<Chunk>);

impl Command for Request {
    type Response = Chunks;

    const COMMAND_SET: u8 = COMMAND_SET;
    const COMMAND: u8 = 1;

    fn encode_request(&self, writer: &mut Writer, _sizes: IdSizes) -> anyhow::Result<()> {
        let mut payload = Writer::default();
        let chunk = match self {
            Self::Features => "FEAT",
            Self::Windows => "VULW",
            Self::Dump { window } => {
                payload.write_i32(DUMP_HIERARCHY);
                write_string(&mut payload, window)?;
                payload.write_i32(0);
                payload.write_i32(1);
                payload.write_i32(1);

                "VURT"
            }
            Self::Capture { window, view } => {
                payload.write_i32(CAPTURE_VIEW);
                write_string(&mut payload, window)?;
                write_string(&mut payload, view)?;
                "VUOP"
            }
            Self::Raw {
                chunk,
                payload: bytes,
            } => {
                payload.write_bytes(bytes);
                chunk.as_str()
            }
        };
        let payload = payload.into_bytes();

        ensure!(
            chunk.len() == 4 && chunk.is_ascii(),
            "DDM chunk must be four ASCII characters"
        );
        writer.write_bytes(chunk.as_bytes());
        writer.write_count(payload.len())?;
        writer.write_bytes(&payload);

        Ok(())
    }
}

impl Response for Chunks {
    fn decode(reader: &mut Reader<'_>, _sizes: IdSizes) -> anyhow::Result<Self> {
        let mut chunks = Vec::new();

        while reader.remaining() != 0 {
            let name = reader.read_bytes(4)?;

            ensure!(name.is_ascii(), "non-ASCII DDM chunk type");

            let chunk = String::from_utf8(name.to_vec())?;
            let length = reader.read_u32()? as usize;
            let payload = reader.read_bytes(length)?.to_vec();

            if chunk == "FAIL" {
                let mut failure_reader = Reader::new(&payload);
                let code = failure_reader.read_i32()?;
                let message = read_string(&mut failure_reader)?;

                failure_reader.finish()?;

                let mut failure = Failure::new("ddm_error", message);

                failure.ddm_error_code = Some(code);
                failure.outcome = "rejected".into();

                return Err(failure.into());
            }

            chunks.push(Chunk { chunk, payload });
        }

        Ok(Self(chunks))
    }
}

impl Chunks {
    pub fn single(mut self, expected: &str) -> anyhow::Result<Vec<u8>> {
        ensure!(
            self.0.len() == 1,
            "expected one {expected} response chunk, received {}",
            self.0.len()
        );

        let chunk = self.0.pop().context("missing DDM response")?;

        ensure!(
            chunk.chunk == expected,
            "expected {expected}, received {}",
            chunk.chunk
        );

        Ok(chunk.payload)
    }
}

pub fn strings(bytes: &[u8]) -> anyhow::Result<Vec<String>> {
    let mut reader = Reader::new(bytes);
    let count = reader.read_count()?;
    let values = (0..count)
        .map(|_| read_string(&mut reader))
        .collect::<anyhow::Result<_>>()?;

    reader.finish()?;

    Ok(values)
}

fn write_string(writer: &mut Writer, value: &str) -> anyhow::Result<()> {
    writer.write_count(value.encode_utf16().count())?;

    for unit in value.encode_utf16() {
        writer.write_u16(unit);
    }

    Ok(())
}

fn read_string(reader: &mut Reader<'_>) -> anyhow::Result<String> {
    let length = reader.read_count()?;
    let bytes = reader.read_bytes(length.checked_mul(2).context("DDM string too long")?)?;
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();

    Ok(String::from_utf16(&units)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes() -> IdSizes {
        IdSizes {
            field: 8,
            method: 8,
            object: 8,
            reference_type: 8,
            frame: 8,
        }
    }

    #[test]
    fn chunk_requests_encode_lengths_utf16_and_operations() {
        let mut writer = Writer::default();
        Request::Capture {
            window: "窗😀".into(),
            view: "V@1".into(),
        }
        .encode_request(&mut writer, sizes())
        .unwrap();
        assert_eq!(
            wire::hex(&writer.into_bytes()),
            "56554f500000001800000001000000037a97d83dde0000000003005600400031"
        );
        let mut writer = Writer::default();
        Request::Dump { window: "w".into() }
            .encode_request(&mut writer, sizes())
            .unwrap();
        assert_eq!(
            wire::hex(&writer.into_bytes()),
            "565552540000001600000001000000010077000000000000000100000001"
        );
        assert_eq!(
            strings(&[0, 0, 0, 1, 0, 0, 0, 1, 0x7a, 0x97]).unwrap(),
            ["窗"]
        );
        assert!(strings(&[0, 0, 0, 1, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn replies_preserve_empty_chunks_and_ddm_failure() {
        assert!(
            Chunks::decode(&mut Reader::new(&[]), sizes())
                .unwrap()
                .0
                .is_empty()
        );
        let bytes = b"VULW\0\0\0\0FEAT\0\0\0\0";
        assert_eq!(
            Chunks::decode(&mut Reader::new(bytes), sizes())
                .unwrap()
                .0
                .len(),
            2
        );
        for bytes in [b"VUL".as_slice(), b"VULW\0\0\0\x02\0"] {
            assert!(Chunks::decode(&mut Reader::new(bytes), sizes()).is_err());
        }
        let bytes = b"FAIL\0\0\0\x0a\xff\xff\xff\xfe\0\0\0\x01\0x";
        let error = Chunks::decode(&mut Reader::new(bytes), sizes())
            .err()
            .unwrap();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.ddm_error_code, Some(-2));
        assert_eq!(failure.message, "x");
        assert_eq!(failure.error_code, None);
        assert_eq!(failure.code, "ddm_error");
    }
}

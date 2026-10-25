use std::fmt::Formatter;

use serde::de::{Error, Visitor};
use serde::{Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    if serializer.is_human_readable() {
        serializer.serialize_str(&super::hex(bytes))
    } else {
        serializer.serialize_bytes(bytes)
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    // Inspect the value: Serde's tagged-enum buffer loses is_human_readable.
    deserializer.deserialize_any(PayloadVisitor)
}

struct PayloadVisitor;

impl<'de> Visitor<'de> for PayloadVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
        formatter.write_str("a hex string or binary payload")
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
        super::unhex(value).map_err(E::custom)
    }

    fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Ok(value.to_vec())
    }

    fn visit_byte_buf<E: Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(value)
    }
}

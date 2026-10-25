mod queue;

pub use queue::EventQueue;

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

use crate::commands::Value;
use crate::jdwp::Response;
use crate::jdwp::location::Location as JdwpLocation;
use crate::jdwp::value::Value as ProtocolValue;
use crate::wire::{self, IdSizes, Reader};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Location {
    pub type_tag: u8,
    pub type_id: String,
    pub method_id: String,
    pub code_index: String,
}

impl Location {
    pub fn read(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: wire::id(reader.read_id(sizes.reference_type)?),
            method_id: wire::id(reader.read_id(sizes.method)?),
            code_index: wire::id(reader.read_u64()?),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FieldEvent {
    pub thread: String,
    pub location: Location,
    pub type_tag: u8,
    pub type_id: String,
    pub field_id: String,
    pub object: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Exception {
        request_id: u32,
        thread: String,
        location: Location,
        exception: Value,
        catch_location: Option<Location>,
    },
    FieldAccess {
        request_id: u32,
        #[serde(flatten)]
        field: FieldEvent,
    },
    FieldModification {
        request_id: u32,
        #[serde(flatten)]
        field: FieldEvent,
        value_to_be: Value,
    },
    Breakpoint {
        request_id: u32,
        thread: String,
        location: Location,
    },
    StepCompleted {
        request_id: u32,
        thread: String,
        location: Location,
    },
    ThreadStarted {
        request_id: u32,
        thread: String,
    },
    ThreadDied {
        request_id: u32,
        thread: String,
    },
    ClassPrepared {
        request_id: u32,
        thread: String,
        type_tag: u8,
        type_id: String,
        signature: String,
        status: u32,
    },
    ClassUnloaded {
        request_id: u32,
        signature: String,
    },
    VmStarted {
        request_id: u32,
        thread: String,
    },
    VmDied {
        request_id: u32,
    },
    Unknown {
        packet_id: u32,
        command_set: u8,
        command: u8,
        #[serde(with = "wire::byte_payload")]
        payload: Vec<u8>,
        diagnostic: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuspendPolicy {
    None,
    EventThread,
    All,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EventBatch {
    pub received_at_ms: u64,
    pub suspend_policy: SuspendPolicy,
    pub events: Vec<Event>,
}

impl EventBatch {
    pub fn decode(packet_id: u32, set: u8, command: u8, bytes: &[u8], sizes: IdSizes) -> Self {
        let received_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let policy = if (set, command) == (64, 100) {
            match bytes.first() {
                Some(0) => SuspendPolicy::None,
                Some(1) => SuspendPolicy::EventThread,
                Some(2) => SuspendPolicy::All,
                _ => SuspendPolicy::Unknown,
            }
        } else {
            SuspendPolicy::Unknown
        };
        let decoded = Self::parse(set, command, bytes, sizes);
        let events = match decoded {
            Ok(events) => events,
            Err(error) => vec![Event::Unknown {
                packet_id,
                command_set: set,
                command,
                payload: bytes.to_vec(),
                diagnostic: error.to_string(),
            }],
        };

        Self {
            received_at_ms,
            suspend_policy: policy,
            events,
        }
    }

    fn parse(set: u8, command: u8, bytes: &[u8], sizes: IdSizes) -> anyhow::Result<Vec<Event>> {
        ensure!((set, command) == (64, 100), "unsupported target command");

        let mut reader = Reader::new(bytes);
        let policy = reader.read_u8()?;

        ensure!(policy <= 2, "invalid suspend policy");

        let count = reader.read_count()?;
        let mut events = Vec::new();

        for _ in 0..count {
            let kind = reader.read_u8()?;
            let request_id = reader.read_u32()?;
            let event = match kind {
                1 | 2 => {
                    let thread = wire::id(reader.read_id(sizes.object)?);
                    let location = Location::read(&mut reader, sizes)?;

                    if kind == 1 {
                        Event::StepCompleted {
                            request_id,
                            thread,
                            location,
                        }
                    } else {
                        Event::Breakpoint {
                            request_id,
                            thread,
                            location,
                        }
                    }
                }
                4 => {
                    let thread = wire::id(reader.read_id(sizes.object)?);
                    let location = Location::read(&mut reader, sizes)?;
                    let exception = read_object(&mut reader, sizes)?;
                    let catch = JdwpLocation::read(&mut reader, sizes)?;
                    let catch_location = if catch.type_tag == 0 {
                        ensure!(
                            catch.type_id == 0 && catch.method_id == 0 && catch.code_index == 0,
                            "invalid null catch location"
                        );

                        None
                    } else {
                        Some(catch.into())
                    };

                    Event::Exception {
                        request_id,
                        thread,
                        location,
                        exception,
                        catch_location,
                    }
                }
                20 | 21 => {
                    let field = FieldEvent {
                        thread: wire::id(reader.read_id(sizes.object)?),
                        location: Location::read(&mut reader, sizes)?,
                        type_tag: reader.read_u8()?,
                        type_id: wire::id(reader.read_id(sizes.reference_type)?),
                        field_id: wire::id(reader.read_id(sizes.field)?),
                        object: read_object(&mut reader, sizes)?,
                    };

                    if kind == 20 {
                        Event::FieldAccess { request_id, field }
                    } else {
                        let value = ProtocolValue::decode(&mut reader, sizes)?;

                        ensure!(
                            !matches!(value, ProtocolValue::Void),
                            "field modification cannot have a void value"
                        );

                        let value_to_be = value.into();

                        Event::FieldModification {
                            request_id,
                            field,
                            value_to_be,
                        }
                    }
                }
                6 => Event::ThreadStarted {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                7 => Event::ThreadDied {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                8 => Event::ClassPrepared {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                    type_tag: reader.read_u8()?,
                    type_id: wire::id(reader.read_id(sizes.reference_type)?),
                    signature: reader.read_string()?,
                    status: reader.read_u32()?,
                },
                9 => Event::ClassUnloaded {
                    request_id,
                    signature: reader.read_string()?,
                },
                90 => Event::VmStarted {
                    request_id,
                    thread: wire::id(reader.read_id(sizes.object)?),
                },
                99 => Event::VmDied { request_id },
                _ => bail!("unsupported event kind {kind}; entire Composite retained"),
            };

            events.push(event);
        }

        reader.finish()?;

        Ok(events)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    Events { batch: EventBatch },
    Dropped { packets: u64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Delivery {
    pub sequence: u64,
    pub payload: Payload,
}

impl From<JdwpLocation> for Location {
    fn from(location: JdwpLocation) -> Self {
        Self {
            type_tag: location.type_tag,
            type_id: wire::id(location.type_id),
            method_id: wire::id(location.method_id),
            code_index: wire::id(location.code_index),
        }
    }
}

fn read_object(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Value> {
    let value = ProtocolValue::decode(reader, sizes)?;

    ensure!(
        matches!(value, ProtocolValue::Object { .. }),
        "expected tagged object ID"
    );

    Ok(value.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_events_decode_mixed_batches_and_preserve_nulls() {
        use crate::wire::Writer;
        let sizes = IdSizes {
            field: 2,
            method: 4,
            object: 4,
            reference_type: 2,
            frame: 8,
        };
        let mut writer = Writer::default();
        writer.write_u8(1);
        writer.write_i32(5);
        let location = JdwpLocation {
            type_tag: 1,
            type_id: 0x101,
            method_id: 0x82,
            code_index: 0x19,
        };
        for caught in [false, true] {
            writer.write_u8(4);
            writer.write_u32(12);
            writer.write_id(0x12, sizes.object).unwrap();
            location.write(&mut writer, sizes).unwrap();
            writer.write_u8(b'L');
            writer.write_id(0x55, sizes.object).unwrap();
            if caught {
                location.write(&mut writer, sizes).unwrap();
            } else {
                writer.write_bytes(&[0; 15]);
            }
        }
        for kind in [20, 21] {
            writer.write_u8(kind);
            writer.write_u32(13);
            writer.write_id(0x12, sizes.object).unwrap();
            location.write(&mut writer, sizes).unwrap();
            writer.write_u8(1);
            writer.write_id(0x101, sizes.reference_type).unwrap();
            writer.write_id(0x45, sizes.field).unwrap();
            writer.write_u8(b'L');
            writer
                .write_id(if kind == 20 { 0 } else { 0x55 }, sizes.object)
                .unwrap();
            if kind == 21 {
                writer.write_u8(b'J');
                writer.write_i64(-7);
            }
        }
        writer.write_u8(8);
        writer.write_u32(14);
        writer.write_id(0, sizes.object).unwrap();
        writer.write_u8(1);
        writer.write_id(0x101, sizes.reference_type).unwrap();
        writer.write_string("Ltest;").unwrap();
        writer.write_u32(2);
        let bytes = writer.into_bytes();
        let batch = EventBatch::decode(1, 64, 100, &bytes, sizes);
        assert_eq!(batch.events.len(), 5);
        assert!(matches!(
            &batch.events[0],
            Event::Exception {
                catch_location: None,
                ..
            }
        ));
        assert!(matches!(
            &batch.events[1],
            Event::Exception {
                catch_location: Some(_),
                ..
            }
        ));
        let json = serde_json::to_value(&batch).unwrap();
        assert_eq!(json["events"][2]["object"]["value"], "null");
        assert_eq!(json["events"][3]["field_id"], "0x45");
        assert_eq!(json["events"][3]["value_to_be"]["value"], "-7");
        let encoded = rmp_serde::to_vec_named(&batch).unwrap();
        let roundtrip: EventBatch = rmp_serde::from_slice(&encoded).unwrap();
        assert_eq!(serde_json::to_value(roundtrip).unwrap(), json);
        for (offset, replacement) in [(29, b'I'), (36, 1)] {
            let mut malformed = bytes.clone();
            malformed[offset] = replacement;
            let decoded = EventBatch::decode(1, 64, 100, &malformed, sizes);
            assert!(matches!(&decoded.events[0], Event::Unknown { .. }));
        }
        let truncated = EventBatch::decode(1, 64, 100, &bytes[..bytes.len() - 1], sizes);
        assert!(
            matches!(&truncated.events[0], Event::Unknown { payload, .. } if payload == &bytes[..bytes.len() - 1])
        );
    }

    #[test]
    fn unknown_composite_keeps_entire_payload() {
        let payload = [2, 0, 0, 0, 1, 255, 0, 0, 0, 1];
        let sizes = IdSizes {
            field: 8,
            method: 8,
            object: 8,
            reference_type: 8,
            frame: 8,
        };
        let batch = EventBatch::decode(1, 64, 100, &payload, sizes);
        assert_eq!(batch.suspend_policy, SuspendPolicy::All);
        assert!(matches!(&batch.events[0], Event::Unknown { payload: raw, .. } if raw == &payload));
    }
}

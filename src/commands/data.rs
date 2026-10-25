use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::events::Location;
use crate::jdwp::ddm::Chunk;
use crate::jdwp::ddm::hierarchy::Hierarchy;
use crate::jdwp::value::Value as ProtocolValue;
use crate::wire::{self, IdSizes};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Value {
    pub tag: u8,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TypeInfo {
    pub type_tag: u8,
    pub type_id: String,
    pub signature: String,
    pub status: u32,
    pub class_loader_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub signature: String,
    pub modifiers: u32,
    pub declaring_type_id: String,
    pub declaring_class: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FieldValue {
    pub field: Member,
    pub value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ThreadInfo {
    pub thread: String,
    pub name: String,
    pub status: u32,
    pub suspend_status: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrameInfo {
    pub frame: String,
    pub thread: String,
    pub location: Location,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Data {
    DdmFeatures {
        features: Vec<String>,
    },
    DdmRaw {
        chunks: Vec<Chunk>,
    },
    Windows {
        windows: Vec<String>,
    },
    ViewHierarchy {
        window: String,
        hierarchy: Hierarchy,
    },
    FileSaved {
        path: PathBuf,
        bytes: usize,
    },
    Attached {
        endpoint: String,
    },
    Status {
        endpoint: String,
        queued_wire_bytes: usize,
        dropped_packets: u64,
        idsizes: IdSizes,
    },
    Confirmed {
        operation: String,
    },
    Types {
        types: Vec<TypeInfo>,
    },
    Members {
        members: Vec<Member>,
    },
    Objects {
        objects: Vec<Value>,
    },
    Fields {
        fields: Vec<FieldValue>,
    },
    Threads {
        threads: Vec<ThreadInfo>,
    },
    Frames {
        frames: Vec<FrameInfo>,
    },
    Registers {
        registers: Vec<(String, Value)>,
    },
    EventRequest {
        event_request_id: u32,
        operation: String,
    },
    Idsizes {
        sizes: IdSizes,
    },
    Raw {
        jdwp_request_id: u32,
        error_code: u16,
        #[serde(with = "wire::byte_payload")]
        payload: Vec<u8>,
    },
    Sessions {
        sessions: Vec<String>,
    },
}

impl From<ProtocolValue> for Value {
    fn from(value: ProtocolValue) -> Self {
        let tag = value.tag();
        let value = match value {
            ProtocolValue::Boolean(value) => value.to_string(),
            ProtocolValue::Byte(value) => value.to_string(),
            ProtocolValue::Char(value) => value.to_string(),
            ProtocolValue::Short(value) => value.to_string(),
            ProtocolValue::Int(value) => value.to_string(),
            ProtocolValue::Long(value) => value.to_string(),
            ProtocolValue::Float(value) => value.to_string(),
            ProtocolValue::Double(value) => value.to_string(),
            ProtocolValue::Void => "void".into(),
            ProtocolValue::Object { id, .. } => {
                if id == 0 {
                    "null".into()
                } else {
                    wire::id(id)
                }
            }
        };

        Self { tag, value }
    }
}

use std::collections::BTreeMap;

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::wire::Reader;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Property {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    Text(String),
    Map(BTreeMap<String, Property>),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Node {
    pub view: String,
    pub parent: Option<usize>,
    pub properties: BTreeMap<String, Property>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Hierarchy {
    pub properties: BTreeMap<String, Property>,
    pub nodes: Vec<Node>,
}

pub fn decode(bytes: &[u8]) -> anyhow::Result<Hierarchy> {
    let mut reader = Reader::new(bytes);
    let mut properties = BTreeMap::new();
    let root = loop {
        let tag = reader.read_u8()?;

        if tag == b'M' {
            break read_map(&mut reader, 0)?;
        }

        ensure!(tag == b'S', "invalid V2 window property key");

        let key = reader.read_u16()?.to_string();
        let value = read_value(&mut reader, 0)?;

        ensure!(
            properties.insert(key, value).is_none(),
            "duplicate V2 window property"
        );
    };

    ensure!(reader.read_u8()? == b'M', "missing V2 property index");

    let index = read_map(&mut reader, 0)?;

    reader.finish()?;
    resolve(&mut properties, &index)?;

    let mut hierarchy = Hierarchy {
        properties,
        nodes: Vec::new(),
    };
    let mut root = root;

    resolve(&mut root, &index)?;
    append_node(root, None, &mut hierarchy.nodes)?;

    Ok(hierarchy)
}

fn read_map(reader: &mut Reader<'_>, depth: usize) -> anyhow::Result<BTreeMap<String, Property>> {
    ensure!(depth <= 128, "V2 hierarchy exceeds 128 nested maps");

    let mut properties = BTreeMap::new();

    loop {
        ensure!(reader.read_u8()? == b'S', "invalid V2 property key");

        let key = reader.read_u16()?;

        if key == 0 {
            return Ok(properties);
        }

        let value = read_value(reader, depth + 1)?;

        // The property index repeats its marker key; later entries take precedence.
        properties.insert(key.to_string(), value);
    }
}

fn read_value(reader: &mut Reader<'_>, depth: usize) -> anyhow::Result<Property> {
    Ok(match reader.read_u8()? {
        b'Z' => {
            let value = reader.read_u8()?;

            ensure!(value <= 1, "invalid V2 boolean");
            Property::Boolean(value != 0)
        }
        b'B' => Property::Integer(reader.read_i8()? as i64),
        b'S' => Property::Integer(reader.read_i16()? as i64),
        b'I' => Property::Integer(reader.read_i32()? as i64),
        b'J' => Property::Integer(reader.read_i64()?),
        b'F' => float(f32::from_bits(reader.read_u32()?) as f64),
        b'D' => float(f64::from_bits(reader.read_u64()?)),
        b'R' => {
            let length = reader.read_u16()? as usize;
            let bytes = reader.read_bytes(length)?;

            // Android truncates UTF-8 properties by bytes, possibly inside a code point.
            Property::Text(String::from_utf8_lossy(bytes).into_owned())
        }
        b'M' => Property::Map(read_map(reader, depth)?),
        tag => bail!("unsupported V2 property tag {tag:#x}"),
    })
}

fn float(value: f64) -> Property {
    if value.is_finite() {
        Property::Float(value)
    } else {
        Property::Text(value.to_string())
    }
}

fn resolve(
    properties: &mut BTreeMap<String, Property>,
    index: &BTreeMap<String, Property>,
) -> anyhow::Result<()> {
    let encoded = std::mem::take(properties);

    for (key, mut value) in encoded {
        let Some(Property::Text(name)) = index.get(&key) else {
            bail!("missing V2 property name for index {key}");
        };

        if let Property::Map(children) = &mut value {
            resolve(children, index)?;
        }

        ensure!(
            properties.insert(name.clone(), value).is_none(),
            "duplicate V2 property name {name}"
        );
    }

    Ok(())
}

fn append_node(
    mut properties: BTreeMap<String, Property>,
    parent: Option<usize>,
    nodes: &mut Vec<Node>,
) -> anyhow::Result<()> {
    let Some(Property::Text(class)) = properties.get("meta:__name__") else {
        bail!("V2 node has no class name");
    };
    let Some(Property::Integer(hash)) = properties.get("meta:__hash__") else {
        bail!("V2 node has no hash code");
    };
    let view = format!("{class}@{:x}", i32::try_from(*hash)? as u32);
    let count = match properties.get("meta:__childCount__") {
        Some(Property::Integer(count)) => usize::try_from(*count)?,
        None => 0,
        _ => bail!("invalid V2 child count"),
    };

    ensure!(count <= properties.len(), "invalid V2 child count");

    let mut children = Vec::new();

    for child in 0..count {
        let value = properties
            .remove(&format!("meta:__child__{child}"))
            .context("missing V2 child")?;
        let Property::Map(child) = value else {
            bail!("V2 child is not a map");
        };

        children.push(child);
    }

    let position = nodes.len();

    nodes.push(Node {
        view,
        parent,
        properties,
    });

    for child in children {
        append_node(child, Some(position), nodes)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_resolves_index_children_signed_hash_and_nested_properties() {
        let bytes = include_bytes!("../../../tests/fixtures/view-v2.bin");
        let tree = decode(bytes).unwrap();
        assert_eq!(tree.nodes.len(), 2);
        assert_eq!(tree.nodes[0].view, "android.widget.FrameLayout@123");
        assert_eq!(tree.nodes[0].parent, None);
        assert_eq!(tree.nodes[1].view, "android.widget.TextView@ffffffff");
        assert_eq!(tree.nodes[1].parent, Some(0));
        assert!(
            matches!(&tree.nodes[1].properties["text:text"], Property::Text(value) if value == "你好")
        );
        assert!(matches!(
            &tree.properties["window:left"],
            Property::Integer(3)
        ));
        assert!(
            matches!(&tree.nodes[0].properties["layoutParams"], Property::Map(values) if matches!(values["width"], Property::Integer(100)))
        );
        for length in 0..bytes.len() {
            assert!(
                decode(&bytes[..length]).is_err(),
                "accepted truncated tree at {length}"
            );
        }
        let mut extra = bytes.to_vec();
        extra.push(0);
        assert!(decode(&extra).is_err());
    }

    #[test]
    fn v2_rejects_invalid_types_and_excessive_nesting() {
        assert!(read_value(&mut Reader::new(b"Z\x02"), 0).is_err());
        assert!(read_value(&mut Reader::new(b"?"), 0).is_err());
        let mut nested = Vec::new();
        for _ in 0..130 {
            nested.extend_from_slice(b"MS\0\x01");
        }
        assert!(read_value(&mut Reader::new(&nested), 0).is_err());
        assert!(
            matches!(read_value(&mut Reader::new(b"R\0\x01\xe4"), 0).unwrap(), Property::Text(value) if value == "�")
        );
        assert!(matches!(float(f64::NAN), Property::Text(_)));
    }
}

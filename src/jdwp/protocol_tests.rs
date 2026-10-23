use crate::wire::{self, IdSizes, Reader, Writer};

use super::event_request::{EventKind, Modifier, StepDepth, StepSize, SuspendPolicy};
use super::location::Location;
use super::stack_frame::Slot;
use super::value::Value;
use super::{Command, Response};
use super::{
    class_type, event_request, object_reference, reference_type, stack_frame, thread_reference,
    virtual_machine,
};

const SIZES: IdSizes = IdSizes {
    field: 2,
    method: 4,
    object: 4,
    reference_type: 2,
    frame: 8,
};

fn encoded(request: impl Command) -> Vec<u8> {
    let mut writer = Writer::default();
    request.encode_request(&mut writer, SIZES).unwrap();
    writer.into_bytes()
}

fn decoded<T: Command>(request: T, bytes: &[u8]) -> anyhow::Result<T::Response> {
    let mut reader = Reader::new(bytes);
    let response = request.decode_response(&mut reader, SIZES)?;
    reader.finish()?;
    Ok(response)
}

#[test]
fn request_bytes_preserve_id_widths_and_value_tag_rules() {
    let get = stack_frame::GetValues {
        thread: 0x12,
        frame: 0x22,
        slots: vec![Slot {
            index: 2,
            tag: b'L',
        }],
    };
    assert_eq!(
        encoded(get),
        wire::unhex(
            "00000012 0000000000000022 00000001 00000002 4c"
                .replace(' ', "")
                .as_str()
        )
        .unwrap()
    );
    let set = stack_frame::SetValues {
        thread: 0x12,
        frame: 0x22,
        slots: vec![(2, Value::Int(-7))],
    };
    assert_eq!(
        encoded(set),
        wire::unhex("000000120000000000000022000000010000000249fffffff9").unwrap()
    );
    let set = object_reference::SetValues {
        object: 0x12,
        fields: vec![(0x33, Value::Int(-7))],
    };
    assert_eq!(
        encoded(set),
        wire::unhex("00000012000000010033fffffff9").unwrap()
    );
    let set = class_type::SetValues {
        class: 0x12,
        fields: vec![(
            0x33,
            Value::Object {
                tag: b'L',
                id: 0x44,
            },
        )],
    };
    assert_eq!(
        encoded(set),
        wire::unhex("001200000001003300000044").unwrap()
    );
    let frames = thread_reference::Frames {
        thread: 0x12,
        start: 0,
        length: -1,
    };
    assert_eq!(
        encoded(frames),
        wire::unhex("0000001200000000ffffffff").unwrap()
    );
    assert!(
        stack_frame::GetValues {
            thread: u64::MAX,
            frame: 0,
            slots: vec![]
        }
        .encode_request(&mut Writer::default(), SIZES)
        .is_err()
    );
}

#[test]
fn event_modifiers_match_breakpoint_and_step_wire_format() {
    let breakpoint = event_request::Set {
        kind: EventKind::Breakpoint,
        suspend_policy: SuspendPolicy::EventThread,
        modifiers: vec![Modifier::LocationOnly(Location {
            type_tag: 1,
            type_id: 0x101,
            method_id: 0x82,
            code_index: 0x19,
        })],
    };
    assert_eq!(
        encoded(breakpoint),
        wire::unhex("02010000000107010101000000820000000000000019").unwrap()
    );
    for (depth, code) in [
        (StepDepth::Into, 0),
        (StepDepth::Over, 1),
        (StepDepth::Out, 2),
    ] {
        let step = event_request::Set {
            kind: EventKind::SingleStep,
            suspend_policy: SuspendPolicy::EventThread,
            modifiers: vec![
                Modifier::Step {
                    thread: 0x12,
                    size: StepSize::Min,
                    depth,
                },
                Modifier::Count(1),
            ],
        };
        let mut expected = wire::unhex("0101000000020a0000001200000000000000000100000001").unwrap();
        expected[18] = code;
        assert_eq!(encoded(step), expected);
    }
    assert_eq!(
        encoded(event_request::Clear {
            kind: EventKind::Breakpoint,
            request_id: 77
        }),
        vec![2, 0, 0, 0, 77]
    );
}

#[test]
fn replies_keep_numeric_ids_and_reject_bad_payloads() {
    let classes = decoded(
        virtual_machine::AllClasses,
        &wire::unhex("00000001010101000000034c543b00000007").unwrap(),
    )
    .unwrap();
    assert_eq!(classes[0].type_id, 0x101);
    assert_eq!(classes[0].signature, "LT;");
    assert_eq!(classes[0].status, 7);
    let frame_bytes =
        wire::unhex("000000010000000000000022010101000000820000000000000019").unwrap();
    let frames = decoded(
        thread_reference::Frames {
            thread: 0x12,
            start: 0,
            length: -1,
        },
        &frame_bytes,
    )
    .unwrap();
    assert_eq!(frames[0].id, 0x22);
    assert_eq!(frames[0].location.type_id, 0x101);
    assert_eq!(frames[0].location.method_id, 0x82);
    assert_eq!(frames[0].location.code_index, 0x19);
    assert!(decoded(virtual_machine::AllClasses, &[0, 0, 0, 1]).is_err());
    assert!(decoded(virtual_machine::AllThreads, &[0, 0, 0, 0, 1]).is_err());
    assert!(decoded(virtual_machine::Dispose, &[1]).is_err());
    assert!(
        decoded(
            reference_type::GetValues {
                type_id: 1,
                fields: vec![2]
            },
            &[0, 0, 0, 0]
        )
        .is_err()
    );
    assert!(
        decoded(
            stack_frame::GetValues {
                thread: 1,
                frame: 2,
                slots: vec![Slot {
                    index: 0,
                    tag: b'I'
                }]
            },
            &[0, 0, 0, 1, b'I', 0]
        )
        .is_err()
    );
    assert!(
        decoded(
            object_reference::GetValues {
                object: 1,
                fields: vec![2]
            },
            &[0, 0, 0, 1, b'?']
        )
        .is_err()
    );
}

#[test]
fn primitive_values_preserve_encoding_and_display() {
    for (tag, text, hex) in [
        (b'Z', "true", "01"),
        (b'B', "-1", "ff"),
        (b'C', "65535", "ffff"),
        (b'S', "-2", "fffe"),
        (b'I', "-7", "fffffff9"),
        (b'J', "-9223372036854775808", "8000000000000000"),
        (b'F', "1.5", "3fc00000"),
        (b'D', "-2.5", "c004000000000000"),
        (b'L', "0x1234", "00001234"),
        (b'L', "null", "00000000"),
    ] {
        let value = Value::parse(tag, text).unwrap();
        let mut writer = Writer::default();
        value.write_untagged(&mut writer, SIZES).unwrap();
        let bytes = writer.into_bytes();
        assert_eq!(wire::hex(&bytes), hex);
        let mut tagged = vec![tag];
        tagged.extend(bytes);
        let value = Value::decode(&mut Reader::new(&tagged), SIZES).unwrap();
        let output = crate::commands::Data::Objects {
            objects: vec![value.into()],
        };
        let output = serde_json::to_value(output).unwrap();
        assert_eq!(output["objects"][0]["tag"], tag);
        assert_eq!(output["objects"][0]["value"], text);
    }
    for (tag, text) in [(b'B', "128"), (b'Z', "1"), (b'I', "2147483648")] {
        assert!(Value::parse(tag, text).is_err());
    }
}

#[test]
fn response_types_select_their_own_wire_layout() {
    let object = decoded(
        reference_type::ClassLoader { type_id: 1 },
        &[0, 0, 0x12, 0x34],
    )
    .unwrap();
    assert_eq!(object.0, 0x1234);
    let parent = decoded(class_type::Superclass { class: 1 }, &[0x12, 0x34]).unwrap();
    assert_eq!(parent.0, 0x1234);
    let field_bytes = wire::unhex("0000000100330000000166000000014900000008").unwrap();
    let fields = decoded(reference_type::Fields { type_id: 1 }, &field_bytes).unwrap();
    assert_eq!(fields.0[0].id, 0x33);
    assert_eq!(fields.0[0].name, "f");
    let method_bytes = wire::unhex("00000001000000330000000166000000014900000008").unwrap();
    let methods = decoded(reference_type::Methods { type_id: 1 }, &method_bytes).unwrap();
    assert_eq!(methods.0[0].id, 0x33);
    assert_eq!(methods.0[0].modifiers, 8);
    assert!(encoded(virtual_machine::Dispose).is_empty());
    assert!(decoded(virtual_machine::Dispose, &[]).is_ok());
    assert!(decoded(virtual_machine::Dispose, &[0]).is_err());
}

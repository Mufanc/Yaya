use crate::commands::Data;
use crate::wire::{self, IdSizes, Reader, Writer};

use super::class_type::{SetValues as SetClassValues, Superclass};
use super::event_request::{
    Clear as ClearEvent, EventKind, Modifier, Set as SetEvent, StepDepth, StepSize, SuspendPolicy,
};
use super::location::Location;
use super::object_reference::{GetValues as GetObjectValues, SetValues as SetObjectValues};
use super::reference_type::{ClassLoader, Fields, GetValues as GetStaticValues, Methods};
use super::stack_frame::{GetValues as GetFrameValues, SetValues as SetFrameValues, Slot};
use super::thread_reference::Frames;
use super::value::Value;
use super::virtual_machine::{AllClasses, AllThreads, Dispose};
use super::{Command, Response};

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
    let get = GetFrameValues {
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
    let set = SetFrameValues {
        thread: 0x12,
        frame: 0x22,
        slots: vec![(2, Value::Int(-7))],
    };
    assert_eq!(
        encoded(set),
        wire::unhex("000000120000000000000022000000010000000249fffffff9").unwrap()
    );
    let set = SetObjectValues {
        object: 0x12,
        fields: vec![(0x33, Value::Int(-7))],
    };
    assert_eq!(
        encoded(set),
        wire::unhex("00000012000000010033fffffff9").unwrap()
    );
    let set = SetClassValues {
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
    let frames = Frames {
        thread: 0x12,
        start: 0,
        length: -1,
    };
    assert_eq!(
        encoded(frames),
        wire::unhex("0000001200000000ffffffff").unwrap()
    );
    assert!(
        GetFrameValues {
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
    let breakpoint = SetEvent {
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
        let step = SetEvent {
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
        encoded(ClearEvent {
            kind: EventKind::Breakpoint,
            request_id: 77
        }),
        vec![2, 0, 0, 0, 77]
    );
}

#[test]
fn replies_keep_numeric_ids_and_reject_bad_payloads() {
    let classes = decoded(
        AllClasses,
        &wire::unhex("00000001010101000000034c543b00000007").unwrap(),
    )
    .unwrap();
    assert_eq!(classes[0].type_id, 0x101);
    assert_eq!(classes[0].signature, "LT;");
    assert_eq!(classes[0].status, 7);
    let frame_bytes =
        wire::unhex("000000010000000000000022010101000000820000000000000019").unwrap();
    let frames = decoded(
        Frames {
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
    assert!(decoded(AllClasses, &[0, 0, 0, 1]).is_err());
    assert!(decoded(AllThreads, &[0, 0, 0, 0, 1]).is_err());
    assert!(decoded(Dispose, &[1]).is_err());
    assert!(
        decoded(
            GetStaticValues {
                type_id: 1,
                fields: vec![2]
            },
            &[0, 0, 0, 0]
        )
        .is_err()
    );
    assert!(
        decoded(
            GetFrameValues {
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
            GetObjectValues {
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
        let output = Data::Objects {
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
    let object = decoded(ClassLoader { type_id: 1 }, &[0, 0, 0x12, 0x34]).unwrap();
    assert_eq!(object.0, 0x1234);
    let parent = decoded(Superclass { class: 1 }, &[0x12, 0x34]).unwrap();
    assert_eq!(parent.0, 0x1234);
    let field_bytes = wire::unhex("0000000100330000000166000000014900000008").unwrap();
    let fields = decoded(Fields { type_id: 1 }, &field_bytes).unwrap();
    assert_eq!(fields.0[0].id, 0x33);
    assert_eq!(fields.0[0].name, "f");
    let method_bytes = wire::unhex("00000001000000330000000166000000014900000008").unwrap();
    let methods = decoded(Methods { type_id: 1 }, &method_bytes).unwrap();
    assert_eq!(methods.0[0].id, 0x33);
    assert_eq!(methods.0[0].modifiers, 8);
    assert!(encoded(Dispose).is_empty());
    assert!(decoded(Dispose, &[]).is_ok());
    assert!(decoded(Dispose, &[0]).is_err());
}

#[test]
fn primitive_inspection_request_layouts() {
    use super::array_reference::{GetValues as GetArray, Length, SetValues as SetArray};
    use super::object_reference::ReferringObjects;
    use super::stack_frame::ThisObject;
    use super::string_reference::Value as StringValue;
    use super::thread_reference::{Suspend, SuspendCount};
    use super::virtual_machine::Suspend as SuspendVm;

    assert_eq!((SuspendVm::COMMAND_SET, SuspendVm::COMMAND), (1, 8));
    assert!(encoded(SuspendVm).is_empty());
    assert_eq!((Suspend::COMMAND_SET, Suspend::COMMAND), (11, 2));
    assert_eq!(encoded(Suspend { thread: 0x12 }), [0, 0, 0, 0x12]);
    assert_eq!((SuspendCount::COMMAND_SET, SuspendCount::COMMAND), (11, 12));
    assert_eq!(
        decoded(SuspendCount { thread: 0x12 }, &[0, 0, 0, 2]).unwrap(),
        2
    );
    assert_eq!((StringValue::COMMAND_SET, StringValue::COMMAND), (10, 1));
    assert_eq!(encoded(StringValue { string: 0x34 }), [0, 0, 0, 0x34]);
    let mut writer = Writer::default();
    writer.write_string("你好\0🎵").unwrap();
    assert_eq!(
        decoded(StringValue { string: 0x34 }, &writer.into_bytes()).unwrap(),
        "你好\0🎵"
    );
    assert_eq!((ThisObject::COMMAND_SET, ThisObject::COMMAND), (16, 3));
    assert_eq!(
        encoded(ThisObject {
            thread: 0x12,
            frame: 0x34
        }),
        wire::unhex("000000120000000000000034").unwrap()
    );
    assert!(matches!(
        decoded(
            ThisObject {
                thread: 0x12,
                frame: 0x34
            },
            b"L\0\0\0\0"
        )
        .unwrap(),
        Value::Object { id: 0, .. }
    ));
    assert_eq!(
        (ReferringObjects::COMMAND_SET, ReferringObjects::COMMAND),
        (9, 10)
    );
    assert_eq!(
        encoded(ReferringObjects {
            object: 0x34,
            limit: 0
        }),
        wire::unhex("0000003400000000").unwrap()
    );
    assert_eq!((Length::COMMAND_SET, Length::COMMAND), (13, 1));
    assert_eq!(encoded(Length { array: 0x34 }), [0, 0, 0, 0x34]);
    assert_eq!(
        encoded(GetArray {
            array: 0x34,
            start: 1,
            length: 2
        }),
        wire::unhex("000000340000000100000002").unwrap()
    );
    assert_eq!((SetArray::COMMAND_SET, SetArray::COMMAND), (13, 3));
    assert_eq!(
        encoded(SetArray {
            array: 0x34,
            start: 1,
            values: vec![Value::Int(-7), Value::Int(42)]
        }),
        wire::unhex("000000340000000100000002fffffff90000002a").unwrap()
    );
    assert_eq!(
        encoded(SetArray {
            array: 0x34,
            start: 0,
            values: vec![Value::Object { tag: b'L', id: 0 }]
        }),
        wire::unhex("00000034000000000000000100000000").unwrap()
    );
    assert!(
        GetArray {
            array: 0,
            start: u32::MAX,
            length: 0
        }
        .encode_request(&mut Writer::default(), SIZES)
        .is_err()
    );
    assert!(
        ReferringObjects {
            object: 0,
            limit: u32::MAX
        }
        .encode_request(&mut Writer::default(), SIZES)
        .is_err()
    );
}

#[test]
fn array_regions_distinguish_primitive_and_tagged_object_elements() {
    use super::array_reference::GetValues as GetArray;
    let request = || GetArray {
        array: 1,
        start: 0,
        length: 2,
    };
    let primitive = decoded(
        request(),
        &wire::unhex("4900000002fffffff90000002a").unwrap(),
    )
    .unwrap();
    assert_eq!(primitive.tag, b'I');
    assert!(matches!(
        primitive.values.as_slice(),
        [Value::Int(-7), Value::Int(42)]
    ));
    let objects = decoded(
        request(),
        &wire::unhex("4c0000000273000000125b00000000").unwrap(),
    )
    .unwrap();
    assert!(matches!(
        objects.values.as_slice(),
        [
            Value::Object {
                tag: b's',
                id: 0x12
            },
            Value::Object { tag: b'[', id: 0 }
        ]
    ));
    assert!(
        decoded(
            GetArray {
                array: 1,
                start: 0,
                length: 0
            },
            b"I\0\0\0\0"
        )
        .unwrap()
        .values
        .is_empty()
    );
    for invalid in [
        "490000000200",
        "490000000100000001",
        "5600000000",
        "4c0000000249000000014900000002",
        "49ffffffff",
    ] {
        assert!(
            decoded(request(), &wire::unhex(invalid).unwrap()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn watch_filters_preserve_wire_order_and_id_widths() {
    let request = SetEvent {
        kind: EventKind::Exception,
        suspend_policy: SuspendPolicy::EventThread,
        modifiers: vec![
            Modifier::ExceptionOnly {
                exception: 0x123,
                caught: true,
                uncaught: false,
            },
            Modifier::ThreadOnly(0x456),
            Modifier::Count(3),
        ],
    };
    assert_eq!(
        encoded(request),
        wire::unhex("040100000003080123010003000004560100000003").unwrap()
    );
    assert_eq!(
        encoded(SetEvent {
            kind: EventKind::ClassPrepare,
            suspend_policy: SuspendPolicy::None,
            modifiers: vec![Modifier::ClassMatch("a.*".into())],
        }),
        wire::unhex("0800000000010500000003612e2a").unwrap()
    );
    for kind in [EventKind::FieldAccess, EventKind::FieldModification] {
        let bytes = encoded(SetEvent {
            kind,
            suspend_policy: SuspendPolicy::All,
            modifiers: vec![Modifier::FieldOnly {
                declaring: 0x101,
                field: 0x45,
            }],
        });
        assert_eq!(bytes[0], kind as u8);
        assert_eq!(&bytes[1..], &wire::unhex("02000000010901010045").unwrap());
        assert_eq!(
            encoded(ClearEvent {
                kind,
                request_id: 77
            }),
            [kind as u8, 0, 0, 0, 77]
        );
    }
    for count in [0, u32::MAX] {
        let request = SetEvent {
            kind: EventKind::Exception,
            suspend_policy: SuspendPolicy::None,
            modifiers: vec![Modifier::Count(count)],
        };
        assert!(
            request
                .encode_request(&mut Writer::default(), SIZES)
                .is_err()
        );
    }
}

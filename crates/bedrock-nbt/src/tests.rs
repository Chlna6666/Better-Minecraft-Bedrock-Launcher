use super::*;
use crate::constants::*;
use indexmap::IndexMap;
use std::{borrow::Cow, ops::ControlFlow};

#[test]
fn actor_storage_key_preserves_non_utf8_tag_string_bytes() {
    let bytes = [
        TAG_COMPOUND,
        0,
        0,
        TAG_STRING,
        10,
        0,
        b'S',
        b't',
        b'o',
        b'r',
        b'a',
        b'g',
        b'e',
        b'K',
        b'e',
        b'y',
        8,
        0,
        0,
        0,
        1,
        0x27,
        0xff,
        0,
        0x12,
        0x21,
        TAG_END,
    ];
    let parsed = parse_root(&bytes).expect("parse actor StorageKey");
    let NbtTag::Compound(root) = &parsed else {
        panic!("actor root must be compound");
    };
    assert_eq!(
        root.get("StorageKey"),
        Some(&NbtTag::ByteString(vec![
            0, 0, 1, 0x27, 0xff, 0, 0x12, 0x21
        ]))
    );
    assert_eq!(serialize_root(&parsed).unwrap(), bytes);
}

#[test]
fn root_nbt_roundtrips_little_endian_tags() {
    let mut root = IndexMap::new();
    root.insert("Name".to_string(), NbtTag::String("World".to_string()));
    root.insert("Seed".to_string(), NbtTag::Long(-42));
    root.insert(
        "List".to_string(),
        NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)]),
    );
    let tag = NbtTag::Compound(root);

    let bytes = serialize_root(&tag).expect("serialize");
    let parsed = parse_root(&bytes).expect("parse");

    assert!(tags_equal_for_write(&tag, &parsed));
}

#[test]
fn mixed_list_is_rejected_before_write() {
    let mut root = IndexMap::new();
    root.insert(
        "Bad".to_string(),
        NbtTag::List(vec![NbtTag::Int(1), NbtTag::String("bad".to_string())]),
    );

    assert!(validate_root_for_write(&NbtTag::Compound(root)).is_err());
}

#[test]
fn root_parser_reports_consumed_bytes() {
    let mut root = IndexMap::new();
    root.insert("Name".to_string(), NbtTag::String("First".to_string()));
    let bytes = serialize_root(&NbtTag::Compound(root)).expect("serialize");
    let mut combined = bytes.clone();
    combined.extend_from_slice(&bytes);

    let (_, consumed) = parse_root_with_consumed(&combined).expect("parse");

    assert_eq!(consumed, bytes.len());
}

#[test]
fn nbt_view_emits_borrowed_events_without_owned_dom() {
    let mut root = IndexMap::new();
    root.insert("Name".to_string(), NbtTag::String("Borrowed".to_string()));
    root.insert("Seed".to_string(), NbtTag::Long(42));
    root.insert("Bytes".to_string(), NbtTag::ByteArray(vec![1, 2, 3]));
    let bytes = serialize_root(&NbtTag::Compound(root)).expect("serialize");

    let events = NbtReader::new(&bytes).view().events().expect("events");

    assert!(matches!(
        events.first(),
        Some(NbtEvent::BeginCompound { name: Some("") })
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        NbtEvent::String {
            name: Some("Name"),
            value: "Borrowed"
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        NbtEvent::Long {
            name: Some("Seed"),
            value: 42
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        NbtEvent::ByteArray {
            name: Some("Bytes"),
            bytes
        } if *bytes == [1, 2, 3]
    )));
}

#[test]
fn nbt_event_visitor_stops_without_materializing_remaining_events() {
    let root = NbtTag::Compound(IndexMap::from([
        ("Name".to_string(), NbtTag::String("Wanted".to_string())),
        ("Seed".to_string(), NbtTag::Long(42)),
    ]));
    let bytes = serialize_root(&root).expect("serialize");
    let mut visited = 0_usize;
    let mut found = None;

    NbtView::new(&bytes)
        .visit_events(|event| {
            visited = visited.saturating_add(1);
            if let NbtEvent::String {
                name: Some("Name"),
                value,
            } = event
            {
                found = Some(value);
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        })
        .expect("visit events");

    assert_eq!(found, Some("Wanted"));
    assert_eq!(visited, 2);
}

#[test]
fn root_ref_parser_borrows_string_values() {
    let bytes = serialize_root(&NbtTag::Compound(IndexMap::from([(
        "Name".to_string(),
        NbtTag::String("Borrowed".to_string()),
    )])))
    .expect("serialize");

    let parsed = NbtReader::new(&bytes).parse_root_ref().expect("parse ref");
    let NbtRef::Compound(entries) = parsed else {
        panic!("expected compound");
    };

    assert!(matches!(
        entries.first(),
        Some((
            Cow::Borrowed("Name"),
            NbtRef::String(Cow::Borrowed("Borrowed"))
        ))
    ));
}

#[test]
fn root_ref_parser_matches_owned_parser_for_all_tag_kinds() {
    let tag = NbtTag::Compound(IndexMap::from([
        ("Byte".to_string(), NbtTag::Byte(-1)),
        ("Short".to_string(), NbtTag::Short(-2)),
        ("Int".to_string(), NbtTag::Int(-3)),
        ("Long".to_string(), NbtTag::Long(-4)),
        ("Float".to_string(), NbtTag::Float(1.5)),
        ("Double".to_string(), NbtTag::Double(2.5)),
        ("Bytes".to_string(), NbtTag::ByteArray(vec![-1, 0, 1])),
        ("String".to_string(), NbtTag::String("value".to_string())),
        (
            "List".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)]),
        ),
        (
            "Compound".to_string(),
            NbtTag::Compound(IndexMap::from([(
                "Nested".to_string(),
                NbtTag::String("value".to_string()),
            )])),
        ),
        ("Ints".to_string(), NbtTag::IntArray(vec![-1, 0, 1])),
        ("Longs".to_string(), NbtTag::LongArray(vec![-1, 0, 1])),
        ("Shorts".to_string(), NbtTag::ShortArray(vec![-1, 0, 1])),
    ]));
    let bytes = serialize_root(&tag).expect("serialize");

    let owned = NbtReader::new(&bytes).parse_root().expect("parse owned");
    let borrowed = NbtReader::new(&bytes)
        .parse_root_ref()
        .expect("parse borrowed")
        .to_owned_tag();

    assert!(tags_equal_for_write(&owned, &borrowed));
}

#[test]
fn consecutive_root_parser_advances_between_compounds() {
    let mut root = IndexMap::new();
    root.insert("Name".to_string(), NbtTag::String("First".to_string()));
    let bytes = serialize_root(&NbtTag::Compound(root)).expect("serialize");
    let mut combined = bytes.clone();
    combined.extend_from_slice(&bytes);

    let tags = parse_consecutive_roots(&combined).expect("parse consecutive");

    assert_eq!(tags.len(), 2);
}

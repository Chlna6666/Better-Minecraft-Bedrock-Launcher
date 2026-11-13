use bedrock_nbt::{
    binary::{self, Encoding, NamedTag},
    *,
};
use indexmap::IndexMap;

fn compound(value: NbtTag) -> NbtTag {
    NbtTag::Compound(IndexMap::from([("v".into(), value)]))
}

#[test]
fn little_endian_and_network_match_independently_specified_bytes() {
    let root = NamedTag {
        name: "".into(),
        tag: compound(NbtTag::Int(-1)),
    };
    let disk = [10, 0, 0, 3, 1, 0, b'v', 255, 255, 255, 255, 0];
    let network = [10, 0, 3, 1, b'v', 1, 0];
    for (encoding, bytes) in [
        (Encoding::LittleEndian, disk.as_slice()),
        (Encoding::NetworkLittleEndian, network.as_slice()),
    ] {
        assert_eq!(binary::serialize(&root, encoding).unwrap(), bytes);
        assert_eq!(
            binary::parse(bytes, encoding).unwrap(),
            (root.clone(), bytes.len())
        );
    }
}

#[test]
fn network_string_lengths_are_unsigned_and_array_lengths_are_zigzag() {
    let string = [8, 0, 3, b'a', b'b', b'c'];
    assert_eq!(
        binary::parse(&string, Encoding::NetworkLittleEndian)
            .unwrap()
            .0
            .tag,
        NbtTag::String("abc".into())
    );
    let ints = [11, 0, 4, 1, 2]; // 2 elements, -1 and +1
    assert_eq!(
        binary::parse(&ints, Encoding::NetworkLittleEndian)
            .unwrap()
            .0
            .tag,
        NbtTag::IntArray(vec![-1, 1])
    );
    let root = NamedTag {
        name: "".into(),
        tag: NbtTag::IntArray(vec![-1, 1]),
    };
    assert_eq!(
        binary::serialize(&root, Encoding::NetworkLittleEndian).unwrap(),
        ints
    );
}

#[test]
fn encodings_roundtrip_named_roots_all_types_and_integer_extremes() {
    let values = vec![
        NbtTag::Byte(-128),
        NbtTag::Short(i16::MIN),
        NbtTag::Int(i32::MIN),
        NbtTag::Int(i32::MAX),
        NbtTag::Long(i64::MIN),
        NbtTag::Long(i64::MAX),
        NbtTag::Float(-0.0),
        NbtTag::Double(1.25),
        NbtTag::ByteArray(vec![-128, 0, 127]),
        NbtTag::ByteString(vec![0, 255, 128]),
        NbtTag::String("中文💎\0".into()),
        NbtTag::List(vec![NbtTag::Int(-1), NbtTag::Int(0)]),
        NbtTag::IntArray(vec![i32::MIN, -1, 0, i32::MAX]),
        NbtTag::LongArray(vec![i64::MIN, -1, 0, i64::MAX]),
        NbtTag::ShortArray(vec![i16::MIN, -1, 0, i16::MAX]),
        NbtTag::Compound(IndexMap::new()),
    ];
    for encoding in [Encoding::LittleEndian, Encoding::NetworkLittleEndian] {
        for tag in &values {
            let root = NamedTag {
                name: "命名根".into(),
                tag: tag.clone(),
            };
            let bytes = binary::serialize(&root, encoding).unwrap();
            assert_eq!(
                binary::parse(&bytes, encoding).unwrap(),
                (root, bytes.len())
            );
            for end in 0..bytes.len() {
                assert!(
                    binary::parse(&bytes[..end], encoding).is_err(),
                    "accepted prefix {end} of {tag:?}"
                );
            }
        }
    }
}

#[test]
fn short_array_extension_matches_literal_disk_bytes() {
    let bytes = [10, 0, 0, 100, 1, 0, b'v', 2, 0, 0, 0, 255, 255, 0, 128, 0];
    let tag = compound(NbtTag::ShortArray(vec![-1, i16::MIN]));
    assert_eq!(parse_root(&bytes).unwrap(), tag);
    assert_eq!(serialize_root(&tag).unwrap(), bytes);
    assert_eq!(
        NbtReader::new(&bytes)
            .parse_root_ref()
            .unwrap()
            .to_owned_tag(),
        tag
    );
}

#[test]
fn legacy_negative_and_zero_length_lists_are_empty_in_all_readers() {
    for len in [-1_i32, i32::MIN, 0] {
        for element_type in [0, 1, 8, 100, 255] {
            let mut bytes = vec![10, 0, 0, 9, 1, 0, b'v', element_type];
            bytes.extend_from_slice(&len.to_le_bytes());
            bytes.push(0);
            assert_eq!(parse_root(&bytes).unwrap(), compound(NbtTag::List(vec![])));
            assert_eq!(
                NbtReader::new(&bytes)
                    .parse_root_ref()
                    .unwrap()
                    .to_owned_tag(),
                parse_root(&bytes).unwrap()
            );
            assert!(NbtView::new(&bytes).events().is_ok());
        }
    }
}

#[test]
fn impossible_lengths_and_nonempty_end_lists_fail_before_allocation() {
    for kind in [7, 9, 11, 12, 100] {
        let mut bytes = vec![10, 0, 0, kind, 1, 0, b'v'];
        if kind == 9 {
            bytes.push(1);
        }
        bytes.extend_from_slice(&134_217_728_i32.to_le_bytes());
        bytes.push(0);
        assert!(parse_root(&bytes).is_err());
        assert!(NbtReader::new(&bytes).parse_root_ref().is_err());
        assert!(NbtView::new(&bytes).events().is_err());
    }
    let bytes = [10, 0, 0, 9, 1, 0, b'v', 0, 1, 0, 0, 0, 0];
    assert!(parse_root(&bytes).is_err());
    assert!(NbtReader::new(&bytes).parse_root_ref().is_err());
    assert!(NbtView::new(&bytes).events().is_err());
}

#[test]
fn overflowing_unterminated_and_truncated_varints_are_rejected() {
    for (kind, bytes) in [
        (3, vec![255, 255, 255, 255, 16]),
        (3, vec![128; 5]),
        (4, vec![255, 255, 255, 255, 255, 255, 255, 255, 255, 2]),
        (4, vec![128; 10]),
        (3, vec![128]),
    ] {
        let mut input = vec![kind, 0];
        input.extend(bytes);
        assert!(binary::parse(&input, Encoding::NetworkLittleEndian).is_err());
    }
}

#[test]
fn root_end_markers_and_compound_end_children_are_distinguished() {
    let end = NamedTag {
        name: "".into(),
        tag: NbtTag::End,
    };
    assert_eq!(
        binary::parse(&[0], Encoding::LittleEndian).unwrap(),
        (end.clone(), 1)
    );
    assert_eq!(
        binary::serialize(&end, Encoding::LittleEndian).unwrap(),
        [0]
    );
    assert!(parse_root(&[0]).is_err());
    assert!(serialize_root(&compound(NbtTag::End)).is_err());
    assert!(serialize_root(&compound(NbtTag::List(vec![NbtTag::End]))).is_err());
}

#[test]
fn all_readers_reject_excessive_nesting() {
    let mut bytes = vec![10, 0, 0];
    for _ in 0..130 {
        bytes.extend_from_slice(&[10, 0, 0]);
    }
    bytes.extend(std::iter::repeat_n(0, 131));
    assert!(parse_root(&bytes).is_err());
    assert!(NbtReader::new(&bytes).parse_root_ref().is_err());
    assert!(NbtView::new(&bytes).events().is_err());
}

#[cfg(feature = "parallel")]
#[test]
fn parallel_batches_preserve_order_and_individual_errors() {
    let tags: Vec<_> = (0..64).map(|n| compound(NbtTag::Int(n))).collect();
    let mut bytes: Vec<_> = tags
        .iter()
        .map(|tag| serialize_root(tag).unwrap())
        .collect();
    bytes[31] = vec![255];
    let inputs: Vec<_> = bytes.iter().map(Vec::as_slice).collect();
    for (index, result) in parallel::parse_roots(&inputs).into_iter().enumerate() {
        if index == 31 {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), tags[index]);
        }
    }
    let output = parallel::parse_snbt(&["1", "[I;1]", "broken!", "4L"]);
    assert_eq!(output[0].as_ref().unwrap(), &NbtTag::Int(1));
    assert!(output[2].is_err());
    assert_eq!(output[3].as_ref().unwrap(), &NbtTag::Long(4));
}

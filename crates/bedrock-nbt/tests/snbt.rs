use bedrock_nbt::{NbtTag, snbt, tags_equal_for_write};

#[test]
fn text_roundtrips_all_standard_tags_escapes_and_unicode() {
    let input = r#"{byte:-128b,short:-32768S,int:-2147483648,long:-9223372036854775808L,
        float:-0f,double:6.25e-2d,bytes:[B;-128b,0b,127b],ints:[I;-1,0,2147483647],
        longs:[L;-9223372036854775808L,0L,9223372036854775807L],
        text:"中文💎\\\"",single:'it\'s',list:[{x:true},{x:false}],empty:[],nested:{}}"#;
    let tag = snbt::parse(input).unwrap();
    let encoded = snbt::to_string(&tag).unwrap();
    let decoded = snbt::parse(&encoded).unwrap();
    assert!(tags_equal_for_write(&tag, &decoded));
}

#[test]
fn scalar_roots_have_the_expected_types() {
    for (input, expected) in [
        ("+127B", NbtTag::Byte(127)),
        ("-32768s", NbtTag::Short(i16::MIN)),
        ("2147483647", NbtTag::Int(i32::MAX)),
        ("9223372036854775807l", NbtTag::Long(i64::MAX)),
        ("1e3", NbtTag::Double(1000.0)),
        (".5F", NbtTag::Float(0.5)),
        ("TRUE", NbtTag::Byte(1)),
        ("false", NbtTag::Byte(0)),
        ("128b", NbtTag::String("128b".into())),
        (
            "9223372036854775808",
            NbtTag::String("9223372036854775808".into()),
        ),
        ("'12'", NbtTag::String("12".into())),
        ("hello", NbtTag::String("hello".into())),
    ] {
        assert_eq!(snbt::parse(input).unwrap(), expected, "{input}");
    }
}

#[test]
fn empty_typed_arrays_keep_their_type() {
    for (input, expected) in [
        ("[B;]", NbtTag::ByteArray(vec![])),
        ("[I;]", NbtTag::IntArray(vec![])),
        ("[L;]", NbtTag::LongArray(vec![])),
    ] {
        assert_eq!(snbt::parse(input).unwrap(), expected);
        assert_eq!(snbt::to_string(&expected).unwrap(), input);
    }
}

#[test]
fn malformed_text_and_invalid_types_return_offsets() {
    for input in [
        "",
        "{",
        "[",
        "{x}",
        "{x:}",
        "[1,2b]",
        "[B;1]",
        "[I;1L]",
        "[L;1]",
        "{}x",
        "'broken",
        "'bad\\n'",
        "{x:1 y:2}",
        "minecraft:stone",
    ] {
        let error = snbt::parse(input).unwrap_err();
        assert!(
            matches!(error, bedrock_nbt::Error::Snbt { .. }),
            "{input}: {error}"
        );
    }
}

#[test]
fn binary_only_tags_fail_explicitly_in_text_output() {
    for tag in [
        NbtTag::End,
        NbtTag::ShortArray(vec![1]),
        NbtTag::ByteString(vec![255]),
    ] {
        assert!(snbt::to_string(&tag).is_err());
    }
}

#[test]
fn strings_enforce_byte_limits_and_handle_simd_boundaries() {
    for len in [0, 1, 15, 16, 31, 32, 63, 64, 65, 127, 128, 129, 65535] {
        for suffix in ["", "\\", "\"", "💎"] {
            let value = format!("{}{suffix}", "x".repeat(len));
            let tag = NbtTag::String(value.clone());
            if value.len() <= 65535 {
                let text = snbt::to_string(&tag).unwrap();
                assert_eq!(snbt::parse(&text).unwrap(), tag);
            } else {
                assert!(snbt::to_string(&tag).is_err());
            }
        }
    }
    assert!(snbt::parse(&format!("\"{}\"", "x".repeat(65536))).is_err());
}

#[test]
fn depth_is_bounded_and_float_bits_survive_finite_roundtrips() {
    assert!(snbt::parse(&format!("{}0{}", "[".repeat(130), "]".repeat(130))).is_err());
    for bits in [0, 1, 0x8000_0000, 0x7f7f_ffff, 0x0080_0000, 0x3eaa_aaab] {
        let tag = NbtTag::Float(f32::from_bits(bits));
        assert!(tags_equal_for_write(
            &tag,
            &snbt::parse(&snbt::to_string(&tag).unwrap()).unwrap()
        ));
    }
    for bits in [
        0,
        1,
        0x8000_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
        0x3fd5_5555_5555_5555,
    ] {
        let tag = NbtTag::Double(f64::from_bits(bits));
        assert!(tags_equal_for_write(
            &tag,
            &snbt::parse(&snbt::to_string(&tag).unwrap()).unwrap()
        ));
    }
}

#[test]
fn typed_array_elements_do_not_add_a_container_depth() {
    let mut tag = NbtTag::ByteArray(vec![1]);
    for _ in 0..128 {
        tag = NbtTag::List(vec![tag]);
    }
    let text = snbt::to_string(&tag).unwrap();
    assert_eq!(snbt::parse(&text).unwrap(), tag);
}

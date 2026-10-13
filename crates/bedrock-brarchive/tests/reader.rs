use std::io::Write;

use bedrock_brarchive::{Acceleration, Archive, Error, Index};

fn container(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = vec![0; 16 + entries.len() * 256];
    bytes[..8].copy_from_slice(&[0x7d, 0x27, 0x25, 0xb1, 0xa0, 0x52, 0x70, 0x26]);
    bytes[8..12].copy_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    let mut offset = 0u32;
    for (index, (name, payload)) in entries.iter().enumerate() {
        let record = &mut bytes[16 + index * 256..16 + (index + 1) * 256];
        record[0] = u8::try_from(name.len()).unwrap();
        record[1..=name.len()].copy_from_slice(name.as_bytes());
        record[248..252].copy_from_slice(&offset.to_le_bytes());
        let length = u32::try_from(payload.len()).unwrap();
        record[252..256].copy_from_slice(&length.to_le_bytes());
        offset += length;
    }
    for (_, payload) in entries {
        bytes.extend_from_slice(payload);
    }
    bytes
}

#[test]
fn binary_stubs_duplicates_and_unsorted_names_are_preserved() {
    let bytes = container(&[
        ("z.png", &[0, 255, 128]),
        ("a.json", b"old"),
        ("stub.png", b""),
        ("a.json", b"new"),
    ]);
    let index = Index::parse(&bytes, Acceleration::Auto).unwrap();
    assert_eq!(index.entries().len(), 3);
    assert_eq!(index.get("a.json").unwrap().bytes(&bytes).unwrap(), b"new");
    assert_eq!(
        index.get("z.png").unwrap().bytes(&bytes).unwrap(),
        &[0, 255, 128]
    );
    assert!(index.get("stub.png").unwrap().is_empty());
    assert!(index.get("absent").is_none());
    assert!(index.get("Z.png").is_none());
}

#[test]
fn overlapping_payloads_are_valid() {
    let mut bytes = container(&[("a", b"same"), ("b", b"same")]);
    bytes[16 + 256 + 248..16 + 256 + 252].copy_from_slice(&0u32.to_le_bytes());
    let index = Index::parse(&bytes, Acceleration::Auto).unwrap();
    assert_eq!(
        index.get("a").unwrap().bytes(&bytes).unwrap(),
        index.get("b").unwrap().bytes(&bytes).unwrap()
    );
}

#[test]
fn all_truncated_prefixes_and_invalid_ranges_return_errors() {
    let bytes = container(&[("texture.png", b"payload")]);
    for length in 0..bytes.len() {
        assert!(
            Index::parse(&bytes[..length], Acceleration::Auto).is_err(),
            "prefix {length}"
        );
    }
    let mut invalid = bytes;
    invalid[264..268].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Index::parse(&invalid, Acceleration::Auto).is_err());
}

#[test]
fn invalid_magic_versions_counts_and_names_return_errors() {
    let original = container(&[("a", b"payload")]);
    let mut bytes = original.clone();
    bytes[0] ^= 1;
    assert!(matches!(
        Index::parse(&bytes, Acceleration::Auto),
        Err(Error::Invalid(_))
    ));
    bytes = original.clone();
    bytes[12..16].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(
        Index::parse(&bytes, Acceleration::Auto),
        Err(Error::UnsupportedVersion(2))
    ));
    bytes = original.clone();
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        Index::parse(&bytes, Acceleration::Auto),
        Err(Error::LimitExceeded(_))
    ));
    for name in [
        "",
        "../escape",
        "/root",
        "C:drive",
        "a\\b",
        "a//b",
        "a/./b",
        "a\0b",
    ] {
        assert!(
            Index::parse(&container(&[(name, b"x")]), Acceleration::Auto).is_err(),
            "{name:?}"
        );
    }
    bytes = original.clone();
    bytes[16] = 248;
    assert!(Index::parse(&bytes, Acceleration::Auto).is_err());
    bytes = original;
    bytes[17] = 0xff;
    assert!(Index::parse(&bytes, Acceleration::Auto).is_err());
}

#[test]
fn file_reads_reuse_buffers_stream_and_distinguish_missing_entries() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&container(&[
        ("image.png", &[137, 80, 78, 71]),
        ("stub", b""),
    ]))
    .unwrap();
    let archive = Archive::open(file.path(), Acceleration::Auto).unwrap();
    let mut buffer = Vec::with_capacity(100);
    let pointer = buffer.as_ptr();
    assert!(archive.read_into("image.png", &mut buffer).unwrap());
    assert_eq!(buffer, &[137, 80, 78, 71]);
    assert_eq!(buffer.as_ptr(), pointer);
    let mut output = Vec::new();
    assert_eq!(archive.copy_to("image.png", &mut output).unwrap(), Some(4));
    assert_eq!(output, buffer);
    assert!(archive.read_into("stub", &mut buffer).unwrap());
    assert!(buffer.is_empty());
    assert!(!archive.read_into("missing", &mut buffer).unwrap());
    assert_eq!(archive.copy_to("missing", &mut output).unwrap(), None);
}

#[test]
fn truncation_after_open_is_reported_and_does_not_leave_partial_buffer() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&container(&[("a", b"payload")])).unwrap();
    let archive = Archive::open(file.path(), Acceleration::Auto).unwrap();
    file.as_file().set_len(272).unwrap();
    let mut buffer = Vec::new();
    assert!(archive.read_into("a", &mut buffer).is_err());
    assert!(buffer.is_empty());
    assert!(archive.copy_to("a", &mut Vec::new()).is_err());
}

#[test]
fn concurrent_readers_do_not_share_an_unsynchronized_file_cursor() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&container(&[("a", b"first"), ("b", b"second")]))
        .unwrap();
    let archive = std::sync::Arc::new(Archive::open(file.path(), Acceleration::Auto).unwrap());
    let threads: Vec<_> = (0..4)
        .map(|worker| {
            let archive = std::sync::Arc::clone(&archive);
            std::thread::spawn(move || {
                let (name, expected): (&str, &[u8]) = if worker % 2 == 0 {
                    ("a", b"first")
                } else {
                    ("b", b"second")
                };
                let mut buffer = Vec::new();
                for _ in 0..100 {
                    assert!(archive.read_into(name, &mut buffer).unwrap());
                    assert_eq!(buffer, expected);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn oversized_payload_is_rejected_before_allocating() {
    let mut bytes = container(&[("large", b"")]);
    let length = 256u32 * 1024 * 1024 + 1;
    bytes[268..272].copy_from_slice(&length.to_le_bytes());
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file.as_file().set_len(272 + u64::from(length)).unwrap();
    let archive = Archive::open(file.path(), Acceleration::Auto).unwrap();
    let mut buffer = Vec::new();
    assert!(matches!(
        archive.read_into("large", &mut buffer),
        Err(Error::LimitExceeded(_))
    ));
    assert!(buffer.is_empty());
    assert_eq!(buffer.capacity(), 0);
}

#[test]
fn scalar_and_auto_have_identical_metadata_and_malformed_input_errors() {
    let name = "long_texture_filename_for_simd_validation_材质.json";
    let bytes = container(&[(name, b"payload")]);
    let result =
        |bytes: &[u8], policy| Index::parse(bytes, policy).map_err(|error| error.to_string());
    assert_eq!(
        result(&bytes, Acceleration::Scalar),
        result(&bytes, Acceleration::Auto)
    );
    for length in 0..bytes.len() {
        assert_eq!(
            result(&bytes[..length], Acceleration::Scalar),
            result(&bytes[..length], Acceleration::Auto)
        );
    }
    for position in 0..name.len() {
        for byte in [0, b':', b'\\', 0xff] {
            let mut malformed = bytes.clone();
            malformed[17 + position] = byte;
            assert_eq!(
                result(&malformed, Acceleration::Scalar),
                result(&malformed, Acceleration::Auto)
            );
        }
    }
}

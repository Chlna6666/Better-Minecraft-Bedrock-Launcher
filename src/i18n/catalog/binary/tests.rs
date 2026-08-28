use super::*;

const EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/locales.bin"));

#[test]
fn borrows_text_and_finds_keys_in_the_embedded_index() {
    let catalog = Catalog::read(EMBEDDED).expect("embedded catalog");
    let index = catalog.key_index("common.cancel").expect("embedded key");
    let translation = catalog.translation(0, index).expect("Chinese entry");
    assert_eq!(translation.text, "取消");
    let start = EMBEDDED.as_ptr() as usize;
    let address = translation.text.as_ptr() as usize;
    assert!((start..start + EMBEDDED.len()).contains(&address));
    assert!(catalog.key_index("missing.translation.for.test").is_none());
    assert!(catalog.translation(usize::MAX, index).is_none());
    assert!(catalog.translation(0, usize::MAX).is_none());
}

#[test]
fn rejects_truncated_headers_and_invalid_section_offsets() {
    for length in 0..HEADER_LEN {
        assert!(Catalog::read(&EMBEDDED[..length]).is_none());
    }
    let mut bytes = EMBEDDED.to_vec();
    bytes[0] ^= 1;
    assert!(Catalog::read(&bytes).is_none());
    bytes[0] ^= 1;
    bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Catalog::read(&bytes).is_none());
}

#[test]
fn rejects_invalid_utf8_and_out_of_bounds_text() {
    let mut bytes = EMBEDDED.to_vec();
    let pool = integer(&bytes, 16).expect("pool offset");
    let saved = bytes[pool];
    bytes[pool] = 0xff;
    assert!(Catalog::read(&bytes).is_none());
    bytes[pool] = saved;
    let index = Catalog::read(&bytes)
        .expect("catalog")
        .key_index("common.cancel")
        .expect("key");
    let values = HEADER_LEN + integer(&bytes, 8).expect("key count") * KEY_LEN;
    let record = values + index * VALUE_LEN;
    bytes[record..record + 4].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    let catalog = Catalog::read(&bytes).expect("valid section header");
    assert!(catalog.translation(0, index).is_none());
}

fn records(offsets: &[[u32; 3]]) -> Vec<u8> {
    offsets
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[test]
fn uses_precompiled_unicode_and_repeated_placeholder_offsets() {
    let text = "雪{{value}} / {{value}} {{other}} {{open";
    let bytes = records(&[[5, 5, 0], [17, 5, 0], [27, 5, 1]]);
    let parts = Parts {
        text,
        records: &bytes,
        cursor: 0,
        finished: false,
    };
    let named = crate::i18n::interpolate_args(parts.clone(), crate::i18n_args![("value", "雨")]);
    let positional =
        crate::i18n::interpolate_positional_args(parts, crate::i18n_positional_args!["雨"]);
    assert_eq!(named, "雪雨 / 雨 {{other}} {{open");
    assert_eq!(positional, named);
}

#[test]
fn invalid_placeholder_offsets_do_not_panic_or_resume_iteration() {
    let bytes = records(&[[u32::MAX, 1, 0]]);
    let mut parts = Parts {
        text: "{{value}}",
        records: &bytes,
        cursor: 0,
        finished: false,
    };
    assert!(parts.next().is_none());
    assert!(parts.next().is_none());
}

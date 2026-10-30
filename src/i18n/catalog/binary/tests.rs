use super::*;
use crate::i18n::{Locale, interpolate_owned_args};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

const EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/locales.bin"));

struct LocaleSource {
    locale: Locale,
    primary: &'static str,
    extra: &'static str,
}

static LOCALE_SOURCES: [LocaleSource; 5] = [
    LocaleSource {
        locale: Locale::ZhCn,
        primary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/zh-CN.lang"
        )),
        extra: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/extra/zh-CN.lang"
        )),
    },
    LocaleSource {
        locale: Locale::ZhTw,
        primary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/zh-TW.lang"
        )),
        extra: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/extra/zh-TW.lang"
        )),
    },
    LocaleSource {
        locale: Locale::EnUs,
        primary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/en-US.lang"
        )),
        extra: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/extra/en-US.lang"
        )),
    },
    LocaleSource {
        locale: Locale::JaJp,
        primary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/ja-JP.lang"
        )),
        extra: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/extra/ja-JP.lang"
        )),
    },
    LocaleSource {
        locale: Locale::KoKr,
        primary: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/ko-KR.lang"
        )),
        extra: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/locales/extra/ko-KR.lang"
        )),
    },
];

fn parse_entries(source: &'static str, entries: &mut BTreeMap<&'static str, &'static str>) {
    for line in source.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key, text)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        entries.insert(key, text.trim());
    }
}

fn expected_entries(source: &LocaleSource) -> BTreeMap<&'static str, &'static str> {
    let mut entries = BTreeMap::new();
    parse_entries(source.primary, &mut entries);
    parse_entries(source.extra, &mut entries);
    entries
}

fn expected_entries_by_locale() -> Vec<BTreeMap<&'static str, &'static str>> {
    LOCALE_SOURCES.iter().map(expected_entries).collect()
}

fn expected_keys(entries: &[BTreeMap<&'static str, &'static str>]) -> BTreeSet<&'static str> {
    entries
        .iter()
        .flat_map(|entries| entries.keys().copied())
        .collect()
}

struct ExpectedPart {
    literal: &'static str,
    placeholder: Option<(&'static str, usize)>,
}

fn expected_parts(text: &'static str) -> Vec<ExpectedPart> {
    let mut names = Vec::<&'static str>::new();
    let mut parts = Vec::new();
    let mut cursor = 0;
    let mut remaining = text;
    while let Some(open) = remaining.find("{{") {
        let after_open = &remaining[open + 2..];
        let Some(close) = after_open.find("}}") else {
            break;
        };
        let name = &after_open[..close];
        let argument_index = names
            .iter()
            .position(|existing| *existing == name)
            .unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            });
        let name_start = text.len() - remaining.len() + open + 2;
        parts.push(ExpectedPart {
            literal: &text[cursor..name_start - 2],
            placeholder: Some((name, argument_index)),
        });
        cursor = name_start + name.len() + 2;
        remaining = &after_open[close + 2..];
    }
    parts.push(ExpectedPart {
        literal: &text[cursor..],
        placeholder: None,
    });
    parts
}

fn embedded_range() -> std::ops::Range<usize> {
    let start = EMBEDDED.as_ptr() as usize;
    start..start + EMBEDDED.len()
}

fn assert_entry(
    catalog: &Catalog<'static>,
    locale_index: usize,
    locale: Locale,
    key: &'static str,
    text: &'static str,
    embedded: &std::ops::Range<usize>,
) {
    let index = catalog
        .key_index(key)
        .unwrap_or_else(|| panic!("missing key {key}"));
    let translation = catalog
        .translation(locale_index, index)
        .unwrap_or_else(|| panic!("missing {locale:?} translation for {key}"));
    assert_eq!(translation.text, text, "{locale:?}/{key}");
    let address = translation.text.as_ptr() as usize;
    assert!(
        embedded.contains(&address),
        "{locale:?}/{key} text is not borrowed from the embedded catalog"
    );

    let expected = expected_parts(text);
    let actual = translation.parts.clone().collect::<Vec<_>>();
    assert_eq!(actual.len(), expected.len(), "{locale:?}/{key} part count");
    for (part, expected) in actual.iter().zip(&expected) {
        assert_eq!(part.literal, expected.literal, "{locale:?}/{key} literal");
        assert_eq!(
            part.placeholder,
            expected.placeholder.map(|(name, _)| name),
            "{locale:?}/{key} placeholder"
        );
        assert_eq!(
            part.argument_index,
            expected.placeholder.map(|(_, index)| index),
            "{locale:?}/{key} argument index"
        );
    }

    let mut rebuilt = String::new();
    for part in &actual {
        rebuilt.push_str(part.literal);
        if let Some(name) = part.placeholder {
            rebuilt.push_str("{{");
            rebuilt.push_str(name);
            rebuilt.push_str("}}");
        }
    }
    assert_eq!(rebuilt, text, "{locale:?}/{key} reconstructed text");

    let mut names = Vec::<&'static str>::new();
    for part in &actual {
        if let Some(name) = part.placeholder
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    let args = names
        .iter()
        .map(|name| (*name, Arc::<str>::from(format!("<{name}>"))))
        .collect::<Vec<_>>();
    let mut expected_interpolation = String::new();
    for part in &actual {
        expected_interpolation.push_str(part.literal);
        if let Some(name) = part.placeholder {
            match args.iter().find(|(candidate, _)| *candidate == name) {
                Some((_, value)) => expected_interpolation.push_str(value),
                None => {
                    expected_interpolation.push_str("{{");
                    expected_interpolation.push_str(name);
                    expected_interpolation.push_str("}}");
                }
            }
        }
    }
    assert_eq!(
        interpolate_owned_args(translation.parts, &args),
        expected_interpolation,
        "{locale:?}/{key} interpolation"
    );
}

#[test]
fn header_and_record_layout_is_compact() {
    assert_eq!(HEADER_LEN, 32);
    assert_eq!(KEY_LEN, 3);
    assert_eq!(VALUE_LEN, 7);
    assert_eq!(PART_LEN, 6);
    assert_eq!(&EMBEDDED[..8], MAGIC);
    assert_eq!(integer(EMBEDDED, 28), Some(0));

    let key_count = integer(EMBEDDED, 8).expect("key count");
    let locale_count = integer(EMBEDDED, 12).expect("locale count");
    let part_count = integer(EMBEDDED, 16).expect("part count");
    let pool_offset = integer(EMBEDDED, 20).expect("pool offset");
    let pool_len = integer(EMBEDDED, 24).expect("pool length");
    let values_offset = HEADER_LEN + key_count * KEY_LEN;
    let parts_offset = values_offset + key_count * locale_count * VALUE_LEN;
    assert_eq!(parts_offset + part_count * PART_LEN, pool_offset);
    assert_eq!(pool_offset + pool_len, EMBEDDED.len());
    assert_eq!(EMBEDDED.last(), Some(&0));
}

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
    assert!(Catalog::read(&EMBEDDED[..EMBEDDED.len() - 1]).is_none());
    let mut bytes = EMBEDDED.to_vec();
    bytes[0] ^= 1;
    assert!(Catalog::read(&bytes).is_none());
    bytes[0] ^= 1;
    bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Catalog::read(&bytes).is_none());
    let mut bytes = EMBEDDED.to_vec();
    bytes[28..32].copy_from_slice(&1u32.to_le_bytes());
    assert!(Catalog::read(&bytes).is_none());
}

#[test]
fn rejects_invalid_utf8_and_out_of_bounds_text() {
    let mut bytes = EMBEDDED.to_vec();
    let pool = integer(&bytes, 20).expect("pool offset");
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
    bytes[record..record + 3].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    let catalog = Catalog::read(&bytes).expect("valid section header");
    assert!(catalog.translation(0, index).is_none());
    bytes[record..record + 3].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    let catalog = Catalog::read(&bytes).expect("valid section header");
    assert!(catalog.translation(0, index).is_none());
}

#[test]
fn rejects_unsorted_duplicate_and_unterminated_keys() {
    let mut bytes = EMBEDDED.to_vec();
    let first = bytes[HEADER_LEN..HEADER_LEN + KEY_LEN].to_vec();
    let second = bytes[HEADER_LEN + KEY_LEN..HEADER_LEN + KEY_LEN * 2].to_vec();
    bytes[HEADER_LEN..HEADER_LEN + KEY_LEN].copy_from_slice(&second);
    bytes[HEADER_LEN + KEY_LEN..HEADER_LEN + KEY_LEN * 2].copy_from_slice(&first);
    assert!(Catalog::read(&bytes).is_none());

    let mut bytes = EMBEDDED.to_vec();
    let first = bytes[HEADER_LEN..HEADER_LEN + KEY_LEN].to_vec();
    bytes[HEADER_LEN + KEY_LEN..HEADER_LEN + KEY_LEN * 2].copy_from_slice(&first);
    assert!(Catalog::read(&bytes).is_none());

    let mut bytes = EMBEDDED.to_vec();
    let pool = integer(&bytes, 20).expect("pool offset");
    let pool_len = integer(&bytes, 24).expect("pool length");
    bytes[pool + pool_len - 1] = b'x';
    assert!(Catalog::read(&bytes).is_none());
}

fn records(offsets: &[[u32; 2]]) -> Vec<u8> {
    offsets
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes()[..3].to_vec())
        .collect()
}

#[test]
fn uses_precompiled_unicode_and_repeated_placeholder_offsets() {
    let text = "雪{{value}} / {{value}} {{other}} {{open";
    let pool = "value\0other\0";
    let bytes = records(&[[0, 5], [0, 17], [6, 27]]);
    let parts = Parts {
        text,
        pool,
        records: &bytes,
        record_index: 0,
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
    let bytes = records(&[[u32::MAX, 2]]);
    let mut parts = Parts {
        text: "{{value}}",
        pool: "value\0",
        records: &bytes,
        record_index: 0,
        cursor: 0,
        finished: false,
    };
    assert!(parts.next().is_none());
    assert!(parts.next().is_none());

    let bytes = records(&[[0, 3]]);
    let mut parts = Parts {
        text: "{{value}}",
        pool: "value\0",
        records: &bytes,
        record_index: 0,
        cursor: 0,
        finished: false,
    };
    assert!(parts.next().is_none());
    assert!(parts.next().is_none());
}

#[test]
fn embedded_catalog_matches_all_11460_locale_entries() {
    let expected = expected_entries_by_locale();
    let keys = expected_keys(&expected);
    let total = expected.iter().map(BTreeMap::len).sum::<usize>();
    assert_eq!(keys.len(), 2_292, "expected locale key count");
    assert_eq!(
        total, 11_460,
        "expected translated entries across five locales"
    );
    assert_eq!(total, keys.len() * LOCALE_SOURCES.len());

    let catalog = Catalog::read(EMBEDDED).expect("embedded catalog");
    assert_eq!(integer(EMBEDDED, 8), Some(keys.len()));
    assert_eq!(integer(EMBEDDED, 12), Some(LOCALE_SOURCES.len()));
    assert_eq!(
        integer(EMBEDDED, 8).expect("key count") * integer(EMBEDDED, 12).expect("locale count"),
        total
    );

    let embedded = embedded_range();
    let mut checked = 0usize;
    for (locale_index, source) in LOCALE_SOURCES.iter().enumerate() {
        assert_eq!(usize::from(source.locale.index()), locale_index);
        for (key, text) in &expected[locale_index] {
            assert_entry(&catalog, locale_index, source.locale, key, text, &embedded);
            checked += 1;
        }
    }
    assert_eq!(checked, 11_460);
}

#[test]
#[ignore = "performance measurement; run with --ignored --nocapture"]
fn key_lookup_and_translation_interpolation_performance() {
    const ITERATIONS: usize = 200_000;

    let catalog = Catalog::read(EMBEDDED).expect("embedded catalog");
    let expected = expected_entries_by_locale();
    let keys = expected_keys(&expected).into_iter().collect::<Vec<_>>();
    let mut checksum = 0usize;

    let start = Instant::now();
    for iteration in 0..ITERATIONS {
        let key = keys[iteration % keys.len()];
        checksum = checksum.wrapping_add(catalog.key_index(key).expect("embedded key"));
    }
    let lookup = start.elapsed();

    let index = catalog
        .key_index("LauncherSettings.language_save_failed")
        .expect("interpolated key");
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        checksum = checksum.wrapping_add(
            catalog
                .translation(0, index)
                .expect("Chinese translation")
                .text
                .len(),
        );
    }
    let translation = start.elapsed();

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let translated = catalog.translation(0, index).expect("Chinese translation");
        checksum = checksum.wrapping_add(
            crate::i18n::interpolate_args(
                translated.parts,
                crate::i18n_args![("error", "disk full")],
            )
            .len(),
        );
    }
    let interpolation = start.elapsed();
    std::hint::black_box(checksum);

    let per_iteration =
        |elapsed: std::time::Duration| elapsed.as_secs_f64() * 1_000_000_000.0 / ITERATIONS as f64;
    println!(
        "i18n performance over {ITERATIONS} iterations: key lookup {:.2} ns/op, translation {:.2} ns/op, translation+interpolation {:.2} ns/op",
        per_iteration(lookup),
        per_iteration(translation),
        per_iteration(interpolation),
    );
}

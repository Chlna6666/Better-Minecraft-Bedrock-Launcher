use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const LOCALE_CODES: [&str; 5] = ["zh-CN", "zh-TW", "en-US", "ja-JP", "ko-KR"];
const MAGIC: &[u8; 8] = b"BMCLANG1";
const HEADER_LEN: usize = 20;
const KEY_RECORD_LEN: usize = 8;
const VALUE_RECORD_LEN: usize = 16;
const PART_RECORD_LEN: usize = 12;

/// Read the five embedded language files and write the compact binary catalog.
///
/// Small feature-specific translations may live in `assets/locales/extra/<locale>.lang`. Extra
/// entries are appended after the primary catalog, so the normal last-key-wins parser can both add
/// new keys and intentionally override an existing translation without rewriting the large base
/// language files.
pub fn generate(manifest_dir: &Path, out_dir: &Path) -> io::Result<()> {
    let paths = locale_paths(manifest_dir);
    let extra_paths = locale_extra_paths(manifest_dir);
    for path in paths.iter().chain(extra_paths.iter()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let sources = paths
        .iter()
        .zip(extra_paths.iter())
        .map(|(path, extra_path)| {
            let mut source = fs::read_to_string(path)?;
            match fs::read_to_string(extra_path) {
                Ok(extra) => {
                    if !source.ends_with('\n') {
                        source.push('\n');
                    }
                    source.push_str(&extra);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            Ok(source)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let source_refs = sources.iter().map(String::as_str).collect::<Vec<_>>();
    let bytes = encode(&source_refs)?;
    write_if_changed(&out_dir.join("locales.bin"), &bytes)
}

/// Encode language sources without generating Rust code or validating catalogs.
pub(crate) fn encode(sources: &[&str]) -> io::Result<Vec<u8>> {
    let catalogs = sources
        .iter()
        .map(|source| parse(source))
        .collect::<Vec<_>>();
    let keys = catalog_keys(&catalogs);
    let key_count = as_u32(keys.len(), "key count")?;
    let locale_count = as_u32(catalogs.len(), "locale count")?;
    let mut pool = Vec::new();
    let mut pool_index = BTreeMap::new();
    let key_records = encode_keys(&keys, &mut pool, &mut pool_index)?;
    let (value_records, part_records) =
        encode_values(&catalogs, &keys, &mut pool, &mut pool_index)?;
    let key_bytes = record_section_len(keys.len(), KEY_RECORD_LEN, key_records.len(), "key")?;
    let value_count = checked_mul(keys.len(), catalogs.len(), "value count")?;
    let value_bytes =
        record_section_len(value_count, VALUE_RECORD_LEN, value_records.len(), "value")?;
    let part_count = part_records.len() / PART_RECORD_LEN;
    let parts_bytes = record_section_len(part_count, PART_RECORD_LEN, part_records.len(), "parts")?;
    let pool_offset = checked_add(
        checked_add(
            checked_add(HEADER_LEN, key_bytes, "catalog layout")?,
            value_bytes,
            "catalog layout",
        )?,
        parts_bytes,
        "catalog layout",
    )?;
    let total_len = checked_add(pool_offset, pool.len(), "catalog length")?;
    let mut output = Vec::new();
    output
        .try_reserve(total_len)
        .map_err(|error| io_error(format!("failed to reserve catalog buffer: {error}")))?;
    output.extend_from_slice(MAGIC);
    push_u32(&mut output, key_count);
    push_u32(&mut output, locale_count);
    push_u32(&mut output, as_u32(pool_offset, "pool offset")?);
    output.extend_from_slice(&key_records);
    output.extend_from_slice(&value_records);
    output.extend_from_slice(&part_records);
    output.extend_from_slice(&pool);
    Ok(output)
}

fn encode_keys<'a>(
    keys: &[&'a str],
    pool: &mut Vec<u8>,
    pool_index: &mut BTreeMap<&'a str, u32>,
) -> io::Result<Vec<u8>> {
    let mut records = Vec::new();
    for &key in keys {
        let (offset, length) = intern(key, pool, pool_index)?;
        push_u32(&mut records, offset);
        push_u32(&mut records, length);
    }
    Ok(records)
}

fn encode_values<'a>(
    catalogs: &[BTreeMap<&'a str, &'a str>],
    keys: &[&'a str],
    pool: &mut Vec<u8>,
    pool_index: &mut BTreeMap<&'a str, u32>,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut value_records = Vec::new();
    let mut part_records = Vec::new();
    let mut part_count = 0usize;
    for catalog in catalogs {
        for key in keys {
            let Some(&text) = catalog.get(key) else {
                push_u32(&mut value_records, u32::MAX);
                push_u32(&mut value_records, 0);
                push_u32(&mut value_records, 0);
                push_u32(&mut value_records, 0);
                continue;
            };
            let (offset, length) = intern(text, pool, pool_index)?;
            let first_part_index = as_u32(part_count, "part index")?;
            let value_part_count = append_parts(text, &mut part_records, &mut part_count)?;
            push_u32(&mut value_records, offset);
            push_u32(&mut value_records, length);
            push_u32(&mut value_records, first_part_index);
            push_u32(&mut value_records, value_part_count);
        }
    }
    Ok((value_records, part_records))
}

fn locale_paths(manifest_dir: &Path) -> Vec<PathBuf> {
    LOCALE_CODES
        .iter()
        .map(|code| {
            manifest_dir
                .join("assets")
                .join("locales")
                .join(format!("{code}.lang"))
        })
        .collect()
}

fn locale_extra_paths(manifest_dir: &Path) -> Vec<PathBuf> {
    LOCALE_CODES
        .iter()
        .map(|code| {
            manifest_dir
                .join("assets")
                .join("locales")
                .join("extra")
                .join(format!("{code}.lang"))
        })
        .collect()
}

fn parse(source: &str) -> BTreeMap<&str, &str> {
    let mut entries = BTreeMap::new();
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
    entries
}

fn catalog_keys<'a>(catalogs: &[BTreeMap<&'a str, &'a str>]) -> Vec<&'a str> {
    catalogs
        .iter()
        .flat_map(|catalog| catalog.keys().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn intern<'a>(
    value: &'a str,
    pool: &mut Vec<u8>,
    pool_index: &mut BTreeMap<&'a str, u32>,
) -> io::Result<(u32, u32)> {
    let length = as_u32(value.len(), "string length")?;
    if let Some(&offset) = pool_index.get(value) {
        return Ok((offset, length));
    }
    let end = checked_add(pool.len(), value.len(), "string pool length")?;
    let offset = as_pool_offset(pool.len())?;
    as_pool_offset(end)?;
    pool.extend_from_slice(value.as_bytes());
    pool_index.insert(value, offset);
    Ok((offset, length))
}

fn append_parts(text: &str, records: &mut Vec<u8>, total_count: &mut usize) -> io::Result<u32> {
    let first_count = *total_count;
    let mut names = Vec::<&str>::new();
    let mut remaining = text;
    while let Some(open) = remaining.find("{{") {
        // find() matched both delimiter bytes, so these slices stay in the input.
        let after_open = &remaining[open + 2..];
        let Some(close) = after_open.find("}}") else {
            break;
        };
        let name = &after_open[..close];
        let name_start = text.len() - remaining.len() + open + 2;
        let argument_index = names
            .iter()
            .position(|existing| *existing == name)
            .unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            });
        push_u32(records, as_u32(name_start, "placeholder offset")?);
        push_u32(records, as_u32(name.len(), "placeholder length")?);
        push_u32(records, as_u32(argument_index, "argument index")?);
        *total_count = total_count
            .checked_add(1)
            .ok_or_else(|| io_error("part count overflow"))?;
        remaining = &after_open[close + 2..];
    }
    as_u32(
        total_count
            .checked_sub(first_count)
            .ok_or_else(|| io_error("part count underflow"))?,
        "part count",
    )
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => Ok(()),
        Ok(_) => fs::write(path, bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::write(path, bytes),
        Err(error) => Err(error),
    }
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn as_u32(value: usize, field: &str) -> io::Result<u32> {
    u32::try_from(value).map_err(|_| io_error(format!("{field} exceeds u32")))
}

fn as_pool_offset(value: usize) -> io::Result<u32> {
    let offset = as_u32(value, "pool offset")?;
    if offset == u32::MAX {
        return Err(io_error("pool offset collides with missing-value marker"));
    }
    Ok(offset)
}

fn record_section_len(
    record_count: usize,
    record_len: usize,
    actual_len: usize,
    section: &str,
) -> io::Result<usize> {
    let expected_len = checked_mul(record_count, record_len, "record section length")?;
    if expected_len != actual_len {
        return Err(io_error(format!("{section} section length mismatch")));
    }
    Ok(expected_len)
}

fn checked_add(left: usize, right: usize, field: &str) -> io::Result<usize> {
    left.checked_add(right)
        .ok_or_else(|| io_error(format!("{field} overflow")))
}

fn checked_mul(left: usize, right: usize, field: &str) -> io::Result<usize> {
    left.checked_mul(right)
        .ok_or_else(|| io_error(format!("{field} overflow")))
}

fn io_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
    }

    fn pool_text(bytes: &[u8], pool_offset: usize, offset: u32, length: u32) -> &str {
        let start = pool_offset + offset as usize;
        std::str::from_utf8(&bytes[start..start + length as usize]).expect("UTF-8 pool")
    }

    #[test]
    fn encoding_is_deterministic_and_sorted() {
        let sources = ["b=二\na=一\n", "c=三\na=一\n"];
        let first = encode(&sources).expect("encode");
        assert_eq!(first, encode(&sources).expect("repeat encode"));
        assert_eq!(&first[..8], MAGIC);
        assert_eq!(read_u32(&first, 8), 3);
        assert_eq!(read_u32(&first, 12), 2);
        let pool_offset = read_u32(&first, 16) as usize;
        let first_key = read_u32(&first, HEADER_LEN);
        let first_key_len = read_u32(&first, HEADER_LEN + 4);
        let second_key = read_u32(&first, HEADER_LEN + 8);
        let second_key_len = read_u32(&first, HEADER_LEN + 12);
        assert_eq!(
            pool_text(&first, pool_offset, first_key, first_key_len),
            "a"
        );
        assert_eq!(
            pool_text(&first, pool_offset, second_key, second_key_len),
            "b"
        );
    }

    #[test]
    fn missing_values_and_different_key_sets_are_encoded() {
        let bytes = encode(&["only_a=A\nshared=S", "only_b=B\nshared=S"]).expect("encode");
        let pool_offset = read_u32(&bytes, 16) as usize;
        let key_count = read_u32(&bytes, 8) as usize;
        assert_eq!(key_count, 3);
        let values = HEADER_LEN + key_count * KEY_RECORD_LEN;
        let only_b_missing_value = values + VALUE_RECORD_LEN;
        let only_a_missing_value = values + VALUE_RECORD_LEN * 3;
        let only_b_present_value = values + VALUE_RECORD_LEN * 4;
        assert_eq!(read_u32(&bytes, only_b_missing_value), u32::MAX);
        assert_eq!(read_u32(&bytes, only_a_missing_value), u32::MAX);
        assert_ne!(read_u32(&bytes, only_b_present_value), u32::MAX);
        assert!(pool_offset > values);
    }

    #[test]
    fn duplicate_keys_keep_the_last_value() {
        let bytes = encode(&["# comment\n// comment\nbad line\n =ignored\nkey=old\nkey=新\n"])
            .expect("encode");
        assert_eq!(read_u32(&bytes, 8), 1);
        let pool_offset = read_u32(&bytes, 16) as usize;
        let value_offset = HEADER_LEN + KEY_RECORD_LEN;
        let text_offset = read_u32(&bytes, value_offset);
        let text_len = read_u32(&bytes, value_offset + 4);
        assert_eq!(pool_text(&bytes, pool_offset, text_offset, text_len), "新");
    }

    #[test]
    fn unicode_placeholders_repeat_argument_indices_and_skip_unclosed() {
        let bytes = encode(&["msg=雪{{值}}/{{值}}/{{二}} {{未闭\n"]).expect("encode");
        let key_count = read_u32(&bytes, 8) as usize;
        let values = HEADER_LEN + key_count * KEY_RECORD_LEN;
        let first_part = read_u32(&bytes, values + 8) as usize;
        let part_count = read_u32(&bytes, values + 12) as usize;
        assert_eq!(first_part, 0);
        assert_eq!(part_count, 3);
        let parts = values + key_count * VALUE_RECORD_LEN;
        let first_name = read_u32(&bytes, parts);
        let first_len = read_u32(&bytes, parts + 4);
        assert_eq!(first_len, "值".len() as u32);
        assert_eq!(read_u32(&bytes, parts + 8), 0);
        assert_eq!(read_u32(&bytes, parts + PART_RECORD_LEN + 8), 0);
        assert_eq!(read_u32(&bytes, parts + PART_RECORD_LEN * 2 + 8), 1);
        assert_eq!(first_name, "雪{{".len() as u32);
    }

    #[test]
    fn identical_output_is_not_rewritten() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("bmcbl-locales-{suffix}"));
        fs::create_dir(&directory).expect("create test directory");
        let path = directory.join("locales.bin");
        let bytes = encode(&["key=value"]).expect("encode");
        fs::write(&path, &bytes).expect("seed output");
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("open output")
            .set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(946_684_800)),
            )
            .expect("set old timestamp");
        let modified = fs::metadata(&path)
            .expect("metadata")
            .modified()
            .expect("timestamp");
        write_if_changed(&path, &bytes).expect("unchanged output");
        assert_eq!(fs::read(&path).expect("read output"), bytes);
        assert_eq!(
            fs::metadata(&path)
                .expect("metadata")
                .modified()
                .expect("timestamp"),
            modified
        );
        fs::remove_file(&path).expect("remove output");
        fs::remove_dir(&directory).expect("remove test directory");
    }
}

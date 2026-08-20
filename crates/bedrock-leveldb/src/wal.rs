use crate::coding::masked_crc32c;
use crate::error::{LevelDbError, Result};
use std::fs::File;
use std::io::{Read, Write};

const BLOCK_SIZE: usize = 32 * 1024;
const HEADER_SIZE: usize = 7;

const ZERO_TYPE: u8 = 0;
const FULL_TYPE: u8 = 1;
const FIRST_TYPE: u8 = 2;
const MIDDLE_TYPE: u8 = 3;
const LAST_TYPE: u8 = 4;

pub(crate) fn append_record(file: &mut File, payload: &[u8]) -> Result<()> {
    let mut offset = usize::try_from(file.metadata()?.len()).map_err(|_| {
        LevelDbError::invalid_argument("log file length does not fit usize".to_string())
    })? % BLOCK_SIZE;
    let mut remaining = payload;
    let mut begin = true;

    while begin || !remaining.is_empty() {
        let leftover = BLOCK_SIZE - offset;
        if leftover < HEADER_SIZE {
            if leftover > 0 {
                file.write_all(&[0; HEADER_SIZE - 1][..leftover])?;
            }
            offset = 0;
        }

        let available = BLOCK_SIZE - offset - HEADER_SIZE;
        let fragment_len = remaining.len().min(available);
        let end = fragment_len == remaining.len();
        let record_type = match (begin, end) {
            (true, true) => FULL_TYPE,
            (true, false) => FIRST_TYPE,
            (false, true) => LAST_TYPE,
            (false, false) => MIDDLE_TYPE,
        };
        write_physical_record(file, record_type, &remaining[..fragment_len])?;
        offset += HEADER_SIZE + fragment_len;
        remaining = &remaining[fragment_len..];
        begin = false;
    }
    Ok(())
}

pub(crate) fn read_records(file: &mut File, paranoid_checks: bool) -> Result<Vec<Vec<u8>>> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let mut records = Vec::new();
    let mut scratch = Vec::new();
    let mut assembling = false;
    let mut pos = 0;

    while pos + HEADER_SIZE <= bytes.len() {
        let block_offset = pos % BLOCK_SIZE;
        if BLOCK_SIZE - block_offset < HEADER_SIZE {
            pos += BLOCK_SIZE - block_offset;
            continue;
        }

        let checksum = u32::from_le_bytes(bytes[pos..pos + 4].try_into().map_err(|_| {
            LevelDbError::corruption("log checksum header is truncated".to_string())
        })?);
        let length = usize::from(u16::from_le_bytes(
            bytes[pos + 4..pos + 6].try_into().map_err(|_| {
                LevelDbError::corruption("log length header is truncated".to_string())
            })?,
        ));
        let record_type = bytes[pos + 6];
        pos += HEADER_SIZE;

        if record_type == ZERO_TYPE && length == 0 {
            pos += BLOCK_SIZE - block_offset - HEADER_SIZE;
            continue;
        }
        let payload_capacity = BLOCK_SIZE - block_offset - HEADER_SIZE;
        if length > payload_capacity {
            if paranoid_checks {
                return Err(LevelDbError::corruption(
                    "log record crosses a physical block boundary".to_string(),
                ));
            }
            pos += BLOCK_SIZE - (pos % BLOCK_SIZE);
            scratch.clear();
            assembling = false;
            continue;
        }
        if pos + length > bytes.len() {
            if paranoid_checks {
                return Err(LevelDbError::corruption(
                    "log record payload is truncated".to_string(),
                ));
            }
            break;
        }
        let payload = &bytes[pos..pos + length];
        pos += length;

        if paranoid_checks {
            let record_type_bytes = [record_type];
            let actual = masked_crc32c(&[&record_type_bytes, payload]);
            if checksum != actual {
                return Err(LevelDbError::corruption(
                    "log record checksum mismatch".to_string(),
                ));
            }
        }

        match record_type {
            FULL_TYPE => {
                if assembling && paranoid_checks {
                    return Err(LevelDbError::corruption(
                        "full log record interrupts a fragmented record".to_string(),
                    ));
                }
                scratch.clear();
                assembling = false;
                records.push(payload.to_vec());
            }
            FIRST_TYPE => {
                if assembling && paranoid_checks {
                    return Err(LevelDbError::corruption(
                        "first log fragment interrupts a fragmented record".to_string(),
                    ));
                }
                scratch.clear();
                scratch.extend_from_slice(payload);
                assembling = true;
            }
            MIDDLE_TYPE => {
                if !assembling {
                    if paranoid_checks {
                        return Err(LevelDbError::corruption(
                            "middle log fragment has no first fragment".to_string(),
                        ));
                    }
                    continue;
                }
                scratch.extend_from_slice(payload);
            }
            LAST_TYPE => {
                if !assembling {
                    if paranoid_checks {
                        return Err(LevelDbError::corruption(
                            "last log fragment has no first fragment".to_string(),
                        ));
                    }
                    continue;
                }
                scratch.extend_from_slice(payload);
                records.push(std::mem::take(&mut scratch));
                assembling = false;
            }
            other if paranoid_checks => {
                return Err(LevelDbError::corruption(format!(
                    "unknown log record type {other}"
                )));
            }
            _ => {}
        }
    }

    if assembling && paranoid_checks {
        return Err(LevelDbError::corruption(
            "fragmented log record is missing its last fragment".to_string(),
        ));
    }
    if paranoid_checks && pos < bytes.len() && bytes[pos..].iter().any(|byte| *byte != 0) {
        return Err(LevelDbError::corruption(
            "log has a truncated physical record header".to_string(),
        ));
    }

    Ok(records)
}

fn write_physical_record(file: &mut File, record_type: u8, payload: &[u8]) -> Result<()> {
    let length = u16::try_from(payload.len())
        .map_err(|_| LevelDbError::invalid_argument("log fragment is too large".to_string()))?;
    let record_type_bytes = [record_type];
    let checksum = masked_crc32c(&[&record_type_bytes, payload]);

    file.write_all(&checksum.to_le_bytes())?;
    file.write_all(&length.to_le_bytes())?;
    file.write_all(&record_type_bytes)?;
    file.write_all(payload)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn log_records_roundtrip_with_fragmentation() {
        let path = std::env::temp_dir().join(format!(
            "bedrock-leveldb-log-{}.log",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("open");
            append_record(&mut file, b"small").expect("small");
            append_record(&mut file, &vec![9; BLOCK_SIZE * 2]).expect("large");
        }
        let mut file = File::open(&path).expect("open read");
        let records = read_records(&mut file, true).expect("read");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], b"small");
        assert_eq!(records[1], vec![9; BLOCK_SIZE * 2]);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn orphan_middle_fragment_is_rejected() {
        let path = temporary_log_path("orphan-middle");
        {
            let mut file = File::create(&path).expect("create");
            write_physical_record(&mut file, MIDDLE_TYPE, b"orphan").expect("write");
        }

        let error = read_records(&mut File::open(&path).expect("open"), true)
            .expect_err("orphan middle fragment must fail");

        assert_eq!(error.kind(), crate::error::ErrorKind::Corruption);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn unterminated_fragmented_record_is_rejected() {
        let path = temporary_log_path("unterminated");
        {
            let mut file = File::create(&path).expect("create");
            write_physical_record(&mut file, FIRST_TYPE, b"partial").expect("write");
        }

        let error = read_records(&mut File::open(&path).expect("open"), true)
            .expect_err("unterminated fragmented record must fail");

        assert_eq!(error.kind(), crate::error::ErrorKind::Corruption);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn zero_header_at_block_end_does_not_skip_next_block() {
        let path = temporary_log_path("zero-header-at-block-end");
        {
            let mut file = File::create(&path).expect("create");
            write_physical_record(
                &mut file,
                FULL_TYPE,
                &vec![1; BLOCK_SIZE - (2 * HEADER_SIZE)],
            )
            .expect("write first record");
            file.write_all(&[0; HEADER_SIZE])
                .expect("write zero header");
            write_physical_record(&mut file, FULL_TYPE, b"next-block").expect("write next block");
        }

        let records = read_records(&mut File::open(&path).expect("open"), true)
            .expect("read records across zero header");

        assert_eq!(records.len(), 2);
        assert_eq!(records[1], b"next-block");
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn nonzero_truncated_header_is_rejected() {
        let path = temporary_log_path("truncated-header");
        std::fs::write(&path, [1, 2, 3]).expect("write truncated header");

        let error = read_records(&mut File::open(&path).expect("open"), true)
            .expect_err("nonzero truncated header must fail");

        assert_eq!(error.kind(), crate::error::ErrorKind::Corruption);
        std::fs::remove_file(path).expect("cleanup");
    }

    fn temporary_log_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-{name}-{}.log",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }
}

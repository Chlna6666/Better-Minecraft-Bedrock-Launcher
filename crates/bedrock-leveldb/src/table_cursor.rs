use crate::coding::{crc32c, get_length_prefixed_slice, get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, decompress_owned};
use crate::error::{LevelDbError, Result};
use bytes::Bytes;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CUSTOM_TABLE_MAGIC: &[u8; 9] = b"BWLDBTBL1";
const CUSTOM_TABLE_VERSION: u32 = 1;
const CUSTOM_TABLE_HEADER_LEN: usize = CUSTOM_TABLE_MAGIC.len() + 9;
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;

#[derive(Debug, Clone)]
pub(crate) struct TableCursorEntry {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Option<Bytes>,
}

/// Sequential SSTable cursor used by compaction and batch scan pipelines.
///
/// The cursor performs no callback dispatch. Native data blocks are loaded one
/// at a time and their values are returned as shared `Bytes` slices, while the
/// positional read buffer and prefix-decoding key buffer are reused for the
/// lifetime of the cursor.
pub(crate) struct TableCursor {
    inner: CursorKind,
}

enum CursorKind {
    Custom(CustomCursor),
    Native(NativeCursor),
}

impl TableCursor {
    pub(crate) fn open(path: &Path, paranoid_checks: bool) -> Result<Self> {
        let file = File::open(path)
            .map_err(|error| LevelDbError::io_at("open table cursor", path, error))?;
        let mut magic = [0_u8; CUSTOM_TABLE_MAGIC.len()];
        let read = read_at(&file, &mut magic, 0)
            .map_err(|error| LevelDbError::io_at("read table cursor header", path, error))?;
        let inner = if read == CUSTOM_TABLE_MAGIC.len() && magic == *CUSTOM_TABLE_MAGIC {
            CursorKind::Custom(CustomCursor::open(file, path, paranoid_checks)?)
        } else {
            CursorKind::Native(NativeCursor::open(file, path, paranoid_checks)?)
        };
        Ok(Self { inner })
    }

    pub(crate) fn next(&mut self) -> Result<Option<TableCursorEntry>> {
        match &mut self.inner {
            CursorKind::Custom(cursor) => cursor.next(),
            CursorKind::Native(cursor) => cursor.next(),
        }
    }
}

struct CustomCursor {
    path: PathBuf,
    payload: Bytes,
    offset: usize,
    remaining: usize,
}

impl CustomCursor {
    fn open(mut file: File, path: &Path, paranoid_checks: bool) -> Result<Self> {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| LevelDbError::io_at("read custom table", path, error))?;
        if bytes.len() < CUSTOM_TABLE_HEADER_LEN || !bytes.starts_with(CUSTOM_TABLE_MAGIC) {
            return Err(LevelDbError::corruption_at(
                path,
                "custom table header is truncated".to_string(),
            ));
        }
        let version_offset = CUSTOM_TABLE_MAGIC.len();
        let version = u32::from_le_bytes(
            bytes[version_offset..version_offset + 4]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "custom table version is invalid"))?,
        );
        if version != CUSTOM_TABLE_VERSION {
            return Err(LevelDbError::corruption_at(
                path,
                format!("unsupported custom table version {version}"),
            ));
        }
        let compression_tag = bytes[version_offset + 4];
        let crc_offset = version_offset + 5;
        let expected_crc = u32::from_le_bytes(
            bytes[crc_offset..crc_offset + 4]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "custom table crc is invalid"))?,
        );
        let encoded = &bytes[crc_offset + 4..];
        if paranoid_checks && crc32c(encoded) != expected_crc {
            return Err(LevelDbError::corruption_at(
                path,
                "custom table checksum mismatch".to_string(),
            ));
        }
        let payload = Bytes::from(decompress_owned(compression_tag, encoded)?);
        let mut input = payload.as_ref();
        let remaining = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("custom table entry count overflow"))?;
        let offset = payload.len().saturating_sub(input.len());
        Ok(Self {
            path: path.to_path_buf(),
            payload,
            offset,
            remaining,
        })
    }

    fn next(&mut self) -> Result<Option<TableCursorEntry>> {
        if self.remaining == 0 {
            if self.offset != self.payload.len() {
                return Err(LevelDbError::corruption_at(
                    &self.path,
                    "custom table contains trailing bytes".to_string(),
                ));
            }
            return Ok(None);
        }
        let original_offset = self.offset;
        let mut input = &self.payload[original_offset..];
        let key = get_length_prefixed_slice(&mut input)?;
        let value = get_length_prefixed_slice(&mut input)?;
        let consumed = self.payload.len().saturating_sub(original_offset).saturating_sub(input.len());
        let key_owned = key.to_vec();
        let value_start = value.as_ptr() as usize - self.payload.as_ptr() as usize;
        let value_end = value_start.checked_add(value.len()).ok_or_else(|| {
            LevelDbError::corruption_at(&self.path, "custom value range overflow")
        })?;
        if value_end > self.payload.len() {
            return Err(LevelDbError::corruption_at(
                &self.path,
                "custom value range exceeds payload".to_string(),
            ));
        }
        self.offset = original_offset.saturating_add(consumed);
        self.remaining = self.remaining.saturating_sub(1);
        Ok(Some(TableCursorEntry {
            key: key_owned,
            value: Some(self.payload.slice(value_start..value_end)),
        }))
    }
}

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

struct NativeCursor {
    path: PathBuf,
    file: File,
    paranoid_checks: bool,
    handles: Vec<BlockHandle>,
    handle_index: usize,
    block: Option<BlockDecoder>,
    previous_user_key: Vec<u8>,
    read_scratch: Vec<u8>,
}

impl NativeCursor {
    fn open(file: File, path: &Path, paranoid_checks: bool) -> Result<Self> {
        let footer = read_footer(&file, path)?;
        let magic_offset = LEVELDB_FOOTER_LEN - 8;
        let magic = u64::from_le_bytes(
            footer[magic_offset..]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "native footer magic is invalid"))?,
        );
        if magic != LEVELDB_TABLE_MAGIC {
            return Err(LevelDbError::corruption_at(
                path,
                "native table magic mismatch".to_string(),
            ));
        }
        let mut footer_input = &footer[..magic_offset];
        let _meta_index = read_block_handle(&mut footer_input)?;
        let index_handle = read_block_handle(&mut footer_input)?;
        let mut scratch = Vec::new();
        let index_block = read_block_owned(
            &file,
            path,
            index_handle,
            paranoid_checks,
            &mut scratch,
        )?;
        let mut index_decoder = BlockDecoder::new(index_block)?;
        let mut handles = Vec::new();
        while let Some((_key, value)) = index_decoder.next()? {
            let mut input = value.as_ref();
            handles.push(read_block_handle(&mut input)?);
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            paranoid_checks,
            handles,
            handle_index: 0,
            block: None,
            previous_user_key: Vec::with_capacity(32),
            read_scratch: scratch,
        })
    }

    fn next(&mut self) -> Result<Option<TableCursorEntry>> {
        loop {
            if let Some(block) = &mut self.block {
                while let Some((internal_key, value)) = block.next()? {
                    let Some((user_key, is_value)) = split_internal_key(&internal_key) else {
                        continue;
                    };
                    if self.previous_user_key.as_slice() == user_key {
                        continue;
                    }
                    self.previous_user_key.clear();
                    self.previous_user_key.extend_from_slice(user_key);
                    return Ok(Some(TableCursorEntry {
                        key: user_key.to_vec(),
                        value: is_value.then_some(value),
                    }));
                }
                self.block = None;
            }

            let Some(handle) = self.handles.get(self.handle_index).copied() else {
                return Ok(None);
            };
            self.handle_index = self.handle_index.saturating_add(1);
            let block = read_block_owned(
                &self.file,
                &self.path,
                handle,
                self.paranoid_checks,
                &mut self.read_scratch,
            )?;
            self.block = Some(BlockDecoder::new(block)?);
        }
    }
}

struct BlockDecoder {
    block: Bytes,
    entries_end: usize,
    offset: usize,
    key: Vec<u8>,
}

impl BlockDecoder {
    fn new(block: Bytes) -> Result<Self> {
        let entries_end = block_entries_end(&block)?;
        Ok(Self {
            block,
            entries_end,
            offset: 0,
            key: Vec::with_capacity(32),
        })
    }

    fn next(&mut self) -> Result<Option<(Vec<u8>, Bytes)>> {
        if self.offset >= self.entries_end {
            return Ok(None);
        }
        let start_offset = self.offset;
        let mut input = &self.block[start_offset..self.entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block shared key length overflow")
        })?;
        let non_shared = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block key delta length overflow")
        })?;
        let value_len = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block value length overflow")
        })?;
        if shared > self.key.len() {
            return Err(LevelDbError::corruption(
                "native block shared prefix exceeds previous key".to_string(),
            ));
        }
        if input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption(
                "native block entry is truncated".to_string(),
            ));
        }
        self.key.truncate(shared);
        self.key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value_start = self.entries_end.saturating_sub(input.len());
        let value_end = value_start.checked_add(value_len).ok_or_else(|| {
            LevelDbError::corruption("native block value range overflow")
        })?;
        input = &input[value_len..];
        self.offset = self.entries_end.saturating_sub(input.len());
        Ok(Some((
            self.key.clone(),
            self.block.slice(value_start..value_end),
        )))
    }
}

fn read_footer(file: &File, path: &Path) -> Result<[u8; LEVELDB_FOOTER_LEN]> {
    let file_len = file
        .metadata()
        .map_err(|error| LevelDbError::io_at("stat native table", path, error))?
        .len();
    if file_len < LEVELDB_FOOTER_LEN as u64 {
        return Err(LevelDbError::corruption_at(path, "native table is truncated"));
    }
    let mut footer = [0_u8; LEVELDB_FOOTER_LEN];
    read_exact_at(
        file,
        &mut footer,
        file_len.saturating_sub(LEVELDB_FOOTER_LEN as u64),
    )
    .map_err(|error| LevelDbError::io_at("read native table footer", path, error))?;
    Ok(footer)
}

fn read_block_owned(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    scratch: &mut Vec<u8>,
) -> Result<Bytes> {
    let size = usize::try_from(handle.size).map_err(|_| {
        LevelDbError::corruption_at(path, "native block size overflows usize")
    })?;
    let total_size = size.checked_add(LEVELDB_BLOCK_TRAILER_LEN).ok_or_else(|| {
        LevelDbError::corruption_at(path, "native block trailer range overflow")
    })?;
    scratch.clear();
    scratch.resize(total_size, 0);
    read_exact_at(file, scratch, handle.offset)
        .map_err(|error| LevelDbError::io_at("read native table block", path, error))?;
    let payload = &scratch[..size];
    let compression_tag = scratch[size];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            scratch[size + 1..size + LEVELDB_BLOCK_TRAILER_LEN]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "native block crc is invalid"))?,
        );
        let actual_crc = masked_crc32c(&[payload, &[compression_tag]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption_at(
                path,
                format!("native block checksum mismatch at offset {}", handle.offset),
            ));
        }
    }
    if compression_tag == COMPRESSION_NONE {
        Ok(Bytes::copy_from_slice(payload))
    } else {
        Ok(Bytes::from(decompress_owned(compression_tag, payload)?))
    }
}

fn block_entries_end(block: &[u8]) -> Result<usize> {
    if block.len() < 4 {
        return Err(LevelDbError::corruption("native block is truncated"));
    }
    let count_offset = block.len() - 4;
    let restart_count = usize::try_from(u32::from_le_bytes(
        block[count_offset..]
            .try_into()
            .map_err(|_| LevelDbError::corruption("native restart count is invalid"))?,
    ))
    .map_err(|_| LevelDbError::corruption("native restart count overflow"))?;
    let restart_bytes = restart_count.checked_mul(4).ok_or_else(|| {
        LevelDbError::corruption("native restart array overflow")
    })?;
    if restart_bytes > count_offset {
        return Err(LevelDbError::corruption(
            "native restart array is truncated".to_string(),
        ));
    }
    Ok(count_offset - restart_bytes)
}

fn read_block_handle(input: &mut &[u8]) -> Result<BlockHandle> {
    Ok(BlockHandle {
        offset: get_varint64(input)?,
        size: get_varint64(input)?,
    })
}

fn split_internal_key(internal_key: &[u8]) -> Option<(&[u8], bool)> {
    let user_len = internal_key.len().checked_sub(8)?;
    let user_key = internal_key.get(..user_len)?;
    let trailer: [u8; 8] = internal_key.get(user_len..)?.try_into().ok()?;
    let tag = u64::from_le_bytes(trailer);
    match (tag & 0xff) as u8 {
        crate::coding::VALUE_TYPE_VALUE => Some((user_key, true)),
        crate::coding::VALUE_TYPE_DELETION => Some((user_key, false)),
        _ => None,
    }
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buffer)
}

fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buffer.is_empty() {
        match read_at(file, buffer, offset)? {
            0 => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "failed to fill positional read buffer",
                ));
            }
            read => {
                offset = offset.saturating_add(read as u64);
                buffer = &mut buffer[read..];
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_table_writer::NativeTableWriter;
    use crate::options::CompressionPolicy;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_table_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-cursor-{name}-{}.ldb",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn native_cursor_streams_ordered_values_and_tombstones() {
        let path = temp_table_path("native");
        let mut writer = NativeTableWriter::create(&path, 4, CompressionPolicy::None)
            .expect("create writer");
        writer.push(b"a", Some(b"one")).expect("a");
        writer.push(b"b", None).expect("b");
        writer.push(b"c", Some(b"three")).expect("c");
        writer.finish().expect("finish");

        let mut cursor = TableCursor::open(&path, true).expect("open cursor");
        let first = cursor.next().expect("next").expect("first");
        let second = cursor.next().expect("next").expect("second");
        let third = cursor.next().expect("next").expect("third");
        assert_eq!(first.key, b"a");
        assert_eq!(first.value.as_deref(), Some(b"one".as_slice()));
        assert_eq!(second.key, b"b");
        assert!(second.value.is_none());
        assert_eq!(third.key, b"c");
        assert_eq!(third.value.as_deref(), Some(b"three".as_slice()));
        assert!(cursor.next().expect("end").is_none());
        std::fs::remove_file(path).expect("cleanup");
    }
}

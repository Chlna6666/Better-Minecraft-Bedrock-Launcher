use crate::coding::{crc32c, get_length_prefixed_slice, get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, decompress_into};
use crate::error::{LevelDbError, Result};
use bytes::Bytes;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
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

/// Sequential SSTable cursor used by compaction, repair and visibility scans.
///
/// The hot API is [`TableCursor::next_key_into`]. The caller owns and reuses the
/// key buffer; the current value is borrowed through [`TableCursor::current_value`]
/// until the next cursor advance. Native cursors keep reusable encoded and decoded
/// block buffers, so scanning compressed SSTs does not allocate a new block buffer
/// or a `Bytes` slice for every record.
pub(crate) struct TableCursor {
    inner: CursorKind,
}

enum CursorKind {
    Custom(CustomCursor),
    Native(NativeCursor),
}

impl TableCursor {
    pub(crate) fn open(path: &Path, paranoid_checks: bool) -> Result<Self> {
        Self::open_range(path, paranoid_checks, None, None)
    }

    /// Opens a table cursor restricted to `[lower, upper)` user keys.
    ///
    /// Native tables seek to the first candidate data block through the SST index.
    /// Custom legacy tables retain their linear representation but still stop at
    /// the upper bound.
    pub(crate) fn open_range(
        path: &Path,
        paranoid_checks: bool,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
    ) -> Result<Self> {
        let file = File::open(path)
            .map_err(|error| LevelDbError::io_at("open table cursor", path, error))?;
        let mut magic = [0_u8; CUSTOM_TABLE_MAGIC.len()];
        let read = read_at(&file, &mut magic, 0)
            .map_err(|error| LevelDbError::io_at("read table cursor header", path, error))?;
        let inner = if read == CUSTOM_TABLE_MAGIC.len() && magic == *CUSTOM_TABLE_MAGIC {
            CursorKind::Custom(CustomCursor::open(
                file,
                path,
                paranoid_checks,
                lower,
                upper,
            )?)
        } else {
            CursorKind::Native(NativeCursor::open(
                file,
                path,
                paranoid_checks,
                lower,
                upper,
            )?)
        };
        Ok(Self { inner })
    }

    /// Advances to the next visible user key and writes it into caller-owned
    /// reusable storage. Returns whether the current record contains a value;
    /// `false` represents a tombstone.
    pub(crate) fn next_key_into(&mut self, key: &mut Vec<u8>) -> Result<Option<bool>> {
        match &mut self.inner {
            CursorKind::Custom(cursor) => cursor.next_key_into(key),
            CursorKind::Native(cursor) => cursor.next_key_into(key),
        }
    }

    /// Borrows the current value until the next call to [`Self::next_key_into`].
    pub(crate) fn current_value(&self) -> Option<&[u8]> {
        match &self.inner {
            CursorKind::Custom(cursor) => cursor.current_value(),
            CursorKind::Native(cursor) => cursor.current_value(),
        }
    }

    /// Compatibility API for callers that still need a stable owned/shared value.
    /// Hot scan/compaction paths should use `next_key_into + current_value`.
    pub(crate) fn next_into(&mut self, key: &mut Vec<u8>) -> Result<Option<Option<Bytes>>> {
        let Some(is_value) = self.next_key_into(key)? else {
            return Ok(None);
        };
        if !is_value {
            return Ok(Some(None));
        }
        let value = self.current_value().ok_or_else(|| {
            LevelDbError::corruption("table cursor value metadata is missing".to_string())
        })?;
        Ok(Some(Some(Bytes::copy_from_slice(value))))
    }

    pub(crate) fn next(&mut self) -> Result<Option<TableCursorEntry>> {
        let mut key = Vec::with_capacity(48);
        let Some(value) = self.next_into(&mut key)? else {
            return Ok(None);
        };
        Ok(Some(TableCursorEntry { key, value }))
    }
}

struct CustomCursor {
    path: PathBuf,
    payload: Vec<u8>,
    offset: usize,
    remaining: usize,
    lower: Option<Vec<u8>>,
    upper: Option<Vec<u8>>,
    current_value: Option<Range<usize>>,
    exhausted: bool,
}

impl CustomCursor {
    fn open(
        mut file: File,
        path: &Path,
        paranoid_checks: bool,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
    ) -> Result<Self> {
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
        let mut payload = Vec::new();
        decompress_into(compression_tag, encoded, &mut payload)?;
        let mut input = payload.as_slice();
        let remaining = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("custom table entry count overflow"))?;
        let offset = payload.len().saturating_sub(input.len());
        Ok(Self {
            path: path.to_path_buf(),
            payload,
            offset,
            remaining,
            lower: lower.map(<[u8]>::to_vec),
            upper: upper.map(<[u8]>::to_vec),
            current_value: None,
            exhausted: false,
        })
    }

    fn next_key_into(&mut self, key_out: &mut Vec<u8>) -> Result<Option<bool>> {
        self.current_value = None;
        if self.exhausted {
            return Ok(None);
        }
        loop {
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
            let consumed = self
                .payload
                .len()
                .saturating_sub(original_offset)
                .saturating_sub(input.len());
            let value_start = (value.as_ptr() as usize)
                .checked_sub(self.payload.as_ptr() as usize)
                .ok_or_else(|| {
                    LevelDbError::corruption_at(&self.path, "custom value pointer precedes payload")
                })?;
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

            if self.lower.as_deref().is_some_and(|lower| key < lower) {
                continue;
            }
            if self.upper.as_deref().is_some_and(|upper| key >= upper) {
                self.exhausted = true;
                return Ok(None);
            }
            key_out.clear();
            key_out.extend_from_slice(key);
            self.current_value = Some(value_start..value_end);
            return Ok(Some(true));
        }
    }

    fn current_value(&self) -> Option<&[u8]> {
        let range = self.current_value.clone()?;
        self.payload.get(range)
    }
}

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

struct NativeIndexEntry {
    largest_user_key: Vec<u8>,
    handle: BlockHandle,
}

struct NativeCursor {
    path: PathBuf,
    file: File,
    paranoid_checks: bool,
    index: Vec<NativeIndexEntry>,
    handle_index: usize,
    decoder: Option<BlockDecoder>,
    block: Vec<u8>,
    previous_user_key: Vec<u8>,
    read_scratch: Vec<u8>,
    lower: Option<Vec<u8>>,
    upper: Option<Vec<u8>>,
    current_is_value: bool,
    exhausted: bool,
}

impl NativeCursor {
    fn open(
        file: File,
        path: &Path,
        paranoid_checks: bool,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
    ) -> Result<Self> {
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
        let mut read_scratch = Vec::new();
        let mut index_block = Vec::new();
        read_block_reused(
            &file,
            path,
            index_handle,
            paranoid_checks,
            &mut read_scratch,
            &mut index_block,
        )?;
        let mut index_decoder = BlockDecoder::new(&index_block)?;
        let mut index = Vec::new();
        while let Some(entry) = index_decoder.next(&index_block)? {
            let mut input = entry.value(&index_block);
            let handle = read_block_handle(&mut input)?;
            let largest_user_key = split_internal_key(entry.internal_key)
                .map_or_else(|| entry.internal_key.to_vec(), |(key, _)| key.to_vec());
            index.push(NativeIndexEntry {
                largest_user_key,
                handle,
            });
        }
        let handle_index = lower.map_or(0, |lower| {
            index.partition_point(|entry| entry.largest_user_key.as_slice() < lower)
        });
        Ok(Self {
            path: path.to_path_buf(),
            file,
            paranoid_checks,
            index,
            handle_index,
            decoder: None,
            block: index_block,
            previous_user_key: Vec::with_capacity(48),
            read_scratch,
            lower: lower.map(<[u8]>::to_vec),
            upper: upper.map(<[u8]>::to_vec),
            current_is_value: false,
            exhausted: false,
        })
    }

    fn next_key_into(&mut self, key_out: &mut Vec<u8>) -> Result<Option<bool>> {
        self.current_is_value = false;
        if self.exhausted {
            return Ok(None);
        }
        loop {
            if self.decoder.is_some() {
                loop {
                    let entry = {
                        let decoder = self.decoder.as_mut().expect("decoder checked above");
                        decoder.next(&self.block)?
                    };
                    let Some(entry) = entry else {
                        self.decoder = None;
                        break;
                    };
                    let Some((user_key, is_value)) = split_internal_key(entry.internal_key) else {
                        continue;
                    };
                    if self.previous_user_key.as_slice() == user_key {
                        continue;
                    }
                    self.previous_user_key.clear();
                    self.previous_user_key.extend_from_slice(user_key);
                    if self.lower.as_deref().is_some_and(|lower| user_key < lower) {
                        continue;
                    }
                    if self.upper.as_deref().is_some_and(|upper| user_key >= upper) {
                        self.exhausted = true;
                        self.decoder = None;
                        return Ok(None);
                    }
                    key_out.clear();
                    key_out.extend_from_slice(user_key);
                    self.current_is_value = is_value;
                    return Ok(Some(is_value));
                }
            }

            let Some(handle) = self.index.get(self.handle_index).map(|entry| entry.handle) else {
                return Ok(None);
            };
            self.handle_index = self.handle_index.saturating_add(1);
            read_block_reused(
                &self.file,
                &self.path,
                handle,
                self.paranoid_checks,
                &mut self.read_scratch,
                &mut self.block,
            )?;
            self.decoder = Some(BlockDecoder::new(&self.block)?);
        }
    }

    fn current_value(&self) -> Option<&[u8]> {
        if !self.current_is_value {
            return None;
        }
        self.decoder.as_ref()?.current_value(&self.block)
    }
}

struct DecodedEntry<'a> {
    internal_key: &'a [u8],
    value_range: Range<usize>,
}

impl DecodedEntry<'_> {
    fn value<'a>(&self, block: &'a [u8]) -> &'a [u8] {
        block.get(self.value_range.clone()).unwrap_or(&[])
    }
}

struct BlockDecoder {
    entries_end: usize,
    offset: usize,
    key: Vec<u8>,
    current_value: Option<Range<usize>>,
}

impl BlockDecoder {
    fn new(block: &[u8]) -> Result<Self> {
        let entries_end = block_entries_end(block)?;
        Ok(Self {
            entries_end,
            offset: 0,
            key: Vec::with_capacity(48),
            current_value: None,
        })
    }

    fn next<'a>(&'a mut self, block: &[u8]) -> Result<Option<DecodedEntry<'a>>> {
        self.current_value = None;
        if self.offset >= self.entries_end {
            return Ok(None);
        }
        let mut input = &block[self.offset..self.entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block value length overflow"))?;
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
        let value_end = value_start
            .checked_add(value_len)
            .ok_or_else(|| LevelDbError::corruption("native block value range overflow"))?;
        input = &input[value_len..];
        self.offset = self.entries_end.saturating_sub(input.len());
        let value_range = value_start..value_end;
        self.current_value = Some(value_range.clone());
        Ok(Some(DecodedEntry {
            internal_key: &self.key,
            value_range,
        }))
    }

    fn current_value<'a>(&self, block: &'a [u8]) -> Option<&'a [u8]> {
        block.get(self.current_value.clone()?)
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

fn read_block_reused(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    encoded: &mut Vec<u8>,
    decoded: &mut Vec<u8>,
) -> Result<()> {
    let size = usize::try_from(handle.size)
        .map_err(|_| LevelDbError::corruption_at(path, "native block size overflows usize"))?;
    let total_size = size.checked_add(LEVELDB_BLOCK_TRAILER_LEN).ok_or_else(|| {
        LevelDbError::corruption_at(path, "native block trailer range overflow")
    })?;
    encoded.clear();
    encoded.resize(total_size, 0);
    read_exact_at(file, encoded, handle.offset)
        .map_err(|error| LevelDbError::io_at("read native table block", path, error))?;
    let compression_tag = encoded[size];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            encoded[size + 1..size + LEVELDB_BLOCK_TRAILER_LEN]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "native block crc is invalid"))?,
        );
        let actual_crc = masked_crc32c(&[&encoded[..size], &[compression_tag]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption_at(
                path,
                format!("native block checksum mismatch at offset {}", handle.offset),
            ));
        }
    }
    if compression_tag == COMPRESSION_NONE {
        // Swap the freshly read allocation into the decoded slot. The old decoded
        // allocation becomes the next encoded read buffer, so uncompressed scans
        // avoid copying the payload while still reusing both allocations.
        std::mem::swap(encoded, decoded);
        decoded.truncate(size);
        return Ok(());
    }
    decompress_into(compression_tag, &encoded[..size], decoded)
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
    let restart_bytes = restart_count
        .checked_mul(4)
        .ok_or_else(|| LevelDbError::corruption("native restart array overflow"))?;
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

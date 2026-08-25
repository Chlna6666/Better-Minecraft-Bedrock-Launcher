use crate::bloom::{BloomFilterBlockBuilder, FILTER_META_KEY};
use crate::coding::{
    VALUE_TYPE_DELETION, VALUE_TYPE_VALUE, masked_crc32c, put_varint32, put_varint64,
};
use crate::compression::{COMPRESSION_NONE, compression_tag, with_compressed};
use crate::error::{LevelDbError, Result};
use crate::options::CompressionPolicy;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;
const NATIVE_DATA_BLOCK_TARGET: usize = 4 * 1024;
const NATIVE_RESTART_INTERVAL: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrittenNativeTable {
    pub(crate) file_size: u64,
    pub(crate) smallest_internal_key: Vec<u8>,
    pub(crate) largest_internal_key: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

/// Incremental LevelDB SSTable writer.
///
/// Entries must be supplied in strictly increasing user-key order. Data blocks,
/// compression buffers, Bloom hashes and key-delta buffers are reused for the
/// full table. The emitted filter block/metaindex follow the standard LevelDB
/// `leveldb.BuiltinBloomFilter2` table format.
pub(crate) struct NativeTableWriter {
    path: PathBuf,
    tmp_path: PathBuf,
    writer: Option<BufWriter<File>>,
    sequence: u64,
    compression: CompressionPolicy,
    compression_tag: u8,
    file_offset: u64,
    data_block: NativeBlockBuilder,
    filter_block: BloomFilterBlockBuilder,
    meta_index_block: NativeBlockBuilder,
    index_block: NativeBlockBuilder,
    internal_key: Vec<u8>,
    last_user_key: Vec<u8>,
    handle_bytes: Vec<u8>,
    footer_handles: Vec<u8>,
    smallest_internal_key: Option<Vec<u8>>,
    entry_count: usize,
    committed: bool,
}

impl NativeTableWriter {
    pub(crate) fn create(
        path: &Path,
        sequence: u64,
        compression: CompressionPolicy,
    ) -> Result<Self> {
        let tmp_path = path.with_extension("ldbtmp");
        let file = File::create(&tmp_path).map_err(|error| {
            LevelDbError::io_at("create native table temp file", &tmp_path, error)
        })?;
        let mut filter_block = BloomFilterBlockBuilder::new();
        filter_block.start_block(0)?;
        Ok(Self {
            path: path.to_path_buf(),
            tmp_path,
            writer: Some(BufWriter::new(file)),
            sequence,
            compression,
            compression_tag: compression_tag(compression),
            file_offset: 0,
            data_block: NativeBlockBuilder::new(NATIVE_DATA_BLOCK_TARGET),
            filter_block,
            meta_index_block: NativeBlockBuilder::new(128),
            index_block: NativeBlockBuilder::new(1024),
            internal_key: Vec::with_capacity(32),
            last_user_key: Vec::with_capacity(32),
            handle_bytes: Vec::with_capacity(20),
            footer_handles: Vec::with_capacity(40),
            smallest_internal_key: None,
            entry_count: 0,
            committed: false,
        })
    }

    #[must_use]
    pub(crate) const fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    #[must_use]
    pub(crate) const fn entry_count(&self) -> usize {
        self.entry_count
    }

    #[must_use]
    pub(crate) fn estimated_size(&self) -> u64 {
        self.file_offset
            .saturating_add(self.data_block.estimated_finished_size() as u64)
            .saturating_add(self.filter_block.estimated_size() as u64)
            .saturating_add(self.meta_index_block.estimated_finished_size() as u64)
            .saturating_add(self.index_block.estimated_finished_size() as u64)
            .saturating_add((3 * LEVELDB_BLOCK_TRAILER_LEN + LEVELDB_FOOTER_LEN) as u64)
    }

    pub(crate) fn push(&mut self, user_key: &[u8], value: Option<&[u8]>) -> Result<()> {
        if !self.last_user_key.is_empty() && user_key <= self.last_user_key.as_slice() {
            return Err(LevelDbError::invalid_argument(
                "native table entries must be strictly ordered by user key".to_string(),
            ));
        }

        let value_type = if value.is_some() {
            VALUE_TYPE_VALUE
        } else {
            VALUE_TYPE_DELETION
        };
        let value = value.unwrap_or_default();
        encode_internal_key(user_key, self.sequence, value_type, &mut self.internal_key);

        if !self.data_block.is_empty()
            && self
                .data_block
                .estimated_size_after(&self.internal_key, value)
                > NATIVE_DATA_BLOCK_TARGET
        {
            self.flush_data_block()?;
            self.filter_block.start_block(self.file_offset)?;
        }

        if self.smallest_internal_key.is_none() {
            self.smallest_internal_key = Some(self.internal_key.clone());
        }
        self.filter_block.add_key(user_key)?;
        self.data_block.add(&self.internal_key, value)?;
        self.last_user_key.clear();
        self.last_user_key.extend_from_slice(user_key);
        self.entry_count = self.entry_count.saturating_add(1);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<WrittenNativeTable> {
        if self.is_empty() {
            return Err(LevelDbError::invalid_argument(
                "native table writer requires at least one entry".to_string(),
            ));
        }

        self.flush_data_block()?;

        let filter_handle = {
            let raw = self.filter_block.finish()?;
            let writer = self.writer.as_mut().ok_or_else(writer_closed)?;
            write_native_block(
                writer,
                &mut self.file_offset,
                raw,
                CompressionPolicy::None,
                COMPRESSION_NONE,
            )?
        };

        self.handle_bytes.clear();
        write_block_handle(filter_handle, &mut self.handle_bytes);
        self.meta_index_block
            .add(FILTER_META_KEY, &self.handle_bytes)?;
        self.meta_index_block.finish_in_place()?;
        let meta_index_handle = {
            let raw = self.meta_index_block.bytes();
            let writer = self.writer.as_mut().ok_or_else(writer_closed)?;
            write_native_block(
                writer,
                &mut self.file_offset,
                raw,
                CompressionPolicy::None,
                COMPRESSION_NONE,
            )?
        };

        self.index_block.finish_in_place()?;
        let index_handle = {
            let raw = self.index_block.bytes();
            let writer = self.writer.as_mut().ok_or_else(writer_closed)?;
            write_native_block(
                writer,
                &mut self.file_offset,
                raw,
                CompressionPolicy::None,
                COMPRESSION_NONE,
            )?
        };

        let footer = native_footer(meta_index_handle, index_handle, &mut self.footer_handles);
        let tmp_path = self.tmp_path.clone();
        let mut writer = self.writer.take().ok_or_else(writer_closed)?;
        writer
            .write_all(&footer)
            .map_err(|error| LevelDbError::io_at("write native table footer", &tmp_path, error))?;
        self.file_offset = self.file_offset.saturating_add(LEVELDB_FOOTER_LEN as u64);
        writer
            .flush()
            .map_err(|error| LevelDbError::io_at("flush native table", &tmp_path, error))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| LevelDbError::io_at("sync native table", &tmp_path, error))?;
        drop(writer);
        replace_file(&self.tmp_path, &self.path)?;
        self.committed = true;

        let smallest_internal_key = self.smallest_internal_key.take().ok_or_else(|| {
            LevelDbError::invalid_argument("native table has no smallest key".to_string())
        })?;
        let largest_internal_key = self.internal_key.clone();

        Ok(WrittenNativeTable {
            file_size: self.file_offset,
            smallest_internal_key,
            largest_internal_key,
        })
    }

    fn flush_data_block(&mut self) -> Result<()> {
        if self.data_block.is_empty() {
            return Ok(());
        }
        self.data_block.finish_in_place()?;

        let handle = {
            let raw = self.data_block.bytes();
            let writer = self.writer.as_mut().ok_or_else(writer_closed)?;
            write_native_block(
                writer,
                &mut self.file_offset,
                raw,
                self.compression,
                self.compression_tag,
            )?
        };

        self.handle_bytes.clear();
        write_block_handle(handle, &mut self.handle_bytes);
        self.index_block
            .add(self.data_block.last_key(), &self.handle_bytes)?;
        self.data_block.reset();
        Ok(())
    }
}

impl Drop for NativeTableWriter {
    fn drop(&mut self) {
        if !self.committed {
            self.writer.take();
            if let Err(error) = fs::remove_file(&self.tmp_path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::debug!(
                    "failed to remove abandoned SSTable temp file {}: {}",
                    self.tmp_path.display(),
                    error
                );
            }
        }
    }
}

fn writer_closed() -> LevelDbError {
    LevelDbError::invalid_argument("native table writer is already finished".to_string())
}

fn write_native_block(
    writer: &mut impl Write,
    file_offset: &mut u64,
    raw: &[u8],
    compression: CompressionPolicy,
    compression_tag: u8,
) -> Result<BlockHandle> {
    with_compressed(compression, raw, |encoded| {
        let handle = BlockHandle {
            offset: *file_offset,
            size: u64::try_from(encoded.len()).map_err(|_| {
                LevelDbError::invalid_argument("native block is too large".to_string())
            })?,
        };
        writer.write_all(encoded)?;
        let mut trailer = [0_u8; LEVELDB_BLOCK_TRAILER_LEN];
        trailer[0] = compression_tag;
        trailer[1..].copy_from_slice(&masked_crc32c(&[encoded, &[compression_tag]]).to_le_bytes());
        writer.write_all(&trailer)?;
        *file_offset = file_offset
            .saturating_add(handle.size)
            .saturating_add(LEVELDB_BLOCK_TRAILER_LEN as u64);
        Ok(handle)
    })
}

struct NativeBlockBuilder {
    data: Vec<u8>,
    restarts: Vec<u32>,
    previous_key: Vec<u8>,
    entries_since_restart: usize,
    finished: bool,
}

impl NativeBlockBuilder {
    fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            restarts: Vec::with_capacity(32),
            previous_key: Vec::with_capacity(32),
            entries_since_restart: NATIVE_RESTART_INTERVAL,
            finished: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.previous_key.is_empty() && self.data.is_empty()
    }

    fn last_key(&self) -> &[u8] {
        &self.previous_key
    }

    fn bytes(&self) -> &[u8] {
        &self.data
    }

    fn estimated_finished_size(&self) -> usize {
        self.data
            .len()
            .saturating_add(self.restarts.len().max(1).saturating_mul(4))
            .saturating_add(4)
    }

    fn estimated_size_after(&self, key: &[u8], value: &[u8]) -> usize {
        self.data
            .len()
            .saturating_add(key.len())
            .saturating_add(value.len())
            .saturating_add(15)
            .saturating_add((self.restarts.len() + 1).saturating_mul(4))
            .saturating_add(4)
    }

    fn add(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if self.finished {
            return Err(LevelDbError::invalid_argument(
                "cannot append to a finished native block".to_string(),
            ));
        }
        let restart = self.entries_since_restart >= NATIVE_RESTART_INTERVAL;
        let shared = if restart {
            self.restarts
                .push(u32::try_from(self.data.len()).map_err(|_| {
                    LevelDbError::invalid_argument("native block offset exceeds u32".to_string())
                })?);
            self.entries_since_restart = 0;
            0
        } else {
            common_prefix_len(&self.previous_key, key)
        };
        let non_shared = &key[shared..];
        put_varint32(
            u32::try_from(shared).map_err(|_| {
                LevelDbError::invalid_argument("native shared key length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        put_varint32(
            u32::try_from(non_shared.len()).map_err(|_| {
                LevelDbError::invalid_argument("native key length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        put_varint32(
            u32::try_from(value.len()).map_err(|_| {
                LevelDbError::invalid_argument("native value length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        self.data.extend_from_slice(non_shared);
        self.data.extend_from_slice(value);
        self.previous_key.clear();
        self.previous_key.extend_from_slice(key);
        self.entries_since_restart = self.entries_since_restart.saturating_add(1);
        Ok(())
    }

    fn finish_in_place(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if self.restarts.is_empty() {
            self.restarts.push(0);
        }
        for restart in &self.restarts {
            self.data.extend_from_slice(&restart.to_le_bytes());
        }
        self.data.extend_from_slice(
            &u32::try_from(self.restarts.len())
                .map_err(|_| {
                    LevelDbError::invalid_argument("native restart count is too large".to_string())
                })?
                .to_le_bytes(),
        );
        self.finished = true;
        Ok(())
    }

    fn reset(&mut self) {
        self.data.clear();
        self.restarts.clear();
        self.previous_key.clear();
        self.entries_since_restart = NATIVE_RESTART_INTERVAL;
        self.finished = false;
    }
}

fn encode_internal_key(user_key: &[u8], sequence: u64, value_type: u8, out: &mut Vec<u8>) {
    out.clear();
    let required = user_key.len().saturating_add(8);
    if out.capacity() < required {
        out.reserve(required.saturating_sub(out.len()));
    }
    out.extend_from_slice(user_key);
    out.extend_from_slice(&((sequence << 8) | u64::from(value_type)).to_le_bytes());
}

fn common_prefix_len(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

fn write_block_handle(handle: BlockHandle, out: &mut Vec<u8>) {
    put_varint64(handle.offset, out);
    put_varint64(handle.size, out);
}

fn native_footer(
    meta_index: BlockHandle,
    index: BlockHandle,
    handles: &mut Vec<u8>,
) -> [u8; LEVELDB_FOOTER_LEN] {
    handles.clear();
    write_block_handle(meta_index, handles);
    write_block_handle(index, handles);

    let mut footer = [0_u8; LEVELDB_FOOTER_LEN];
    let handle_len = handles.len().min(LEVELDB_FOOTER_LEN - 8);
    footer[..handle_len].copy_from_slice(&handles[..handle_len]);
    footer[LEVELDB_FOOTER_LEN - 8..].copy_from_slice(&LEVELDB_TABLE_MAGIC.to_le_bytes());
    footer
}

fn replace_file(tmp_path: &Path, path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| LevelDbError::io_at("replace table", path, error))?;
    }
    fs::rename(tmp_path, path)
        .map_err(|error| LevelDbError::io_at("rename table temp file", path, error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{TableLookup, get_table_lookup};
    use bytes::Bytes;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_table_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-{name}-{}.ldb",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn incremental_writer_roundtrips_and_reuses_blocks() {
        let path = temp_table_path("incremental-writer");
        let mut writer =
            NativeTableWriter::create(&path, 17, CompressionPolicy::None).expect("create writer");
        for index in 0..256_u16 {
            let key = format!("key:{index:04}");
            let value = vec![u8::try_from(index % 251).expect("byte"); 256];
            writer.push(key.as_bytes(), Some(&value)).expect("push");
        }
        let written = writer.finish().expect("finish");
        assert!(written.file_size > 0);
        assert_eq!(
            get_table_lookup(&path, b"key:0192", true, None).expect("lookup"),
            TableLookup::Value(Bytes::from(vec![192_u8; 256]))
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn incremental_writer_preserves_tombstones() {
        let path = temp_table_path("incremental-tombstone");
        let mut writer =
            NativeTableWriter::create(&path, 9, CompressionPolicy::None).expect("create writer");
        writer.push(b"a", Some(b"one")).expect("put");
        writer.push(b"b", None).expect("delete");
        writer.finish().expect("finish");
        assert_eq!(
            get_table_lookup(&path, b"b", true, None).expect("lookup"),
            TableLookup::Deleted
        );
        std::fs::remove_file(path).expect("cleanup");
    }
}

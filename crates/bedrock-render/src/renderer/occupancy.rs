//! Persistent whole-world chunk-to-tile occupancy index.
//!
//! The index is built from one partitioned LevelDB key-space scan. Tile entries
//! and chunk positions are stored in contiguous vectors so large worlds do not
//! require one heap allocation per tile.

use super::pipeline::{
    LevelDbRenderSource, RenderChunkSource, RenderLayout, RenderTaskControl,
};
use crate::error::{BedrockRenderError, Result};
use bedrock_world::{
    ChunkBounds, ChunkPos, Dimension, WorldScanOptions, WorldThreadingOptions,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TILE_OCCUPANCY_CACHE_MAGIC: &[u8; 8] = b"BROCC001";
const TILE_OCCUPANCY_CACHE_VERSION: u32 = 1;
const FNV1A64_OFFSET: u64 = 0xcbf_29ce_4842_2325;
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;
static TILE_OCCUPANCY_CACHE_WRITE_ID: AtomicU64 = AtomicU64::new(0);

/// One compact tile entry in a [`TileOccupancyIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileOccupancyEntry {
    /// Tile X coordinate.
    pub tile_x: i32,
    /// Tile Z coordinate.
    pub tile_z: i32,
    chunk_start: u32,
    chunk_len: u32,
}

/// Compact immutable occupancy index for one dimension and tile layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileOccupancyIndex {
    dimension: Dimension,
    chunks_per_tile: u32,
    entries: Vec<TileOccupancyEntry>,
    chunks: Vec<ChunkPos>,
    bounds: Option<ChunkBounds>,
}

impl TileOccupancyIndex {
    /// Builds a compact index from renderable chunk positions.
    pub fn from_render_chunk_positions(
        dimension: Dimension,
        chunks_per_tile: u32,
        mut positions: Vec<ChunkPos>,
    ) -> Result<Self> {
        let span = i32::try_from(chunks_per_tile).map_err(|_| {
            BedrockRenderError::Validation("chunks_per_tile exceeds i32".to_string())
        })?;
        if span <= 0 {
            return Err(BedrockRenderError::Validation(
                "chunks_per_tile must be greater than zero".to_string(),
            ));
        }

        positions.retain(|position| position.dimension == dimension);
        positions.sort_unstable_by_key(|position| {
            (
                position.x.div_euclid(span),
                position.z.div_euclid(span),
                position.x,
                position.z,
            )
        });
        positions.dedup();

        let bounds = chunk_bounds(dimension, &positions);
        let mut entries = Vec::new();
        let mut current_tile = None;
        let mut current_start = 0usize;
        for (index, position) in positions.iter().enumerate() {
            let tile = (position.x.div_euclid(span), position.z.div_euclid(span));
            if current_tile == Some(tile) {
                continue;
            }
            if let Some((tile_x, tile_z)) = current_tile {
                entries.push(tile_entry(tile_x, tile_z, current_start, index)?);
            }
            current_tile = Some(tile);
            current_start = index;
        }
        if let Some((tile_x, tile_z)) = current_tile {
            entries.push(tile_entry(tile_x, tile_z, current_start, positions.len())?);
        }
        positions.shrink_to_fit();
        entries.shrink_to_fit();
        Ok(Self {
            dimension,
            chunks_per_tile,
            entries,
            chunks: positions,
            bounds,
        })
    }

    /// Dimension represented by the index.
    #[must_use]
    pub const fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Number of chunks represented by one tile edge.
    #[must_use]
    pub const fn chunks_per_tile(&self) -> u32 {
        self.chunks_per_tile
    }

    /// Discovered chunk bounds.
    #[must_use]
    pub const fn bounds(&self) -> Option<ChunkBounds> {
        self.bounds
    }

    /// Number of occupied tiles.
    #[must_use]
    pub fn tile_count(&self) -> usize {
        self.entries.len()
    }

    /// Number of renderable chunks.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Sorted compact tile entries.
    #[must_use]
    pub fn entries(&self) -> &[TileOccupancyEntry] {
        &self.entries
    }

    /// Exact renderable chunks for a tile, or `None` when the tile is empty.
    #[must_use]
    pub fn chunk_positions(&self, tile_x: i32, tile_z: i32) -> Option<&[ChunkPos]> {
        let index = self
            .entries
            .binary_search_by_key(&(tile_x, tile_z), |entry| (entry.tile_x, entry.tile_z))
            .ok()?;
        let entry = self.entries[index];
        let start = usize::try_from(entry.chunk_start).ok()?;
        let len = usize::try_from(entry.chunk_len).ok()?;
        self.chunks.get(start..start.checked_add(len)?)
    }

    /// Returns whether a tile contains at least one renderable chunk.
    #[must_use]
    pub fn contains_tile(&self, tile_x: i32, tile_z: i32) -> bool {
        self.chunk_positions(tile_x, tile_z).is_some()
    }
}

/// Input for loading or building a persistent tile occupancy index.
#[derive(Debug, Clone)]
pub struct TileOccupancyIndexRequest {
    /// Bedrock world directory.
    pub world_path: PathBuf,
    /// Root shared with the renderer's disk tile cache.
    pub cache_root: PathBuf,
    /// Dimension to index.
    pub dimension: Dimension,
    /// Tile layout. Only `chunks_per_tile` affects occupancy.
    pub layout: RenderLayout,
    /// Whole-key-space scan options.
    pub scan_options: WorldScanOptions,
}

impl TileOccupancyIndexRequest {
    /// Creates a request with automatic table-parallel scanning.
    #[must_use]
    pub fn new(
        world_path: impl Into<PathBuf>,
        cache_root: impl Into<PathBuf>,
        dimension: Dimension,
        layout: RenderLayout,
    ) -> Self {
        Self {
            world_path: world_path.into(),
            cache_root: cache_root.into(),
            dimension,
            layout,
            scan_options: WorldScanOptions {
                threading: WorldThreadingOptions::Auto,
                ..WorldScanOptions::default()
            },
        }
    }
}

/// Source used to produce an occupancy index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileOccupancyIndexSource {
    /// Loaded from the persistent sidecar.
    DiskCache,
    /// Built from one partitioned key-space scan.
    KeySpaceScan,
}

/// Loaded index and its source.
#[derive(Debug, Clone)]
pub struct TileOccupancyIndexResult {
    /// Compact immutable index.
    pub index: TileOccupancyIndex,
    /// Whether the index came from disk or a scan.
    pub source: TileOccupancyIndexSource,
}

/// Loads a validated sidecar or performs one whole-key-space occupancy scan.
///
/// The sidecar uses the same world identity and cache root as rendered tiles, so
/// a world signature change invalidates both cache paths consistently.
pub fn load_or_build_tile_occupancy_index_blocking(
    request: TileOccupancyIndexRequest,
    control: &RenderTaskControl,
) -> Result<TileOccupancyIndexResult> {
    wait_for_control(control)?;
    let identity = super::cache::world_cache_identity(&request.world_path);
    let validation = occupancy_validation(
        &identity.world_id,
        &identity.world_signature,
        request.dimension,
        request.layout.chunks_per_tile,
    );
    let path = tile_occupancy_cache_path(
        &request.cache_root,
        &identity.world_id,
        request.dimension,
        request.layout.chunks_per_tile,
        validation,
    );
    if let Some(index) = load_index(
        &path,
        validation,
        request.dimension,
        request.layout.chunks_per_tile,
    )? {
        return Ok(TileOccupancyIndexResult {
            index,
            source: TileOccupancyIndexSource::DiskCache,
        });
    }

    wait_for_control(control)?;
    let source = LevelDbRenderSource::open_read_only(&request.world_path)?;
    let positions = source.list_render_chunk_positions_blocking(request.scan_options)?;
    wait_for_control(control)?;
    let index = TileOccupancyIndex::from_render_chunk_positions(
        request.dimension,
        request.layout.chunks_per_tile,
        positions,
    )?;
    store_index(&path, validation, &index)?;
    Ok(TileOccupancyIndexResult {
        index,
        source: TileOccupancyIndexSource::KeySpaceScan,
    })
}

/// Returns the persistent sidecar path for an occupancy configuration.
#[must_use]
pub fn tile_occupancy_cache_path(
    cache_root: &Path,
    world_id: &str,
    dimension: Dimension,
    chunks_per_tile: u32,
    validation: u64,
) -> PathBuf {
    cache_root
        .join("map-occupancy-index")
        .join(world_id)
        .join(format!("dimension-{}", dimension.id()))
        .join(format!("{chunks_per_tile}c-v{validation:016x}.brocc"))
}

fn wait_for_control(control: &RenderTaskControl) -> Result<()> {
    while control.is_paused() {
        if control.is_cancelled() {
            return Err(BedrockRenderError::Cancelled);
        }
        thread::sleep(Duration::from_millis(10));
    }
    if control.is_cancelled() {
        Err(BedrockRenderError::Cancelled)
    } else {
        Ok(())
    }
}

fn tile_entry(
    tile_x: i32,
    tile_z: i32,
    start: usize,
    end: usize,
) -> Result<TileOccupancyEntry> {
    Ok(TileOccupancyEntry {
        tile_x,
        tile_z,
        chunk_start: u32::try_from(start).map_err(|_| {
            BedrockRenderError::Validation("occupancy chunk offset exceeds u32".to_string())
        })?,
        chunk_len: u32::try_from(end.saturating_sub(start)).map_err(|_| {
            BedrockRenderError::Validation("occupancy tile chunk count exceeds u32".to_string())
        })?,
    })
}

fn chunk_bounds(dimension: Dimension, positions: &[ChunkPos]) -> Option<ChunkBounds> {
    let first = *positions.first()?;
    let mut min_chunk_x = first.x;
    let mut min_chunk_z = first.z;
    let mut max_chunk_x = first.x;
    let mut max_chunk_z = first.z;
    for position in &positions[1..] {
        min_chunk_x = min_chunk_x.min(position.x);
        min_chunk_z = min_chunk_z.min(position.z);
        max_chunk_x = max_chunk_x.max(position.x);
        max_chunk_z = max_chunk_z.max(position.z);
    }
    Some(ChunkBounds {
        dimension,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
        chunk_count: positions.len(),
    })
}

fn occupancy_validation(
    world_id: &str,
    world_signature: &str,
    dimension: Dimension,
    chunks_per_tile: u32,
) -> u64 {
    let mut hash = FNV1A64_OFFSET;
    fnv_write(&mut hash, &TILE_OCCUPANCY_CACHE_VERSION.to_le_bytes());
    fnv_write(&mut hash, world_id.as_bytes());
    fnv_write(&mut hash, world_signature.as_bytes());
    fnv_write(&mut hash, &dimension.id().to_le_bytes());
    fnv_write(&mut hash, &chunks_per_tile.to_le_bytes());
    if hash == 0 { FNV1A64_OFFSET } else { hash }
}

fn fnv_write(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
}

fn load_index(
    path: &Path,
    validation: u64,
    dimension: Dimension,
    chunks_per_tile: u32,
) -> Result<Option<TileOccupancyIndex>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(BedrockRenderError::io(
                format!("failed to read tile occupancy index {}", path.display()),
                error,
            ));
        }
    };
    match decode_index(&bytes, validation, dimension, chunks_per_tile) {
        Ok(index) => Ok(Some(index)),
        Err(error) => {
            log::warn!(
                "discarding invalid tile occupancy index {}: {}",
                path.display(),
                error
            );
            let _ = fs::remove_file(path);
            Ok(None)
        }
    }
}

fn store_index(path: &Path, validation: u64, index: &TileOccupancyIndex) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            BedrockRenderError::io("failed to create tile occupancy directory", error)
        })?;
    }
    let bytes = encode_index(validation, index);
    let unique = TILE_OCCUPANCY_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temp = path.with_extension(format!("tmp-{}-{timestamp}-{unique}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| BedrockRenderError::io("failed to create occupancy temp file", error))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_data())
        .map_err(|error| BedrockRenderError::io("failed to write occupancy temp file", error))?;
    drop(file);
    if let Err(error) = fs::rename(&temp, path) {
        if path.exists() {
            fs::remove_file(path).map_err(|remove_error| {
                BedrockRenderError::io("failed to replace occupancy index", remove_error)
            })?;
            fs::rename(&temp, path).map_err(|rename_error| {
                BedrockRenderError::io("failed to commit occupancy index", rename_error)
            })?;
        } else {
            return Err(BedrockRenderError::io(
                "failed to commit occupancy index",
                error,
            ));
        }
    }
    Ok(())
}

fn encode_index(validation: u64, index: &TileOccupancyIndex) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        72usize
            .saturating_add(index.entries.len().saturating_mul(16))
            .saturating_add(index.chunks.len().saturating_mul(8)),
    );
    bytes.extend_from_slice(TILE_OCCUPANCY_CACHE_MAGIC);
    put_u32(&mut bytes, TILE_OCCUPANCY_CACHE_VERSION);
    put_u64(&mut bytes, validation);
    put_i32(&mut bytes, index.dimension.id());
    put_u32(&mut bytes, index.chunks_per_tile);
    match index.bounds {
        Some(bounds) => {
            bytes.push(1);
            bytes.extend_from_slice(&[0; 3]);
            put_i32(&mut bytes, bounds.min_chunk_x);
            put_i32(&mut bytes, bounds.min_chunk_z);
            put_i32(&mut bytes, bounds.max_chunk_x);
            put_i32(&mut bytes, bounds.max_chunk_z);
            put_u64(&mut bytes, u64::try_from(bounds.chunk_count).unwrap_or(u64::MAX));
        }
        None => {
            bytes.push(0);
            bytes.extend_from_slice(&[0; 3]);
            bytes.extend_from_slice(&[0; 24]);
        }
    }
    put_u64(&mut bytes, u64::try_from(index.entries.len()).unwrap_or(u64::MAX));
    put_u64(&mut bytes, u64::try_from(index.chunks.len()).unwrap_or(u64::MAX));
    for entry in &index.entries {
        put_i32(&mut bytes, entry.tile_x);
        put_i32(&mut bytes, entry.tile_z);
        put_u32(&mut bytes, entry.chunk_start);
        put_u32(&mut bytes, entry.chunk_len);
    }
    for chunk in &index.chunks {
        put_i32(&mut bytes, chunk.x);
        put_i32(&mut bytes, chunk.z);
    }
    bytes
}

fn decode_index(
    bytes: &[u8],
    expected_validation: u64,
    dimension: Dimension,
    chunks_per_tile: u32,
) -> Result<TileOccupancyIndex> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != TILE_OCCUPANCY_CACHE_MAGIC {
        return Err(BedrockRenderError::Validation(
            "invalid tile occupancy cache magic".to_string(),
        ));
    }
    if cursor.u32()? != TILE_OCCUPANCY_CACHE_VERSION {
        return Err(BedrockRenderError::Validation(
            "unsupported tile occupancy cache version".to_string(),
        ));
    }
    if cursor.u64()? != expected_validation
        || cursor.i32()? != dimension.id()
        || cursor.u32()? != chunks_per_tile
    {
        return Err(BedrockRenderError::Validation(
            "tile occupancy cache identity mismatch".to_string(),
        ));
    }
    let has_bounds = cursor.u8()? != 0;
    cursor.take(3)?;
    let min_chunk_x = cursor.i32()?;
    let min_chunk_z = cursor.i32()?;
    let max_chunk_x = cursor.i32()?;
    let max_chunk_z = cursor.i32()?;
    let bounds_chunk_count = usize::try_from(cursor.u64()?).map_err(|_| {
        BedrockRenderError::Validation("occupancy bounds count overflow".to_string())
    })?;
    let entry_count = usize::try_from(cursor.u64()?).map_err(|_| {
        BedrockRenderError::Validation("occupancy entry count overflow".to_string())
    })?;
    let chunk_count = usize::try_from(cursor.u64()?).map_err(|_| {
        BedrockRenderError::Validation("occupancy chunk count overflow".to_string())
    })?;
    let required = entry_count
        .checked_mul(16)
        .and_then(|value| value.checked_add(chunk_count.checked_mul(8)?))
        .ok_or_else(|| BedrockRenderError::Validation("occupancy size overflow".to_string()))?;
    if cursor.remaining() != required {
        return Err(BedrockRenderError::Validation(
            "tile occupancy cache length mismatch".to_string(),
        ));
    }
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        entries.push(TileOccupancyEntry {
            tile_x: cursor.i32()?,
            tile_z: cursor.i32()?,
            chunk_start: cursor.u32()?,
            chunk_len: cursor.u32()?,
        });
    }
    if !entries.windows(2).all(|window| {
        (window[0].tile_x, window[0].tile_z) < (window[1].tile_x, window[1].tile_z)
    }) {
        return Err(BedrockRenderError::Validation(
            "tile occupancy entries are not strictly sorted".to_string(),
        ));
    }
    let mut chunks = Vec::with_capacity(chunk_count);
    for _ in 0..chunk_count {
        chunks.push(ChunkPos {
            x: cursor.i32()?,
            z: cursor.i32()?,
            dimension,
        });
    }
    for entry in &entries {
        let start = usize::try_from(entry.chunk_start).map_err(|_| {
            BedrockRenderError::Validation("occupancy chunk start overflow".to_string())
        })?;
        let len = usize::try_from(entry.chunk_len).map_err(|_| {
            BedrockRenderError::Validation("occupancy chunk len overflow".to_string())
        })?;
        if start.checked_add(len).is_none_or(|end| end > chunks.len()) {
            return Err(BedrockRenderError::Validation(
                "occupancy entry references chunks outside the blob".to_string(),
            ));
        }
    }
    let bounds = has_bounds.then_some(ChunkBounds {
        dimension,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
        chunk_count: bounds_chunk_count,
    });
    if bounds.map(|value| value.chunk_count).unwrap_or(0) != chunks.len() {
        return Err(BedrockRenderError::Validation(
            "occupancy bounds count does not match chunks".to_string(),
        ));
    }
    Ok(TileOccupancyIndex {
        dimension,
        chunks_per_tile,
        entries,
        chunks,
        bounds,
    })
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| BedrockRenderError::Validation("occupancy offset overflow".to_string()))?;
        let slice = self.bytes.get(self.offset..end).ok_or_else(|| {
            BedrockRenderError::Validation("truncated tile occupancy cache".to_string())
        })?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| {
            BedrockRenderError::Validation("invalid occupancy u32".to_string())
        })?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().map_err(|_| {
            BedrockRenderError::Validation("invalid occupancy i32".to_string())
        })?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| {
            BedrockRenderError::Validation("invalid occupancy u64".to_string())
        })?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_index_groups_negative_chunks_with_euclidean_tiles() {
        let index = TileOccupancyIndex::from_render_chunk_positions(
            Dimension::Overworld,
            8,
            vec![
                ChunkPos {
                    x: -1,
                    z: -1,
                    dimension: Dimension::Overworld,
                },
                ChunkPos {
                    x: -8,
                    z: -8,
                    dimension: Dimension::Overworld,
                },
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
            ],
        )
        .expect("index");
        assert_eq!(index.tile_count(), 2);
        assert_eq!(index.chunk_positions(-1, -1).map(<[ChunkPos]>::len), Some(2));
        assert_eq!(index.chunk_positions(0, 0).map(<[ChunkPos]>::len), Some(1));
    }

    #[test]
    fn occupancy_cache_round_trip_is_exact() {
        let index = TileOccupancyIndex::from_render_chunk_positions(
            Dimension::Overworld,
            8,
            vec![
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                ChunkPos {
                    x: 9,
                    z: 8,
                    dimension: Dimension::Overworld,
                },
            ],
        )
        .expect("index");
        let encoded = encode_index(42, &index);
        let decoded = decode_index(&encoded, 42, Dimension::Overworld, 8).expect("decode");
        assert_eq!(decoded, index);
    }
}

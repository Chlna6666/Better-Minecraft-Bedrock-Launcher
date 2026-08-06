#![allow(clippy::missing_errors_doc)]

use super::pipeline::{
    DEFAULT_PALETTE_VERSION, RENDERER_CACHE_VERSION, RenderBackend, RenderGpuBackend, RenderLayout,
    RenderMode,
};
use crate::error::{BedrockRenderError, Result};
use bedrock_world::{ChunkBounds, ChunkPos, Dimension};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use xxhash_rust::xxh3::xxh3_128;

const TILE_MANIFEST_CACHE_VERSION: u32 = 1;
const TILE_MANIFEST_CACHE_MAGIC: &[u8; 8] = b"BRTIDX01";
const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;
static TILE_MANIFEST_CACHE_WRITE_ID: AtomicUsize = AtomicUsize::new(0);
const TILE_AUTHORITY_CACHE_VERSION: u32 = 2;
const TILE_AUTHORITY_HEADER_MAGIC: &[u8; 8] = b"BRTCHD01";
const TILE_AUTHORITY_CHUNKS_MAGIC: &[u8; 8] = b"BRTCHK01";
const TILE_AUTHORITY_TILES_MAGIC: &[u8; 8] = b"BRTTIL01";
const TILE_AUTHORITY_DEPS_MAGIC: &[u8; 8] = b"BRTDEP01";
const TILE_AUTHORITY_REFS_MAGIC: &[u8; 8] = b"BRTREF01";
const TILE_AUTHORITY_FREE_EXTENTS_MAGIC: &[u8; 8] = b"BRTFRE01";
const TILE_AUTHORITY_BLOB_MAGIC: &[u8; 8] = b"BRTBLB01";
const TILE_AUTHORITY_WAL_MAGIC: &[u8; 8] = b"BRTWAL01";
const TILE_AUTHORITY_INDEX_STEMS: [&str; 5] = [
    "chunks",
    "tiles",
    "tile_deps",
    "chunk_tiles",
    "free_extents",
];
// Keep one prior generation for readers that opened the header before a commit.
const TILE_AUTHORITY_RETAINED_GENERATIONS: u64 = 2;
const TILE_AUTHORITY_EXTENT_REUSE_DELAY_GENERATIONS: u64 = 2;
const TILE_AUTHORITY_WAL_CHECKPOINT_BYTES: u64 = 512 * 1024;
const TILE_AUTHORITY_BLOB_HEADER_LEN: u64 = 20;
const TILE_AUTHORITY_BLOB_HEADER_LEN_USIZE: usize = 20;
pub const TILE_AUTHORITY_FLAG_EMPTY: u32 = 1;
pub const TILE_AUTHORITY_FLAG_NON_EMPTY: u32 = 1 << 1;
const TILE_AUTHORITY_KNOWN_FLAGS: u32 = TILE_AUTHORITY_FLAG_EMPTY | TILE_AUTHORITY_FLAG_NON_EMPTY;
static TILE_AUTHORITY_CACHE_WRITE_ID: AtomicUsize = AtomicUsize::new(0);

/// Compact identity for a manifest probe cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileManifestCacheKey {
    pub world_id: String,
    pub world_signature: String,
    pub renderer_signature: String,
    pub mode_slug: String,
    pub renderer_version: u32,
    pub palette_version: u32,
    pub dimension: Dimension,
    pub chunks_per_tile: u32,
    pub blocks_per_pixel: u32,
    pub pixels_per_block: u32,
}

/// Snapshot persisted in the compact manifest sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileManifestCacheSnapshot {
    pub requested_tiles: Vec<(i32, i32)>,
    pub tile_chunk_index: BTreeMap<(i32, i32), Vec<ChunkPos>>,
    pub bounds: Option<ChunkBounds>,
    pub center_block_x: Option<i32>,
    pub center_block_z: Option<i32>,
}

/// World cache identity computed from one filesystem metadata snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldCacheIdentity {
    /// Stable cache directory identity for this world snapshot.
    pub world_id: String,
    /// Content signature used to reject cache entries after the world changes.
    pub world_signature: String,
}

impl TileManifestCacheKey {
    #[must_use]
    pub fn new(
        world_path: &Path,
        backend: RenderBackend,
        gpu_backend: RenderGpuBackend,
        mode: RenderMode,
        dimension: Dimension,
        layout: RenderLayout,
    ) -> Self {
        let identity = world_cache_identity(world_path);
        Self {
            world_id: identity.world_id,
            world_signature: identity.world_signature,
            renderer_signature: render_preset_cache_signature(world_path, backend, gpu_backend),
            mode_slug: render_mode_cache_slug(mode),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            dimension,
            chunks_per_tile: layout.chunks_per_tile,
            blocks_per_pixel: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
        }
    }

    #[must_use]
    pub fn validation_value(&self) -> u64 {
        let mut hash = FNV1A64_OFFSET;
        fnv1a_write_u32(&mut hash, TILE_MANIFEST_CACHE_VERSION);
        fnv1a_write_str(&mut hash, &self.world_id);
        fnv1a_write_str(&mut hash, &self.world_signature);
        fnv1a_write_str(&mut hash, &self.renderer_signature);
        fnv1a_write_str(&mut hash, &self.mode_slug);
        fnv1a_write_u32(&mut hash, self.renderer_version);
        fnv1a_write_u32(&mut hash, self.palette_version);
        fnv1a_write_i32(&mut hash, self.dimension.id());
        fnv1a_write_u32(&mut hash, self.chunks_per_tile);
        fnv1a_write_u32(&mut hash, self.blocks_per_pixel);
        fnv1a_write_u32(&mut hash, self.pixels_per_block);
        if hash == 0 { FNV1A64_OFFSET } else { hash }
    }

    #[must_use]
    pub fn path_for_root(&self, root: &Path) -> PathBuf {
        root.join("map-manifest-index")
            .join(&self.world_id)
            .join(&self.renderer_signature)
            .join(format!("dimension-{}", self.dimension.id()))
            .join(&self.mode_slug)
            .join(format!(
                "r{}-p{}-v{:016x}",
                self.renderer_version,
                self.palette_version,
                self.validation_value()
            ))
            .join(format!(
                "{}c-{}bpp-{}ppb.bridx",
                self.chunks_per_tile, self.blocks_per_pixel, self.pixels_per_block
            ))
    }
}

/// Persistent manifest probe cache stored as a compact binary sidecar.
#[derive(Debug, Clone)]
pub struct TileManifestCache {
    root: PathBuf,
}

impl TileManifestCache {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn path_for_key(&self, key: &TileManifestCacheKey) -> PathBuf {
        key.path_for_root(&self.root)
    }

    pub fn load(&self, key: &TileManifestCacheKey) -> Result<Option<TileManifestCacheSnapshot>> {
        let path = self.path_for_key(key);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(BedrockRenderError::io(
                    format!("failed to read manifest index {}", path.display()),
                    error,
                ));
            }
        };
        match decode_tile_manifest_cache(&bytes, key) {
            Ok(decoded) => Ok(Some(decoded)),
            Err(error) => {
                let _ = fs::remove_file(&path);
                Err(error)
            }
        }
    }

    pub fn store(
        &self,
        key: &TileManifestCacheKey,
        result: &TileManifestCacheSnapshot,
    ) -> Result<()> {
        let path = self.path_for_key(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                BedrockRenderError::io("failed to create manifest index directory", error)
            })?;
        }
        let encoded = encode_tile_manifest_cache(key, result)?;
        write_atomic_bytes(&path, &encoded)
    }
}

/// Stable identity for the authoritative final-tile cache of one map/render configuration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileAuthorityCacheKey {
    pub world_id: String,
    pub world_signature: String,
    pub renderer_signature: String,
    pub mode_slug: String,
    pub renderer_version: u32,
    pub palette_version: u32,
    pub dimension: Dimension,
    pub chunks_per_tile: u32,
    pub blocks_per_pixel: u32,
    pub pixels_per_block: u32,
}

impl TileAuthorityCacheKey {
    #[must_use]
    pub fn new(
        world_path: &Path,
        backend: RenderBackend,
        gpu_backend: RenderGpuBackend,
        mode: RenderMode,
        dimension: Dimension,
        layout: RenderLayout,
    ) -> Self {
        let identity = world_cache_identity(world_path);
        Self {
            world_id: identity.world_id,
            world_signature: identity.world_signature,
            renderer_signature: render_preset_cache_signature(world_path, backend, gpu_backend),
            mode_slug: render_mode_cache_slug(mode),
            renderer_version: RENDERER_CACHE_VERSION,
            palette_version: DEFAULT_PALETTE_VERSION,
            dimension,
            chunks_per_tile: layout.chunks_per_tile,
            blocks_per_pixel: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
        }
    }

    #[must_use]
    pub fn validation_value(&self) -> u64 {
        let mut hash = FNV1A64_OFFSET;
        fnv1a_write_u32(&mut hash, TILE_AUTHORITY_CACHE_VERSION);
        fnv1a_write_str(&mut hash, &self.world_id);
        fnv1a_write_str(&mut hash, &self.world_signature);
        fnv1a_write_str(&mut hash, &self.renderer_signature);
        fnv1a_write_str(&mut hash, &self.mode_slug);
        fnv1a_write_u32(&mut hash, self.renderer_version);
        fnv1a_write_u32(&mut hash, self.palette_version);
        fnv1a_write_i32(&mut hash, self.dimension.id());
        fnv1a_write_u32(&mut hash, self.chunks_per_tile);
        fnv1a_write_u32(&mut hash, self.blocks_per_pixel);
        fnv1a_write_u32(&mut hash, self.pixels_per_block);
        if hash == 0 { FNV1A64_OFFSET } else { hash }
    }

    #[must_use]
    pub fn root_for_cache_root(&self, root: &Path) -> PathBuf {
        root.join("maps")
            .join(&self.world_id)
            .join(format!(
                "r{}-p{}",
                self.renderer_version, self.palette_version
            ))
            .join(dimension_slug(self.dimension))
            .join(&self.mode_slug)
            .join(format!(
                "{}c-{}bpp-{}ppb",
                self.chunks_per_tile, self.blocks_per_pixel, self.pixels_per_block
            ))
    }
}

/// Cheap chunk state stored by the authority cache to validate tile dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileAuthorityChunkState {
    pub position: ChunkPos,
    pub revision: u64,
    pub content_hash: u128,
}

/// Dependency stamp recorded for a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileAuthorityDependency {
    pub tile_x: i32,
    pub tile_z: i32,
    pub position: ChunkPos,
    pub revision: u64,
    pub content_hash: u128,
}

/// Immutable input for a parallel XXH3-128 chunk validation pass.
#[derive(Debug, Clone)]
pub struct ChunkFingerprintInput {
    pub position: ChunkPos,
    pub revision: u64,
    pub bytes: Arc<[u8]>,
}

/// Hashes independent chunk records in parallel without sharing mutable state.
#[must_use]
pub fn validate_chunk_fingerprints_parallel(
    inputs: Vec<ChunkFingerprintInput>,
) -> Vec<TileAuthorityChunkState> {
    let mut states = inputs
        .into_par_iter()
        .map(|input| TileAuthorityChunkState {
            position: input.position,
            revision: input.revision,
            content_hash: xxh3_128(&input.bytes),
        })
        .collect::<Vec<_>>();
    states.sort_by_key(tile_authority_chunk_state_sort_key);
    states
}

/// Returns the persisted XXH3-128 fingerprint for an encoded tile payload.
#[must_use]
pub fn tile_payload_fingerprint(bytes: &[u8]) -> u128 {
    xxh3_128(bytes)
}

/// Runs CPU-bound chunk fingerprint validation outside an async caller's executor thread.
///
/// The work itself is split by Rayon, so callers should submit a batch instead of one task per
/// chunk. The returned states can be passed to [`TileAuthorityIndexSnapshot::tiles_changed_by_chunk_states`].
///
/// # Errors
///
/// Returns an error when the background task is cancelled or panics.
#[cfg(feature = "async")]
pub async fn validate_chunk_fingerprints_async(
    inputs: Vec<ChunkFingerprintInput>,
) -> Result<Vec<TileAuthorityChunkState>> {
    tokio::task::spawn_blocking(move || validate_chunk_fingerprints_parallel(inputs))
        .await
        .map_err(|error| {
            BedrockRenderError::Join(format!("chunk fingerprint task failed: {error}"))
        })
}

/// Reverse reference used to invalidate tiles touched by a changed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileAuthorityChunkTileRef {
    pub position: ChunkPos,
    pub tile_x: i32,
    pub tile_z: i32,
}

/// A reusable byte range in `tiles.blob` that is safe to reclaim after its grace generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileAuthorityFreeExtent {
    pub offset: u64,
    pub len: u64,
    pub reusable_after_generation: u64,
}

/// Final tile index entry. Empty entries have `blob_len == 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileAuthorityEntry {
    pub tile_x: i32,
    pub tile_z: i32,
    pub width: u32,
    pub height: u32,
    pub pixel_len: u64,
    pub blob_offset: u64,
    pub blob_len: u64,
    /// XXH3-128 of the encoded payload; zero for empty negative entries.
    pub payload_hash: u128,
    pub validation_value: u64,
    pub flags: u32,
}

impl TileAuthorityEntry {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.flags & TILE_AUTHORITY_FLAG_EMPTY != 0
    }

    #[must_use]
    pub fn payload_matches(self, bytes: &[u8]) -> bool {
        self.is_empty() || tile_payload_fingerprint(bytes) == self.payload_hash
    }
}

/// A single committed tile update. Blob bytes must already be final encoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileAuthorityCommit {
    pub entry: TileAuthorityEntry,
    pub encoded_blob: Vec<u8>,
    pub dependencies: Vec<TileAuthorityDependency>,
    pub chunk_states: Vec<TileAuthorityChunkState>,
    pub chunk_tile_refs: Vec<TileAuthorityChunkTileRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TileAuthorityDelta {
    entry: TileAuthorityEntry,
    dependencies: Vec<TileAuthorityDependency>,
    chunk_states: Vec<TileAuthorityChunkState>,
    chunk_tile_refs: Vec<TileAuthorityChunkTileRef>,
}

/// Loaded view of the authority cache index files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileAuthorityIndexSnapshot {
    pub generation: u64,
    pub chunk_states: Vec<TileAuthorityChunkState>,
    pub tiles: Vec<TileAuthorityEntry>,
    pub dependencies: Vec<TileAuthorityDependency>,
    pub chunk_tile_refs: Vec<TileAuthorityChunkTileRef>,
    pub free_extents: Vec<TileAuthorityFreeExtent>,
}

impl TileAuthorityIndexSnapshot {
    #[must_use]
    pub fn tile(&self, tile_x: i32, tile_z: i32) -> Option<TileAuthorityEntry> {
        self.tiles
            .binary_search_by_key(&(tile_x, tile_z), |entry| (entry.tile_x, entry.tile_z))
            .ok()
            .map(|index| self.tiles[index])
    }

    #[must_use]
    pub fn dependencies_for_tile(&self, tile_x: i32, tile_z: i32) -> &[TileAuthorityDependency] {
        let start = self.dependencies.partition_point(|dependency| {
            (dependency.tile_x, dependency.tile_z) < (tile_x, tile_z)
        });
        let end = self.dependencies[start..]
            .partition_point(|dependency| {
                (dependency.tile_x, dependency.tile_z) == (tile_x, tile_z)
            })
            .saturating_add(start);
        &self.dependencies[start..end]
    }

    #[must_use]
    pub fn tiles_for_chunk(&self, position: ChunkPos) -> &[TileAuthorityChunkTileRef] {
        let key = (position.dimension.id(), position.x, position.z);
        let start = self.chunk_tile_refs.partition_point(|reference| {
            (
                reference.position.dimension.id(),
                reference.position.x,
                reference.position.z,
            ) < key
        });
        let end = self.chunk_tile_refs[start..]
            .partition_point(|reference| {
                (
                    reference.position.dimension.id(),
                    reference.position.x,
                    reference.position.z,
                ) == key
            })
            .saturating_add(start);
        &self.chunk_tile_refs[start..end]
    }

    /// Returns every tile whose persisted chunk fingerprint differs from a validated state.
    #[must_use]
    pub fn tiles_changed_by_chunk_states(
        &self,
        current_states: &[TileAuthorityChunkState],
    ) -> Vec<TileAuthorityChunkTileRef> {
        let mut changed_tiles = BTreeSet::new();
        for state in current_states {
            let known = self.chunk_states.binary_search_by_key(
                &tile_authority_chunk_state_sort_key(state),
                tile_authority_chunk_state_sort_key,
            );
            let unchanged = known
                .ok()
                .and_then(|index| self.chunk_states.get(index))
                .is_some_and(|known| {
                    known.revision == state.revision && known.content_hash == state.content_hash
                });
            if !unchanged {
                changed_tiles.extend(self.tiles_for_chunk(state.position).iter().copied());
            }
        }
        changed_tiles.into_iter().collect()
    }
}

/// Batch-local reader for an authoritative tile blob file.
#[derive(Debug)]
pub struct TileAuthorityBlobReader {
    file: File,
    len: u64,
}

impl TileAuthorityBlobReader {
    pub fn open(cache: &TileAuthorityCache, key: &TileAuthorityCacheKey) -> Result<Option<Self>> {
        let blob_path = cache.root_for_key(key).join("tiles.blob");
        let metadata = match fs::metadata(&blob_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(BedrockRenderError::io(
                    format!("failed to stat tile blob {}", blob_path.display()),
                    error,
                ));
            }
        };
        let file = File::open(&blob_path)
            .map_err(|error| BedrockRenderError::io("failed to open tile blob", error))?;
        Ok(Some(Self {
            file,
            len: metadata.len(),
        }))
    }

    pub fn read_entry(&self, entry: TileAuthorityEntry) -> Result<Option<Vec<u8>>> {
        if entry.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let header_end = entry
            .blob_offset
            .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
            .ok_or_else(|| {
                BedrockRenderError::Validation("tile blob offset overflow".to_string())
            })?;
        let end = header_end.checked_add(entry.blob_len).ok_or_else(|| {
            BedrockRenderError::Validation("tile blob length overflow".to_string())
        })?;
        if end > self.len {
            return Ok(None);
        }
        let mut header = [0u8; TILE_AUTHORITY_BLOB_HEADER_LEN_USIZE];
        read_tile_blob_exact_at(&self.file, entry.blob_offset, &mut header)?;
        let blob_header = decode_tile_authority_blob_header(&header)?;
        if blob_header.payload_len != entry.blob_len {
            return Ok(None);
        }
        let mut payload = vec![
            0u8;
            usize::try_from(entry.blob_len).map_err(|_| {
                BedrockRenderError::Validation("tile blob length does not fit usize".to_string())
            })?
        ];
        read_tile_blob_exact_at(&self.file, header_end, &mut payload)?;
        if !entry.payload_matches(&payload) {
            return Ok(None);
        }
        Ok(Some(payload))
    }
}

#[cfg(unix)]
fn read_tile_blob_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;
    let mut read_len = 0usize;
    while read_len < buffer.len() {
        let read = file
            .read_at(
                &mut buffer[read_len..],
                offset.saturating_add(read_len as u64),
            )
            .map_err(|error| BedrockRenderError::io("failed to read tile blob", error))?;
        if read == 0 {
            return Err(BedrockRenderError::Validation(
                "tile blob ended unexpectedly".to_string(),
            ));
        }
        read_len = read_len.saturating_add(read);
    }
    Ok(())
}

#[cfg(windows)]
fn read_tile_blob_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;
    let mut read_len = 0usize;
    while read_len < buffer.len() {
        let read = file
            .seek_read(
                &mut buffer[read_len..],
                offset.saturating_add(read_len as u64),
            )
            .map_err(|error| BedrockRenderError::io("failed to read tile blob", error))?;
        if read == 0 {
            return Err(BedrockRenderError::Validation(
                "tile blob ended unexpectedly".to_string(),
            ));
        }
        read_len = read_len.saturating_add(read);
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_tile_blob_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> Result<()> {
    let mut file = file
        .try_clone()
        .map_err(|error| BedrockRenderError::io("failed to clone tile blob reader", error))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| BedrockRenderError::io("failed to seek tile blob", error))?;
    file.read_exact(buffer)
        .map_err(|error| BedrockRenderError::io("failed to read tile blob", error))
}

/// Authoritative final-tile cache using a committed binary index and append-only blob file.
#[derive(Debug, Clone)]
pub struct TileAuthorityCache {
    root: PathBuf,
}

impl TileAuthorityCache {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn root_for_key(&self, key: &TileAuthorityCacheKey) -> PathBuf {
        key.root_for_cache_root(&self.root)
    }

    pub fn load_index(
        &self,
        key: &TileAuthorityCacheKey,
    ) -> Result<Option<TileAuthorityIndexSnapshot>> {
        let root = self.root_for_key(key);
        let header_path = root.join("header.bin");
        let header = match fs::read(&header_path) {
            Ok(bytes) => decode_tile_authority_header(&bytes, key)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(BedrockRenderError::io(
                    format!(
                        "failed to read tile authority header {}",
                        header_path.display()
                    ),
                    error,
                ));
            }
        };
        let chunks = decode_tile_authority_chunks(
            &read_authority_file(&authority_generation_path(
                &root,
                "chunks",
                header.generation,
            ))?,
            key,
            header.generation,
        )?;
        let tiles = decode_tile_authority_tiles(
            &read_authority_file(&authority_generation_path(
                &root,
                "tiles",
                header.generation,
            ))?,
            key,
            header.generation,
        )?;
        let dependencies = decode_tile_authority_dependencies(
            &read_authority_file(&authority_generation_path(
                &root,
                "tile_deps",
                header.generation,
            ))?,
            key,
            header.generation,
        )?;
        let chunk_tile_refs = decode_tile_authority_chunk_refs(
            &read_authority_file(&authority_generation_path(
                &root,
                "chunk_tiles",
                header.generation,
            ))?,
            key,
            header.generation,
        )?;
        let free_extents = decode_tile_authority_free_extents(
            &read_authority_file(&authority_generation_path(
                &root,
                "free_extents",
                header.generation,
            ))?,
            key,
            header.generation,
        )?;
        let snapshot = TileAuthorityIndexSnapshot {
            generation: header.generation,
            chunk_states: chunks,
            tiles,
            dependencies,
            chunk_tile_refs,
            free_extents,
        };
        replay_tile_authority_wal(&root, key, snapshot).map(Some)
    }

    pub fn commit_tile(
        &self,
        key: &TileAuthorityCacheKey,
        previous: Option<&TileAuthorityIndexSnapshot>,
        commit: TileAuthorityCommit,
        flush: bool,
    ) -> Result<TileAuthorityIndexSnapshot> {
        self.commit_tiles(key, previous, vec![commit], flush)
    }

    pub fn commit_tiles(
        &self,
        key: &TileAuthorityCacheKey,
        previous: Option<&TileAuthorityIndexSnapshot>,
        commits: Vec<TileAuthorityCommit>,
        flush: bool,
    ) -> Result<TileAuthorityIndexSnapshot> {
        if commits.is_empty() {
            return Ok(previous.cloned().unwrap_or_default());
        }
        for commit in &commits {
            validate_tile_authority_commit(commit)?;
        }
        let root = self.root_for_key(key);
        fs::create_dir_all(&root).map_err(|error| {
            BedrockRenderError::io("failed to create tile authority cache", error)
        })?;

        let next_generation =
            previous.map_or(1, |snapshot| snapshot.generation.saturating_add(1).max(1));
        let (snapshot, deltas) = write_and_merge_tile_authority_commits(
            &root,
            previous,
            commits,
            next_generation,
            flush,
        )?;

        let checkpoint_required = previous.is_none()
            || authority_wal_len(&root)?.saturating_add(
                u64::try_from(encode_tile_authority_wal_deltas(&deltas)?.len()).unwrap_or(u64::MAX),
            ) >= TILE_AUTHORITY_WAL_CHECKPOINT_BYTES;
        if checkpoint_required {
            write_tile_authority_checkpoint(&root, key, &snapshot)?;
            clear_tile_authority_wal(&root)?;
        } else {
            append_tile_authority_wal(&root, key, next_generation, &deltas, flush)?;
        }
        Ok(snapshot)
    }

    pub fn read_blob(
        &self,
        key: &TileAuthorityCacheKey,
        entry: TileAuthorityEntry,
    ) -> Result<Option<Vec<u8>>> {
        if entry.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let Some(reader) = TileAuthorityBlobReader::open(self, key)? else {
            return Ok(None);
        };
        reader.read_entry(entry)
    }
}

fn write_tile_authority_checkpoint(
    root: &Path,
    key: &TileAuthorityCacheKey,
    snapshot: &TileAuthorityIndexSnapshot,
) -> Result<()> {
    let next_generation = snapshot.generation;
    let header = TileAuthorityHeader {
        generation: next_generation,
    };
    let chunks = encode_tile_authority_chunks(key, next_generation, &snapshot.chunk_states)?;
    let tiles = encode_tile_authority_tiles(key, next_generation, &snapshot.tiles)?;
    let dependencies =
        encode_tile_authority_dependencies(key, next_generation, &snapshot.dependencies)?;
    let chunk_refs =
        encode_tile_authority_chunk_refs(key, next_generation, &snapshot.chunk_tile_refs)?;
    let free_extents =
        encode_tile_authority_free_extents(key, next_generation, &snapshot.free_extents)?;
    let header = encode_tile_authority_header(key, header);

    write_authority_atomic_bytes(
        &authority_generation_path(root, "chunks", next_generation),
        &chunks,
    )?;
    write_authority_atomic_bytes(
        &authority_generation_path(root, "tiles", next_generation),
        &tiles,
    )?;
    write_authority_atomic_bytes(
        &authority_generation_path(root, "tile_deps", next_generation),
        &dependencies,
    )?;
    write_authority_atomic_bytes(
        &authority_generation_path(root, "chunk_tiles", next_generation),
        &chunk_refs,
    )?;
    write_authority_atomic_bytes(
        &authority_generation_path(root, "free_extents", next_generation),
        &free_extents,
    )?;
    write_authority_atomic_bytes(&root.join("header.bin"), &header)?;
    prune_tile_authority_generations(root, next_generation);
    Ok(())
}

#[must_use]
pub fn world_cache_id(world_path: &Path) -> String {
    world_cache_identity(world_path).world_id
}

#[must_use]
pub fn world_cache_signature(world_path: &Path) -> String {
    world_cache_identity(world_path).world_signature
}

#[must_use]
pub fn world_cache_identity(world_path: &Path) -> WorldCacheIdentity {
    let mut id_hash = FNV1A64_OFFSET;
    let mut signature_hash = FNV1A64_OFFSET;
    fnv1a_write_str(&mut id_hash, &world_path.to_string_lossy());
    hash_path_metadata_pair(world_path, "level.dat", &mut id_hash, &mut signature_hash);
    hash_leveldb_current_state_pair(world_path, &mut id_hash, &mut signature_hash);
    let folder = world_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("world")
        .chars()
        .map(|value| match value {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => value,
            _ => '_',
        })
        .collect::<String>();
    WorldCacheIdentity {
        world_id: format!("{folder}-{id_hash:016x}"),
        world_signature: format!("{signature_hash:016x}"),
    }
}

#[must_use]
pub fn render_mode_cache_slug(mode: RenderMode) -> String {
    match mode {
        RenderMode::Biome { y } => format!("biome-y{y}"),
        RenderMode::RawBiomeLayer { y } => format!("raw-biome-y{y}"),
        RenderMode::SurfaceBlocks => "surface".to_string(),
        RenderMode::LayerBlocks { y } => format!("layer-y{y}"),
        RenderMode::HeightMap => "heightmap".to_string(),
        RenderMode::RawHeightMap => "raw-heightmap".to_string(),
        RenderMode::CaveSlice { y } => format!("cave-y{y}"),
    }
}

#[must_use]
fn dimension_slug(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "overworld".to_string(),
        Dimension::Nether => "nether".to_string(),
        Dimension::End => "end".to_string(),
        Dimension::Unknown(id) => format!("dimension-{id}"),
    }
}

#[must_use]
pub fn render_backend_cache_slug(backend: RenderBackend) -> &'static str {
    match backend {
        RenderBackend::Cpu => "cpu",
        RenderBackend::Auto => "auto",
        RenderBackend::Wgpu => "wgpu",
    }
}

#[must_use]
pub fn render_gpu_backend_cache_slug(gpu_backend: RenderGpuBackend) -> &'static str {
    match gpu_backend {
        RenderGpuBackend::Auto => "auto",
        RenderGpuBackend::Dx11 => "dx11",
        RenderGpuBackend::Dx12 => "dx12",
        RenderGpuBackend::Vulkan => "vulkan",
    }
}

#[must_use]
pub fn render_preset_cache_signature(
    world_path: &Path,
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> String {
    let _ = world_path;
    let mut hash = FNV1A64_OFFSET;
    fnv1a_write_u32(&mut hash, RENDERER_CACHE_VERSION);
    fnv1a_write_u32(&mut hash, DEFAULT_PALETTE_VERSION);
    fnv1a_write_str(&mut hash, render_backend_cache_slug(backend));
    fnv1a_write_str(&mut hash, render_gpu_backend_cache_slug(gpu_backend));
    fnv1a_write_u32(&mut hash, TILE_MANIFEST_CACHE_VERSION);
    format!("{hash:016x}")
}

#[must_use]
pub fn render_cache_validation_seed_from_signature(signature: &str) -> u64 {
    let mut hash = FNV1A64_OFFSET;
    fnv1a_write_str(&mut hash, signature);
    if hash == 0 { FNV1A64_OFFSET } else { hash }
}

#[must_use]
pub fn render_preset_cache_validation_seed(
    world_path: &Path,
    backend: RenderBackend,
    gpu_backend: RenderGpuBackend,
) -> u64 {
    render_cache_validation_seed_from_signature(&render_preset_cache_signature(
        world_path,
        backend,
        gpu_backend,
    ))
}

#[must_use]
pub fn tile_manifest_cache_path(
    root: &Path,
    world_path: &Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
) -> PathBuf {
    let key = TileManifestCacheKey::new(
        world_path,
        render_backend,
        render_gpu_backend,
        mode,
        dimension,
        layout,
    );
    key.path_for_root(root)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileAuthorityHeader {
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileAuthorityBlobWrite {
    offset: u64,
    payload_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TileAuthorityBlobHeader {
    payload_len: u64,
}

fn validate_tile_authority_commit(commit: &TileAuthorityCommit) -> Result<()> {
    validate_tile_authority_flags(commit.entry.flags)?;
    if commit.entry.is_empty() {
        if !commit.encoded_blob.is_empty()
            || commit.entry.blob_len != 0
            || commit.entry.payload_hash != 0
        {
            return Err(BedrockRenderError::Validation(
                "empty authority tile cannot carry blob bytes".to_string(),
            ));
        }
    } else if commit.encoded_blob.is_empty() {
        return Err(BedrockRenderError::Validation(
            "non-empty authority tile requires blob bytes".to_string(),
        ));
    } else if commit.entry.payload_hash != tile_payload_fingerprint(&commit.encoded_blob) {
        return Err(BedrockRenderError::Validation(
            "tile authority payload fingerprint mismatch".to_string(),
        ));
    }
    let expected_pixel_len = u64::from(commit.entry.width)
        .checked_mul(u64::from(commit.entry.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| BedrockRenderError::Validation("tile pixel length overflow".to_string()))?;
    if commit.entry.pixel_len != expected_pixel_len {
        return Err(BedrockRenderError::Validation(format!(
            "tile pixel length mismatch: expected {expected_pixel_len}, got {}",
            commit.entry.pixel_len
        )));
    }
    Ok(())
}

fn validate_tile_authority_flags(flags: u32) -> Result<()> {
    if flags & !TILE_AUTHORITY_KNOWN_FLAGS != 0 {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported tile authority flags {flags:#x}"
        )));
    }
    let has_empty = flags & TILE_AUTHORITY_FLAG_EMPTY != 0;
    let has_non_empty = flags & TILE_AUTHORITY_FLAG_NON_EMPTY != 0;
    if has_empty == has_non_empty {
        return Err(BedrockRenderError::Validation(
            "authority tile must be exactly one of empty or non-empty".to_string(),
        ));
    }
    Ok(())
}

fn write_and_merge_tile_authority_commits(
    root: &Path,
    previous: Option<&TileAuthorityIndexSnapshot>,
    mut commits: Vec<TileAuthorityCommit>,
    generation: u64,
    flush: bool,
) -> Result<(TileAuthorityIndexSnapshot, Vec<TileAuthorityDelta>)> {
    commits.sort_by_key(|commit| (commit.entry.tile_x, commit.entry.tile_z));
    if commits.windows(2).any(|pair| {
        pair[0].entry.tile_x == pair[1].entry.tile_x && pair[0].entry.tile_z == pair[1].entry.tile_z
    }) {
        return Err(BedrockRenderError::Validation(
            "tile authority commit batch contains duplicate tile coordinates".to_string(),
        ));
    }

    let mut free_extents = previous.map_or_else(Vec::new, |snapshot| snapshot.free_extents.clone());
    for commit in &mut commits {
        let previous_entry =
            previous.and_then(|snapshot| snapshot.tile(commit.entry.tile_x, commit.entry.tile_z));
        if commit.entry.is_empty() {
            commit.entry.blob_offset = 0;
            commit.entry.blob_len = 0;
        } else if let Some(entry) = previous_entry
            && !entry.is_empty()
            && entry.payload_hash == commit.entry.payload_hash
            && entry.blob_len == u64::try_from(commit.encoded_blob.len()).unwrap_or(u64::MAX)
        {
            commit.entry.blob_offset = entry.blob_offset;
            commit.entry.blob_len = entry.blob_len;
        } else {
            let allocation_len = tile_authority_blob_record_len(&commit.encoded_blob)?;
            let offset =
                match allocate_tile_authority_extent(&mut free_extents, allocation_len, generation)
                {
                    Some(offset) => {
                        write_tile_authority_blob_at(root, offset, &commit.encoded_blob, flush)?;
                        offset
                    }
                    None => append_tile_authority_blob(root, &commit.encoded_blob, flush)?.offset,
                };
            commit.entry.blob_offset = offset;
            commit.entry.blob_len = u64::try_from(commit.encoded_blob.len()).map_err(|_| {
                BedrockRenderError::Validation("tile blob payload length overflow".to_string())
            })?;
        }

        if let Some(previous_entry) = previous_entry
            && !previous_entry.is_empty()
            && previous_entry.blob_offset != commit.entry.blob_offset
        {
            free_extents.push(TileAuthorityFreeExtent {
                offset: previous_entry.blob_offset,
                len: previous_entry
                    .blob_len
                    .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
                    .ok_or_else(|| {
                        BedrockRenderError::Validation(
                            "tile blob extent length overflow".to_string(),
                        )
                    })?,
                reusable_after_generation: generation
                    .saturating_add(TILE_AUTHORITY_EXTENT_REUSE_DELAY_GENERATIONS),
            });
        }
    }

    coalesce_tile_authority_free_extents(&mut free_extents);
    let deltas = commits
        .iter()
        .map(|commit| TileAuthorityDelta {
            entry: commit.entry,
            dependencies: commit.dependencies.clone(),
            chunk_states: commit.chunk_states.clone(),
            chunk_tile_refs: commit.chunk_tile_refs.clone(),
        })
        .collect();
    Ok((
        merge_tile_authority_commits(previous, commits, generation, free_extents),
        deltas,
    ))
}

fn tile_authority_blob_record_len(payload: &[u8]) -> Result<u64> {
    u64::try_from(payload.len())
        .map_err(|_| {
            BedrockRenderError::Validation("tile blob payload length overflow".to_string())
        })?
        .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
        .ok_or_else(|| {
            BedrockRenderError::Validation("tile blob record length overflow".to_string())
        })
}

fn allocate_tile_authority_extent(
    free_extents: &mut Vec<TileAuthorityFreeExtent>,
    required_len: u64,
    generation: u64,
) -> Option<u64> {
    let candidate = free_extents
        .iter()
        .enumerate()
        .filter(|(_, extent)| {
            extent.reusable_after_generation <= generation && extent.len >= required_len
        })
        .min_by_key(|(_, extent)| extent.len)?
        .0;
    let extent = free_extents.swap_remove(candidate);
    let remaining_len = extent.len.saturating_sub(required_len);
    if remaining_len > 0 {
        free_extents.push(TileAuthorityFreeExtent {
            offset: extent.offset.saturating_add(required_len),
            len: remaining_len,
            reusable_after_generation: extent.reusable_after_generation,
        });
    }
    Some(extent.offset)
}

fn coalesce_tile_authority_free_extents(free_extents: &mut Vec<TileAuthorityFreeExtent>) {
    free_extents.sort_by_key(|extent| (extent.reusable_after_generation, extent.offset));
    let mut merged = Vec::with_capacity(free_extents.len());
    for extent in free_extents.drain(..) {
        let Some(previous) = merged.last_mut() else {
            merged.push(extent);
            continue;
        };
        if previous.reusable_after_generation == extent.reusable_after_generation
            && previous.offset.checked_add(previous.len) == Some(extent.offset)
        {
            previous.len = previous.len.saturating_add(extent.len);
        } else {
            merged.push(extent);
        }
    }
    *free_extents = merged;
}

fn append_tile_authority_blob(
    root: &Path,
    payload: &[u8],
    flush: bool,
) -> Result<TileAuthorityBlobWrite> {
    if payload.is_empty() {
        return Ok(TileAuthorityBlobWrite {
            offset: 0,
            payload_len: 0,
        });
    }
    let path = root.join("tiles.blob");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
        .map_err(|error| BedrockRenderError::io("failed to open tile blob for append", error))?;
    let offset = file
        .seek(SeekFrom::End(0))
        .map_err(|error| BedrockRenderError::io("failed to seek tile blob", error))?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| {
        BedrockRenderError::Validation("tile blob payload length overflow".to_string())
    })?;
    write_tile_authority_blob_bytes(&mut file, payload)?;
    if flush {
        file.sync_data()
            .map_err(|error| BedrockRenderError::io("failed to flush tile blob", error))?;
    }
    Ok(TileAuthorityBlobWrite {
        offset,
        payload_len,
    })
}

fn write_tile_authority_blob_at(
    root: &Path,
    offset: u64,
    payload: &[u8],
    flush: bool,
) -> Result<()> {
    let path = root.join("tiles.blob");
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| {
            BedrockRenderError::io("failed to open tile blob for extent write", error)
        })?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| BedrockRenderError::io("failed to seek tile blob extent", error))?;
    write_tile_authority_blob_bytes(&mut file, payload)?;
    if flush {
        file.sync_data()
            .map_err(|error| BedrockRenderError::io("failed to flush tile blob extent", error))?;
    }
    Ok(())
}

fn write_tile_authority_blob_bytes(file: &mut File, payload: &[u8]) -> Result<()> {
    let payload_len = u64::try_from(payload.len()).map_err(|_| {
        BedrockRenderError::Validation("tile blob payload length overflow".to_string())
    })?;
    file.write_all(TILE_AUTHORITY_BLOB_MAGIC)
        .and_then(|()| file.write_all(&TILE_AUTHORITY_CACHE_VERSION.to_le_bytes()))
        .and_then(|()| file.write_all(&payload_len.to_le_bytes()))
        .and_then(|()| file.write_all(payload))
        .map_err(|error| BedrockRenderError::io("failed to write tile blob", error))
}

fn merge_tile_authority_commits(
    previous: Option<&TileAuthorityIndexSnapshot>,
    commits: Vec<TileAuthorityCommit>,
    generation: u64,
    free_extents: Vec<TileAuthorityFreeExtent>,
) -> TileAuthorityIndexSnapshot {
    let mut chunk_states = previous.map_or_else(Vec::new, |snapshot| snapshot.chunk_states.clone());
    let mut tiles = previous.map_or_else(Vec::new, |snapshot| snapshot.tiles.clone());
    let mut dependencies = previous.map_or_else(Vec::new, |snapshot| snapshot.dependencies.clone());
    let mut chunk_tile_refs =
        previous.map_or_else(Vec::new, |snapshot| snapshot.chunk_tile_refs.clone());

    for commit in commits {
        for state in commit.chunk_states {
            if let Some(existing) = chunk_states
                .iter_mut()
                .find(|existing| existing.position == state.position)
            {
                *existing = state;
            } else {
                chunk_states.push(state);
            }
        }

        tiles.retain(|entry| {
            entry.tile_x != commit.entry.tile_x || entry.tile_z != commit.entry.tile_z
        });
        tiles.push(commit.entry);

        dependencies.retain(|dependency| {
            dependency.tile_x != commit.entry.tile_x || dependency.tile_z != commit.entry.tile_z
        });
        dependencies.extend(commit.dependencies);

        chunk_tile_refs.retain(|reference| {
            reference.tile_x != commit.entry.tile_x || reference.tile_z != commit.entry.tile_z
        });
        chunk_tile_refs.extend(commit.chunk_tile_refs);
    }

    chunk_states.sort_by_key(tile_authority_chunk_state_sort_key);
    chunk_states.dedup_by_key(|state| tile_authority_chunk_state_sort_key(state));
    tiles.sort_by_key(|entry| (entry.tile_x, entry.tile_z));
    dependencies.sort_by_key(tile_authority_dependency_sort_key);
    chunk_tile_refs.sort_by_key(tile_authority_chunk_tile_ref_sort_key);
    chunk_tile_refs.dedup();

    TileAuthorityIndexSnapshot {
        generation,
        chunk_states,
        tiles,
        dependencies,
        chunk_tile_refs,
        free_extents,
    }
}

fn tile_authority_chunk_state_sort_key(state: &TileAuthorityChunkState) -> (i32, i32, i32) {
    (
        state.position.dimension.id(),
        state.position.x,
        state.position.z,
    )
}

fn tile_authority_dependency_sort_key(
    dependency: &TileAuthorityDependency,
) -> (i32, i32, i32, i32, i32) {
    (
        dependency.tile_x,
        dependency.tile_z,
        dependency.position.dimension.id(),
        dependency.position.x,
        dependency.position.z,
    )
}

fn tile_authority_chunk_tile_ref_sort_key(
    reference: &TileAuthorityChunkTileRef,
) -> (i32, i32, i32, i32, i32) {
    (
        reference.position.dimension.id(),
        reference.position.x,
        reference.position.z,
        reference.tile_x,
        reference.tile_z,
    )
}

fn encode_tile_authority_header(
    key: &TileAuthorityCacheKey,
    header: TileAuthorityHeader,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(TILE_AUTHORITY_HEADER_MAGIC);
    push_u32(&mut bytes, TILE_AUTHORITY_CACHE_VERSION);
    push_u64(&mut bytes, key.validation_value());
    push_u64(&mut bytes, header.generation);
    bytes
}

fn decode_tile_authority_header(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
) -> Result<TileAuthorityHeader> {
    let mut reader = TileManifestCacheReader::new(bytes);
    reader.expect_magic(TILE_AUTHORITY_HEADER_MAGIC)?;
    let version = reader.u32()?;
    if version != TILE_AUTHORITY_CACHE_VERSION {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported tile authority header version {version}"
        )));
    }
    let validation_value = reader.u64()?;
    if validation_value != key.validation_value() {
        return Err(BedrockRenderError::Validation(
            "tile authority header validation mismatch".to_string(),
        ));
    }
    let generation = reader.u64()?;
    if !reader.finished() {
        return Err(BedrockRenderError::Validation(
            "tile authority header has trailing bytes".to_string(),
        ));
    }
    Ok(TileAuthorityHeader { generation })
}

fn encode_authority_file_header(
    bytes: &mut Vec<u8>,
    magic: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
    count: usize,
) -> Result<()> {
    bytes.extend_from_slice(magic);
    push_u32(bytes, TILE_AUTHORITY_CACHE_VERSION);
    push_u64(bytes, key.validation_value());
    push_u64(bytes, generation);
    push_u32(
        bytes,
        u32::try_from(count).map_err(|_| {
            BedrockRenderError::Validation("tile authority entry count overflow".to_string())
        })?,
    );
    Ok(())
}

fn decode_authority_file_header(
    reader: &mut TileManifestCacheReader<'_>,
    magic: &[u8],
    key: &TileAuthorityCacheKey,
    expected_generation: u64,
) -> Result<usize> {
    reader.expect_magic(magic)?;
    let version = reader.u32()?;
    if version != TILE_AUTHORITY_CACHE_VERSION {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported tile authority index version {version}"
        )));
    }
    if reader.u64()? != key.validation_value() {
        return Err(BedrockRenderError::Validation(
            "tile authority index validation mismatch".to_string(),
        ));
    }
    let generation = reader.u64()?;
    if generation != expected_generation {
        return Err(BedrockRenderError::Validation(
            "tile authority index generation mismatch".to_string(),
        ));
    }
    usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile authority entry count overflow".to_string())
    })
}

fn encode_tile_authority_chunks(
    key: &TileAuthorityCacheKey,
    generation: u64,
    entries: &[TileAuthorityChunkState],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32 + entries.len().saturating_mul(36));
    encode_authority_file_header(
        &mut bytes,
        TILE_AUTHORITY_CHUNKS_MAGIC,
        key,
        generation,
        entries.len(),
    )?;
    for entry in entries {
        push_chunk_pos(&mut bytes, entry.position);
        push_u64(&mut bytes, entry.revision);
        push_u128(&mut bytes, entry.content_hash);
    }
    Ok(bytes)
}

fn decode_tile_authority_chunks(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
) -> Result<Vec<TileAuthorityChunkState>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count =
        decode_authority_file_header(&mut reader, TILE_AUTHORITY_CHUNKS_MAGIC, key, generation)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityChunkState {
            position: read_chunk_pos(&mut reader)?,
            revision: reader.u64()?,
            content_hash: reader.u128()?,
        });
    }
    ensure_authority_reader_finished(&reader).map(|()| entries)
}

fn push_tile_authority_entry(bytes: &mut Vec<u8>, entry: TileAuthorityEntry) {
    push_i32(bytes, entry.tile_x);
    push_i32(bytes, entry.tile_z);
    push_u32(bytes, entry.width);
    push_u32(bytes, entry.height);
    push_u64(bytes, entry.pixel_len);
    push_u64(bytes, entry.blob_offset);
    push_u64(bytes, entry.blob_len);
    push_u128(bytes, entry.payload_hash);
    push_u64(bytes, entry.validation_value);
    push_u32(bytes, entry.flags);
}

fn read_tile_authority_entry(
    reader: &mut TileManifestCacheReader<'_>,
) -> Result<TileAuthorityEntry> {
    let entry = TileAuthorityEntry {
        tile_x: reader.i32()?,
        tile_z: reader.i32()?,
        width: reader.u32()?,
        height: reader.u32()?,
        pixel_len: reader.u64()?,
        blob_offset: reader.u64()?,
        blob_len: reader.u64()?,
        payload_hash: reader.u128()?,
        validation_value: reader.u64()?,
        flags: reader.u32()?,
    };
    validate_tile_authority_flags(entry.flags)?;
    Ok(entry)
}

fn push_tile_authority_dependencies(
    bytes: &mut Vec<u8>,
    entries: &[TileAuthorityDependency],
) -> Result<()> {
    push_u32(
        bytes,
        u32::try_from(entries.len()).map_err(|_| {
            BedrockRenderError::Validation("tile authority dependency count overflow".to_string())
        })?,
    );
    for entry in entries {
        push_i32(bytes, entry.tile_x);
        push_i32(bytes, entry.tile_z);
        push_chunk_pos(bytes, entry.position);
        push_u64(bytes, entry.revision);
        push_u128(bytes, entry.content_hash);
    }
    Ok(())
}

fn read_tile_authority_dependencies(
    reader: &mut TileManifestCacheReader<'_>,
) -> Result<Vec<TileAuthorityDependency>> {
    let count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile authority dependency count overflow".to_string())
    })?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityDependency {
            tile_x: reader.i32()?,
            tile_z: reader.i32()?,
            position: read_chunk_pos(reader)?,
            revision: reader.u64()?,
            content_hash: reader.u128()?,
        });
    }
    Ok(entries)
}

fn push_tile_authority_chunk_states(
    bytes: &mut Vec<u8>,
    entries: &[TileAuthorityChunkState],
) -> Result<()> {
    push_u32(
        bytes,
        u32::try_from(entries.len()).map_err(|_| {
            BedrockRenderError::Validation("tile authority chunk state count overflow".to_string())
        })?,
    );
    for entry in entries {
        push_chunk_pos(bytes, entry.position);
        push_u64(bytes, entry.revision);
        push_u128(bytes, entry.content_hash);
    }
    Ok(())
}

fn read_tile_authority_chunk_states(
    reader: &mut TileManifestCacheReader<'_>,
) -> Result<Vec<TileAuthorityChunkState>> {
    let count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile authority chunk state count overflow".to_string())
    })?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityChunkState {
            position: read_chunk_pos(reader)?,
            revision: reader.u64()?,
            content_hash: reader.u128()?,
        });
    }
    Ok(entries)
}

fn push_tile_authority_chunk_refs(
    bytes: &mut Vec<u8>,
    entries: &[TileAuthorityChunkTileRef],
) -> Result<()> {
    push_u32(
        bytes,
        u32::try_from(entries.len()).map_err(|_| {
            BedrockRenderError::Validation(
                "tile authority chunk reference count overflow".to_string(),
            )
        })?,
    );
    for entry in entries {
        push_chunk_pos(bytes, entry.position);
        push_i32(bytes, entry.tile_x);
        push_i32(bytes, entry.tile_z);
    }
    Ok(())
}

fn read_tile_authority_chunk_refs(
    reader: &mut TileManifestCacheReader<'_>,
) -> Result<Vec<TileAuthorityChunkTileRef>> {
    let count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile authority chunk reference count overflow".to_string())
    })?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityChunkTileRef {
            position: read_chunk_pos(reader)?,
            tile_x: reader.i32()?,
            tile_z: reader.i32()?,
        });
    }
    Ok(entries)
}

fn encode_tile_authority_tiles(
    key: &TileAuthorityCacheKey,
    generation: u64,
    entries: &[TileAuthorityEntry],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32 + entries.len().saturating_mul(60));
    encode_authority_file_header(
        &mut bytes,
        TILE_AUTHORITY_TILES_MAGIC,
        key,
        generation,
        entries.len(),
    )?;
    for entry in entries {
        validate_tile_authority_flags(entry.flags)?;
        push_i32(&mut bytes, entry.tile_x);
        push_i32(&mut bytes, entry.tile_z);
        push_u32(&mut bytes, entry.width);
        push_u32(&mut bytes, entry.height);
        push_u64(&mut bytes, entry.pixel_len);
        push_u64(&mut bytes, entry.blob_offset);
        push_u64(&mut bytes, entry.blob_len);
        push_u128(&mut bytes, entry.payload_hash);
        push_u64(&mut bytes, entry.validation_value);
        push_u32(&mut bytes, entry.flags);
    }
    Ok(bytes)
}

fn decode_tile_authority_tiles(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
) -> Result<Vec<TileAuthorityEntry>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count =
        decode_authority_file_header(&mut reader, TILE_AUTHORITY_TILES_MAGIC, key, generation)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let entry = TileAuthorityEntry {
            tile_x: reader.i32()?,
            tile_z: reader.i32()?,
            width: reader.u32()?,
            height: reader.u32()?,
            pixel_len: reader.u64()?,
            blob_offset: reader.u64()?,
            blob_len: reader.u64()?,
            payload_hash: reader.u128()?,
            validation_value: reader.u64()?,
            flags: reader.u32()?,
        };
        validate_tile_authority_flags(entry.flags)?;
        entries.push(entry);
    }
    ensure_authority_reader_finished(&reader).map(|()| entries)
}

fn encode_tile_authority_dependencies(
    key: &TileAuthorityCacheKey,
    generation: u64,
    entries: &[TileAuthorityDependency],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32 + entries.len().saturating_mul(44));
    encode_authority_file_header(
        &mut bytes,
        TILE_AUTHORITY_DEPS_MAGIC,
        key,
        generation,
        entries.len(),
    )?;
    for entry in entries {
        push_i32(&mut bytes, entry.tile_x);
        push_i32(&mut bytes, entry.tile_z);
        push_chunk_pos(&mut bytes, entry.position);
        push_u64(&mut bytes, entry.revision);
        push_u128(&mut bytes, entry.content_hash);
    }
    Ok(bytes)
}

fn decode_tile_authority_dependencies(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
) -> Result<Vec<TileAuthorityDependency>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count =
        decode_authority_file_header(&mut reader, TILE_AUTHORITY_DEPS_MAGIC, key, generation)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityDependency {
            tile_x: reader.i32()?,
            tile_z: reader.i32()?,
            position: read_chunk_pos(&mut reader)?,
            revision: reader.u64()?,
            content_hash: reader.u128()?,
        });
    }
    ensure_authority_reader_finished(&reader).map(|()| entries)
}

fn encode_tile_authority_chunk_refs(
    key: &TileAuthorityCacheKey,
    generation: u64,
    entries: &[TileAuthorityChunkTileRef],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32 + entries.len().saturating_mul(20));
    encode_authority_file_header(
        &mut bytes,
        TILE_AUTHORITY_REFS_MAGIC,
        key,
        generation,
        entries.len(),
    )?;
    for entry in entries {
        push_chunk_pos(&mut bytes, entry.position);
        push_i32(&mut bytes, entry.tile_x);
        push_i32(&mut bytes, entry.tile_z);
    }
    Ok(bytes)
}

fn decode_tile_authority_chunk_refs(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
) -> Result<Vec<TileAuthorityChunkTileRef>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count =
        decode_authority_file_header(&mut reader, TILE_AUTHORITY_REFS_MAGIC, key, generation)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(TileAuthorityChunkTileRef {
            position: read_chunk_pos(&mut reader)?,
            tile_x: reader.i32()?,
            tile_z: reader.i32()?,
        });
    }
    ensure_authority_reader_finished(&reader).map(|()| entries)
}

fn encode_tile_authority_free_extents(
    key: &TileAuthorityCacheKey,
    generation: u64,
    entries: &[TileAuthorityFreeExtent],
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(32 + entries.len().saturating_mul(24));
    encode_authority_file_header(
        &mut bytes,
        TILE_AUTHORITY_FREE_EXTENTS_MAGIC,
        key,
        generation,
        entries.len(),
    )?;
    for entry in entries {
        push_u64(&mut bytes, entry.offset);
        push_u64(&mut bytes, entry.len);
        push_u64(&mut bytes, entry.reusable_after_generation);
    }
    Ok(bytes)
}

fn decode_tile_authority_free_extents(
    bytes: &[u8],
    key: &TileAuthorityCacheKey,
    generation: u64,
) -> Result<Vec<TileAuthorityFreeExtent>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count = decode_authority_file_header(
        &mut reader,
        TILE_AUTHORITY_FREE_EXTENTS_MAGIC,
        key,
        generation,
    )?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let extent = TileAuthorityFreeExtent {
            offset: reader.u64()?,
            len: reader.u64()?,
            reusable_after_generation: reader.u64()?,
        };
        if extent.len == 0 {
            return Err(BedrockRenderError::Validation(
                "tile authority free extent cannot be empty".to_string(),
            ));
        }
        entries.push(extent);
    }
    coalesce_tile_authority_free_extents(&mut entries);
    ensure_authority_reader_finished(&reader).map(|()| entries)
}

fn decode_tile_authority_blob_header(bytes: &[u8]) -> Result<TileAuthorityBlobHeader> {
    let mut reader = TileManifestCacheReader::new(bytes);
    reader.expect_magic(TILE_AUTHORITY_BLOB_MAGIC)?;
    let version = reader.u32()?;
    if version != TILE_AUTHORITY_CACHE_VERSION {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported tile authority blob version {version}"
        )));
    }
    let payload_len = reader.u64()?;
    if !reader.finished() {
        return Err(BedrockRenderError::Validation(
            "tile authority blob header has trailing bytes".to_string(),
        ));
    }
    Ok(TileAuthorityBlobHeader { payload_len })
}

fn authority_wal_path(root: &Path) -> PathBuf {
    root.join("authority.wal")
}

fn authority_wal_len(root: &Path) -> Result<u64> {
    match fs::metadata(authority_wal_path(root)) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(BedrockRenderError::io(
            "failed to stat tile authority WAL",
            error,
        )),
    }
}

fn clear_tile_authority_wal(root: &Path) -> Result<()> {
    match fs::remove_file(authority_wal_path(root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BedrockRenderError::io(
            "failed to clear tile authority WAL",
            error,
        )),
    }
}

fn append_tile_authority_wal(
    root: &Path,
    key: &TileAuthorityCacheKey,
    generation: u64,
    deltas: &[TileAuthorityDelta],
    flush: bool,
) -> Result<()> {
    let payload = encode_tile_authority_wal_deltas(deltas)?;
    let payload_len = u32::try_from(payload.len()).map_err(|_| {
        BedrockRenderError::Validation("tile authority WAL payload length overflow".to_string())
    })?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(authority_wal_path(root))
        .map_err(|error| BedrockRenderError::io("failed to open tile authority WAL", error))?;
    file.write_all(TILE_AUTHORITY_WAL_MAGIC)
        .and_then(|()| file.write_all(&TILE_AUTHORITY_CACHE_VERSION.to_le_bytes()))
        .and_then(|()| file.write_all(&key.validation_value().to_le_bytes()))
        .and_then(|()| file.write_all(&generation.to_le_bytes()))
        .and_then(|()| file.write_all(&payload_len.to_le_bytes()))
        .and_then(|()| file.write_all(&tile_payload_fingerprint(&payload).to_le_bytes()))
        .and_then(|()| file.write_all(&payload))
        .map_err(|error| BedrockRenderError::io("failed to append tile authority WAL", error))?;
    if flush {
        file.sync_data()
            .map_err(|error| BedrockRenderError::io("failed to flush tile authority WAL", error))?;
    }
    Ok(())
}

fn replay_tile_authority_wal(
    root: &Path,
    key: &TileAuthorityCacheKey,
    mut snapshot: TileAuthorityIndexSnapshot,
) -> Result<TileAuthorityIndexSnapshot> {
    let bytes = match fs::read(authority_wal_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(snapshot),
        Err(error) => {
            return Err(BedrockRenderError::io(
                "failed to read tile authority WAL",
                error,
            ));
        }
    };
    let mut offset = 0usize;
    while offset < bytes.len() {
        const WAL_HEADER_LEN: usize = 48;
        if bytes.len().saturating_sub(offset) < WAL_HEADER_LEN {
            break;
        }
        let mut reader = TileManifestCacheReader::new(&bytes[offset..]);
        reader.expect_magic(TILE_AUTHORITY_WAL_MAGIC)?;
        if reader.u32()? != TILE_AUTHORITY_CACHE_VERSION {
            return Err(BedrockRenderError::Validation(
                "unsupported tile authority WAL version".to_string(),
            ));
        }
        if reader.u64()? != key.validation_value() {
            return Err(BedrockRenderError::Validation(
                "tile authority WAL validation mismatch".to_string(),
            ));
        }
        let generation = reader.u64()?;
        let payload_len = usize::try_from(reader.u32()?).map_err(|_| {
            BedrockRenderError::Validation("tile authority WAL payload length overflow".to_string())
        })?;
        let checksum = reader.u128()?;
        let payload_start = offset.saturating_add(reader.offset);
        let Some(payload_end) = payload_start.checked_add(payload_len) else {
            return Err(BedrockRenderError::Validation(
                "tile authority WAL payload offset overflow".to_string(),
            ));
        };
        if payload_end > bytes.len() {
            break;
        }
        let payload = &bytes[payload_start..payload_end];
        if tile_payload_fingerprint(payload) != checksum {
            return Err(BedrockRenderError::Validation(
                "tile authority WAL checksum mismatch".to_string(),
            ));
        }
        if generation > snapshot.generation {
            if generation != snapshot.generation.saturating_add(1) {
                return Err(BedrockRenderError::Validation(
                    "tile authority WAL generation gap".to_string(),
                ));
            }
            snapshot = apply_tile_authority_wal_deltas(
                &snapshot,
                decode_tile_authority_wal_deltas(payload)?,
                generation,
            )?;
        }
        offset = payload_end;
    }
    Ok(snapshot)
}

fn encode_tile_authority_wal_deltas(deltas: &[TileAuthorityDelta]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    push_u32(
        &mut bytes,
        u32::try_from(deltas.len()).map_err(|_| {
            BedrockRenderError::Validation("tile authority WAL delta count overflow".to_string())
        })?,
    );
    for delta in deltas {
        push_tile_authority_entry(&mut bytes, delta.entry);
        push_tile_authority_dependencies(&mut bytes, &delta.dependencies)?;
        push_tile_authority_chunk_states(&mut bytes, &delta.chunk_states)?;
        push_tile_authority_chunk_refs(&mut bytes, &delta.chunk_tile_refs)?;
    }
    Ok(bytes)
}

fn decode_tile_authority_wal_deltas(bytes: &[u8]) -> Result<Vec<TileAuthorityDelta>> {
    let mut reader = TileManifestCacheReader::new(bytes);
    let count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile authority WAL delta count overflow".to_string())
    })?;
    let mut deltas = Vec::with_capacity(count);
    for _ in 0..count {
        deltas.push(TileAuthorityDelta {
            entry: read_tile_authority_entry(&mut reader)?,
            dependencies: read_tile_authority_dependencies(&mut reader)?,
            chunk_states: read_tile_authority_chunk_states(&mut reader)?,
            chunk_tile_refs: read_tile_authority_chunk_refs(&mut reader)?,
        });
    }
    ensure_authority_reader_finished(&reader).map(|()| deltas)
}

fn apply_tile_authority_wal_deltas(
    previous: &TileAuthorityIndexSnapshot,
    deltas: Vec<TileAuthorityDelta>,
    generation: u64,
) -> Result<TileAuthorityIndexSnapshot> {
    let mut free_extents = previous.free_extents.clone();
    for delta in &deltas {
        let previous_entry = previous.tile(delta.entry.tile_x, delta.entry.tile_z);
        let reuses_previous_payload = previous_entry.is_some_and(|entry| {
            !entry.is_empty()
                && entry.blob_offset == delta.entry.blob_offset
                && entry.payload_hash == delta.entry.payload_hash
        });
        if !delta.entry.is_empty() && !reuses_previous_payload {
            consume_tile_authority_free_extent(
                &mut free_extents,
                delta.entry.blob_offset,
                delta
                    .entry
                    .blob_len
                    .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
                    .ok_or_else(|| {
                        BedrockRenderError::Validation(
                            "tile authority WAL extent length overflow".to_string(),
                        )
                    })?,
            );
        }
        if let Some(entry) = previous_entry
            && !entry.is_empty()
            && entry.blob_offset != delta.entry.blob_offset
        {
            free_extents.push(TileAuthorityFreeExtent {
                offset: entry.blob_offset,
                len: entry
                    .blob_len
                    .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
                    .ok_or_else(|| {
                        BedrockRenderError::Validation(
                            "tile authority WAL free extent overflow".to_string(),
                        )
                    })?,
                reusable_after_generation: generation
                    .saturating_add(TILE_AUTHORITY_EXTENT_REUSE_DELAY_GENERATIONS),
            });
        }
    }
    coalesce_tile_authority_free_extents(&mut free_extents);
    let commits = deltas
        .into_iter()
        .map(|delta| TileAuthorityCommit {
            entry: delta.entry,
            encoded_blob: Vec::new(),
            dependencies: delta.dependencies,
            chunk_states: delta.chunk_states,
            chunk_tile_refs: delta.chunk_tile_refs,
        })
        .collect();
    Ok(merge_tile_authority_commits(
        Some(previous),
        commits,
        generation,
        free_extents,
    ))
}

fn consume_tile_authority_free_extent(
    free_extents: &mut Vec<TileAuthorityFreeExtent>,
    offset: u64,
    len: u64,
) {
    let Some(index) = free_extents.iter().position(|extent| {
        extent.offset <= offset
            && extent.offset.checked_add(extent.len).is_some_and(|end| {
                offset
                    .checked_add(len)
                    .is_some_and(|required_end| required_end <= end)
            })
    }) else {
        return;
    };
    let extent = free_extents.swap_remove(index);
    if extent.offset < offset {
        free_extents.push(TileAuthorityFreeExtent {
            offset: extent.offset,
            len: offset.saturating_sub(extent.offset),
            reusable_after_generation: extent.reusable_after_generation,
        });
    }
    let end = offset.saturating_add(len);
    let extent_end = extent.offset.saturating_add(extent.len);
    if end < extent_end {
        free_extents.push(TileAuthorityFreeExtent {
            offset: end,
            len: extent_end.saturating_sub(end),
            reusable_after_generation: extent.reusable_after_generation,
        });
    }
}

fn read_authority_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| {
        BedrockRenderError::io(
            format!("failed to read tile authority index {}", path.display()),
            error,
        )
    })
}

fn authority_generation_path(root: &Path, stem: &str, generation: u64) -> PathBuf {
    root.join(format!("{stem}.{generation:016x}.bin"))
}

fn prune_tile_authority_generations(root: &Path, latest_generation: u64) {
    let oldest_retained_generation =
        latest_generation.saturating_sub(TILE_AUTHORITY_RETAINED_GENERATIONS.saturating_sub(1));
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!(
                "failed to list tile authority cache generations for cleanup {}: {error}",
                root.display()
            );
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::warn!(
                    "failed to inspect tile authority cache generation in {}: {error}",
                    root.display()
                );
                continue;
            }
        };
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(generation) = authority_index_generation_from_file_name(file_name) else {
            continue;
        };
        if generation >= oldest_retained_generation {
            continue;
        }
        let path = entry.path();
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "failed to prune tile authority cache generation {}: {error}",
                path.display()
            );
        }
    }
}

fn authority_index_generation_from_file_name(file_name: &str) -> Option<u64> {
    for stem in TILE_AUTHORITY_INDEX_STEMS {
        let Some(generation) = file_name
            .strip_prefix(stem)
            .and_then(|suffix| suffix.strip_prefix('.'))
            .and_then(|suffix| suffix.strip_suffix(".bin"))
        else {
            continue;
        };
        return u64::from_str_radix(generation, 16).ok();
    }
    None
}

fn write_authority_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp_path = tile_authority_temp_path(path);
    fs::write(&temp_path, bytes).map_err(|error| {
        BedrockRenderError::io("failed to write temporary tile authority index", error)
    })?;
    replace_authority_file(&temp_path, path).map_err(|error| {
        cleanup_temp_authority_cache(&temp_path);
        BedrockRenderError::io(
            format!(
                "failed to commit temporary tile authority index {} to {} atomically",
                temp_path.display(),
                path.display()
            ),
            error,
        )
    })
}

#[cfg(not(windows))]
fn replace_authority_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp_path, path)
}

#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Windows requires MoveFileExW for atomic replace-with-existing semantics."
)]
fn replace_authority_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        let mut extended = OsString::new();
        if path.is_absolute() {
            extended.push(r"\\?\");
        }
        extended.push(path.as_os_str());
        extended.encode_wide().chain(std::iter::once(0)).collect()
    }

    let temp_path = wide_path(temp_path);
    let path = wide_path(path);
    // SAFETY: Both paths are null-terminated UTF-16 buffers that remain alive for the call.
    let moved = unsafe {
        MoveFileExW(
            temp_path.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn tile_authority_temp_path(path: &Path) -> PathBuf {
    let write_id = TILE_AUTHORITY_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("tile-authority-index");
    path.with_file_name(format!(
        "{file_name}.next.{}.{}.tmp",
        std::process::id(),
        write_id
    ))
}

fn cleanup_temp_authority_cache(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        log::warn!(
            "failed to remove temporary tile authority index file {}: {error}",
            path.display()
        );
    }
}

fn ensure_authority_reader_finished(reader: &TileManifestCacheReader<'_>) -> Result<()> {
    if reader.finished() {
        Ok(())
    } else {
        Err(BedrockRenderError::Validation(
            "tile authority index has trailing bytes".to_string(),
        ))
    }
}

fn push_chunk_pos(bytes: &mut Vec<u8>, position: ChunkPos) {
    push_i32(bytes, position.x);
    push_i32(bytes, position.z);
    push_i32(bytes, position.dimension.id());
}

fn read_chunk_pos(reader: &mut TileManifestCacheReader<'_>) -> Result<ChunkPos> {
    Ok(ChunkPos {
        x: reader.i32()?,
        z: reader.i32()?,
        dimension: Dimension::from_id(reader.i32()?),
    })
}

fn encode_tile_manifest_cache(
    key: &TileManifestCacheKey,
    result: &TileManifestCacheSnapshot,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(
        128 + result
            .tile_chunk_index
            .values()
            .map(|positions| 16 + positions.len().saturating_mul(12))
            .sum::<usize>(),
    );
    bytes.extend_from_slice(TILE_MANIFEST_CACHE_MAGIC);
    push_u32(&mut bytes, TILE_MANIFEST_CACHE_VERSION);
    push_u64(&mut bytes, key.validation_value());
    push_i32(&mut bytes, key.dimension.id());
    push_u32(&mut bytes, key.chunks_per_tile);
    push_u32(&mut bytes, key.blocks_per_pixel);
    push_u32(&mut bytes, key.pixels_per_block);
    push_u32(
        &mut bytes,
        u32::try_from(result.requested_tiles.len()).map_err(|_| {
            BedrockRenderError::Validation(
                "tile manifest requested tile count overflow".to_string(),
            )
        })?,
    );
    for (tile_x, tile_z) in &result.requested_tiles {
        push_i32(&mut bytes, *tile_x);
        push_i32(&mut bytes, *tile_z);
    }
    push_u32(
        &mut bytes,
        u32::try_from(result.tile_chunk_index.len()).map_err(|_| {
            BedrockRenderError::Validation("tile manifest entry count overflow".to_string())
        })?,
    );
    let center_flags = u8::from(result.center_block_x.is_some())
        | (u8::from(result.center_block_z.is_some()) << 1);
    push_u8(&mut bytes, center_flags);
    if let Some(center_block_x) = result.center_block_x {
        push_i32(&mut bytes, center_block_x);
    }
    if let Some(center_block_z) = result.center_block_z {
        push_i32(&mut bytes, center_block_z);
    }
    for (&(tile_x, tile_z), positions) in &result.tile_chunk_index {
        push_i32(&mut bytes, tile_x);
        push_i32(&mut bytes, tile_z);
        push_u32(
            &mut bytes,
            u32::try_from(positions.len()).map_err(|_| {
                BedrockRenderError::Validation("tile manifest chunk count overflow".to_string())
            })?,
        );
        for position in positions {
            push_i32(&mut bytes, position.x);
            push_i32(&mut bytes, position.z);
            push_i32(&mut bytes, position.dimension.id());
        }
    }
    match result.bounds {
        Some(bounds) => {
            push_u8(&mut bytes, 1);
            push_i32(&mut bytes, bounds.dimension.id());
            push_i32(&mut bytes, bounds.min_chunk_x);
            push_i32(&mut bytes, bounds.min_chunk_z);
            push_i32(&mut bytes, bounds.max_chunk_x);
            push_i32(&mut bytes, bounds.max_chunk_z);
            push_u64(
                &mut bytes,
                u64::try_from(bounds.chunk_count).map_err(|_| {
                    BedrockRenderError::Validation(
                        "tile manifest bounds count overflow".to_string(),
                    )
                })?,
            );
        }
        None => push_u8(&mut bytes, 0),
    }
    Ok(bytes)
}

#[allow(clippy::too_many_lines)]
fn decode_tile_manifest_cache(
    bytes: &[u8],
    key: &TileManifestCacheKey,
) -> Result<TileManifestCacheSnapshot> {
    let mut reader = TileManifestCacheReader::new(bytes);
    reader.expect_magic(TILE_MANIFEST_CACHE_MAGIC)?;
    let version = reader.u32()?;
    if version != TILE_MANIFEST_CACHE_VERSION {
        return Err(BedrockRenderError::Validation(format!(
            "unsupported tile manifest cache version {version}"
        )));
    }
    let validation_value = reader.u64()?;
    if validation_value != key.validation_value() {
        return Err(BedrockRenderError::Validation(
            "tile manifest cache validation mismatch".to_string(),
        ));
    }
    let dimension = reader.i32()?;
    if dimension != key.dimension.id() {
        return Err(BedrockRenderError::Validation(
            "tile manifest cache dimension mismatch".to_string(),
        ));
    }
    let chunks_per_tile = reader.u32()?;
    let blocks_per_pixel = reader.u32()?;
    let pixels_per_block = reader.u32()?;
    if chunks_per_tile != key.chunks_per_tile
        || blocks_per_pixel != key.blocks_per_pixel
        || pixels_per_block != key.pixels_per_block
    {
        return Err(BedrockRenderError::Validation(
            "tile manifest cache layout mismatch".to_string(),
        ));
    }
    let requested_tile_count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile manifest requested tile count overflow".to_string())
    })?;
    let mut requested_tiles = Vec::with_capacity(requested_tile_count);
    for _ in 0..requested_tile_count {
        requested_tiles.push((reader.i32()?, reader.i32()?));
    }
    let entry_count = usize::try_from(reader.u32()?).map_err(|_| {
        BedrockRenderError::Validation("tile manifest entry count overflow".to_string())
    })?;
    let center_flags = reader.u8()?;
    if center_flags & !0b11 != 0 {
        return Err(BedrockRenderError::Validation(
            "invalid tile manifest center flags".to_string(),
        ));
    }
    let center_block_x = if center_flags & 0b01 != 0 {
        Some(reader.i32()?)
    } else {
        None
    };
    let center_block_z = if center_flags & 0b10 != 0 {
        Some(reader.i32()?)
    } else {
        None
    };
    let mut tile_chunk_index = BTreeMap::<(i32, i32), Vec<ChunkPos>>::new();
    for _ in 0..entry_count {
        let tile_x = reader.i32()?;
        let tile_z = reader.i32()?;
        let chunk_count = usize::try_from(reader.u32()?).map_err(|_| {
            BedrockRenderError::Validation("tile manifest chunk count overflow".to_string())
        })?;
        let mut positions = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let x = reader.i32()?;
            let z = reader.i32()?;
            let dimension = Dimension::from_id(reader.i32()?);
            positions.push(ChunkPos { x, z, dimension });
        }
        positions.sort();
        positions.dedup();
        tile_chunk_index.insert((tile_x, tile_z), positions);
    }
    let bounds = match reader.u8()? {
        0 => None,
        1 => {
            let dimension = Dimension::from_id(reader.i32()?);
            let min_chunk_x = reader.i32()?;
            let min_chunk_z = reader.i32()?;
            let max_chunk_x = reader.i32()?;
            let max_chunk_z = reader.i32()?;
            let chunk_count = usize::try_from(reader.u64()?).map_err(|_| {
                BedrockRenderError::Validation("tile manifest bounds count overflow".to_string())
            })?;
            Some(ChunkBounds {
                dimension,
                min_chunk_x,
                min_chunk_z,
                max_chunk_x,
                max_chunk_z,
                chunk_count,
            })
        }
        tag => {
            return Err(BedrockRenderError::Validation(format!(
                "invalid tile manifest bounds tag {tag}"
            )));
        }
    };
    if !reader.finished() {
        return Err(BedrockRenderError::Validation(
            "tile manifest cache has trailing bytes".to_string(),
        ));
    }
    let requested_tiles = if requested_tiles.is_empty() {
        tile_chunk_index.keys().copied().collect()
    } else {
        requested_tiles
    };
    Ok(TileManifestCacheSnapshot {
        requested_tiles,
        tile_chunk_index,
        bounds,
        center_block_x,
        center_block_z,
    })
}

fn hash_path_metadata_pair(
    world_path: &Path,
    relative: &str,
    first_hasher: &mut u64,
    second_hasher: &mut u64,
) {
    let path = world_path.join(relative);
    fnv1a_write_str(first_hasher, relative);
    fnv1a_write_str(second_hasher, relative);
    if let Ok(metadata) = fs::metadata(&path) {
        fnv1a_write_u64(first_hasher, metadata.len());
        fnv1a_write_u64(second_hasher, metadata.len());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            fnv1a_write_u64(first_hasher, duration.as_secs());
            fnv1a_write_u64(second_hasher, duration.as_secs());
            fnv1a_write_u32(first_hasher, duration.subsec_nanos());
            fnv1a_write_u32(second_hasher, duration.subsec_nanos());
        }
    }
}

fn hash_leveldb_current_state_pair(
    world_path: &Path,
    first_hasher: &mut u64,
    second_hasher: &mut u64,
) {
    hash_path_metadata_pair(world_path, "db/CURRENT", first_hasher, second_hasher);
    let db_path = world_path.join("db");
    let current_path = db_path.join("CURRENT");
    let Ok(current) = fs::read_to_string(&current_path) else {
        return;
    };
    let manifest_name = current.trim();
    fnv1a_write_str(first_hasher, manifest_name);
    fnv1a_write_str(second_hasher, manifest_name);
    if !manifest_name.is_empty() {
        let manifest_relative = format!("db/{manifest_name}");
        hash_path_metadata_pair(world_path, &manifest_relative, first_hasher, second_hasher);
    }
    let Ok(entries) = fs::read_dir(&db_path) else {
        return;
    };
    let mut storage_file_names = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?;
            matches!(extension, "log" | "ldb" | "sst")
                .then(|| path.file_name()?.to_str().map(str::to_string))
                .flatten()
        })
        .collect::<Vec<_>>();
    storage_file_names.sort();
    for file_name in storage_file_names {
        hash_path_metadata_pair(
            world_path,
            &format!("db/{file_name}"),
            first_hasher,
            second_hasher,
        );
    }
}

fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp_path = tile_manifest_temp_path(path);
    fs::write(&temp_path, bytes).map_err(|error| {
        BedrockRenderError::io("failed to write temporary manifest index", error)
    })?;
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(rename_error) if path.exists() => {
            fs::remove_file(path).map_err(|remove_error| {
                cleanup_temp_manifest_cache(&temp_path);
                BedrockRenderError::io(
                    format!("failed to replace manifest index after rename error: {rename_error}"),
                    remove_error,
                )
            })?;
            fs::rename(&temp_path, path).map_err(|error| {
                cleanup_temp_manifest_cache(&temp_path);
                BedrockRenderError::io("failed to replace manifest index", error)
            })
        }
        Err(error) => {
            cleanup_temp_manifest_cache(&temp_path);
            Err(BedrockRenderError::io(
                "failed to move temporary manifest index into place",
                error,
            ))
        }
    }
}

fn tile_manifest_temp_path(path: &Path) -> PathBuf {
    let write_id = TILE_MANIFEST_CACHE_WRITE_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("manifest-index");
    path.with_file_name(format!(
        "{file_name}.{}.{}.tmp",
        std::process::id(),
        write_id
    ))
}

fn cleanup_temp_manifest_cache(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        log::warn!(
            "failed to remove temporary manifest index file {}: {error}",
            path.display()
        );
    }
}

fn fnv1a_write_str(hash: &mut u64, value: &str) {
    fnv1a_write_bytes(hash, value.as_bytes());
}

fn fnv1a_write_u32(hash: &mut u64, value: u32) {
    fnv1a_write_bytes(hash, &value.to_le_bytes());
}

fn fnv1a_write_u64(hash: &mut u64, value: u64) {
    fnv1a_write_bytes(hash, &value.to_le_bytes());
}

fn fnv1a_write_i32(hash: &mut u64, value: i32) {
    fnv1a_write_bytes(hash, &value.to_le_bytes());
}

fn fnv1a_write_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
}

fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u128(bytes: &mut Vec<u8>, value: u128) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct TileManifestCacheReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> TileManifestCacheReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn expect_magic(&mut self, magic: &[u8]) -> Result<()> {
        let value = self.bytes(magic.len())?;
        if value == magic {
            Ok(())
        } else {
            Err(BedrockRenderError::Validation(
                "invalid tile manifest cache magic".to_string(),
            ))
        }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(len).ok_or_else(|| {
            BedrockRenderError::Validation("tile manifest cache offset overflow".to_string())
        })?;
        let slice = self.bytes.get(self.offset..end).ok_or_else(|| {
            BedrockRenderError::Validation("truncated tile manifest cache entry".to_string())
        })?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes = self.bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn u128(&mut self) -> Result<u128> {
        let bytes = self.bytes(16)?;
        Ok(u128::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]))
    }

    fn i32(&mut self) -> Result<i32> {
        let bytes = self.bytes(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn test_world(prefix: &str) -> PathBuf {
        let world_path = unique_test_dir(prefix);
        fs::create_dir_all(&world_path).expect("create test world");
        fs::write(world_path.join("level.dat"), b"level").expect("write level.dat");
        world_path
    }

    fn test_layout() -> RenderLayout {
        RenderLayout {
            chunks_per_tile: 8,
            blocks_per_pixel: 1,
            pixels_per_block: 4,
        }
    }

    fn test_key(world_path: &Path) -> TileManifestCacheKey {
        TileManifestCacheKey::new(
            world_path,
            RenderBackend::Cpu,
            RenderGpuBackend::Auto,
            RenderMode::SurfaceBlocks,
            Dimension::Overworld,
            test_layout(),
        )
    }

    fn test_authority_key(world_path: &Path) -> TileAuthorityCacheKey {
        TileAuthorityCacheKey::new(
            world_path,
            RenderBackend::Cpu,
            RenderGpuBackend::Auto,
            RenderMode::SurfaceBlocks,
            Dimension::Overworld,
            test_layout(),
        )
    }

    fn test_snapshot() -> TileManifestCacheSnapshot {
        let mut tile_chunk_index = BTreeMap::new();
        tile_chunk_index.insert(
            (0, 0),
            vec![ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            }],
        );
        tile_chunk_index.insert(
            (1, -1),
            vec![ChunkPos {
                x: 8,
                z: -1,
                dimension: Dimension::Overworld,
            }],
        );
        TileManifestCacheSnapshot {
            requested_tiles: vec![(0, 0), (1, -1)],
            tile_chunk_index,
            bounds: Some(ChunkBounds {
                dimension: Dimension::Overworld,
                min_chunk_x: 0,
                min_chunk_z: -1,
                max_chunk_x: 8,
                max_chunk_z: 0,
                chunk_count: 2,
            }),
            center_block_x: Some(64),
            center_block_z: Some(-64),
        }
    }

    #[test]
    fn world_cache_identity_matches_compatibility_accessors_and_tracks_db_state() {
        let world_path = test_world("world-cache-identity");
        let initial = world_cache_identity(&world_path);

        assert_eq!(initial.world_id, world_cache_id(&world_path));
        assert_eq!(initial.world_signature, world_cache_signature(&world_path));

        let db_path = world_path.join("db");
        fs::create_dir_all(&db_path).expect("create db directory");
        fs::write(db_path.join("CURRENT"), b"MANIFEST-000001\n").expect("write CURRENT");
        fs::write(db_path.join("MANIFEST-000001"), b"manifest").expect("write manifest");
        let updated = world_cache_identity(&world_path);

        assert_ne!(updated.world_id, initial.world_id);
        assert_ne!(updated.world_signature, initial.world_signature);
        fs::remove_dir_all(world_path).expect("remove test world");
    }

    #[test]
    fn tile_manifest_cache_round_trips_snapshot() {
        let world_path = test_world("br-cache-round-trip-world");
        let cache_root = unique_test_dir("br-cache-round-trip-cache");
        let key = test_key(&world_path);
        let snapshot = test_snapshot();
        let cache = TileManifestCache::new(&cache_root);

        cache.store(&key, &snapshot).expect("store snapshot");
        let loaded = cache
            .load(&key)
            .expect("load snapshot")
            .expect("snapshot present");

        assert_eq!(loaded, snapshot);
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_manifest_cache_validation_value_is_stable() {
        let world_path = test_world("br-cache-validation-world");
        let first = test_key(&world_path);
        let second = test_key(&world_path);

        assert_eq!(first.validation_value(), second.validation_value());
        assert_ne!(first.validation_value(), 0);
        assert_eq!(
            first.path_for_root(Path::new("cache-root")),
            second.path_for_root(Path::new("cache-root"))
        );
        let _ = fs::remove_dir_all(world_path);
    }

    #[test]
    fn tile_manifest_cache_rejects_version_mismatch() {
        let world_path = test_world("br-cache-version-world");
        let cache_root = unique_test_dir("br-cache-version-cache");
        let key = test_key(&world_path);
        let cache = TileManifestCache::new(&cache_root);
        let path = cache.path_for_key(&key);
        let mut encoded =
            encode_tile_manifest_cache(&key, &test_snapshot()).expect("encode snapshot");
        encoded[8..12].copy_from_slice(&(TILE_MANIFEST_CACHE_VERSION + 1).to_le_bytes());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create cache parent");
        }
        fs::write(&path, encoded).expect("write mismatched version");

        assert!(cache.load(&key).is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_manifest_cache_rejects_corrupt_index() {
        let world_path = test_world("br-cache-corrupt-world");
        let cache_root = unique_test_dir("br-cache-corrupt-cache");
        let key = test_key(&world_path);
        let cache = TileManifestCache::new(&cache_root);
        let path = cache.path_for_key(&key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create cache parent");
        }
        fs::write(&path, b"not-a-cache").expect("write corrupt cache");

        assert!(cache.load(&key).is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_manifest_cache_round_trips_empty_negative_tiles() {
        let world_path = test_world("br-cache-empty-world");
        let cache_root = unique_test_dir("br-cache-empty-cache");
        let key = test_key(&world_path);
        let cache = TileManifestCache::new(&cache_root);
        let mut tile_chunk_index = BTreeMap::new();
        tile_chunk_index.insert((0, 0), Vec::new());
        let snapshot = TileManifestCacheSnapshot {
            requested_tiles: vec![(0, 0)],
            tile_chunk_index,
            bounds: None,
            center_block_x: None,
            center_block_z: None,
        };

        cache.store(&key, &snapshot).expect("store empty snapshot");
        let loaded = cache
            .load(&key)
            .expect("load empty snapshot")
            .expect("snapshot present");

        assert_eq!(loaded, snapshot);
        assert!(
            loaded
                .tile_chunk_index
                .get(&(0, 0))
                .is_some_and(Vec::is_empty)
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    fn authority_commit(tile_x: i32, tile_z: i32, payload: Vec<u8>) -> TileAuthorityCommit {
        let position = ChunkPos {
            x: tile_x * 8,
            z: tile_z * 8,
            dimension: Dimension::Overworld,
        };
        TileAuthorityCommit {
            entry: TileAuthorityEntry {
                tile_x,
                tile_z,
                width: 2,
                height: 2,
                pixel_len: 16,
                blob_offset: 0,
                blob_len: u64::try_from(payload.len()).expect("payload length"),
                payload_hash: if payload.is_empty() {
                    0
                } else {
                    xxh3_128(&payload)
                },
                validation_value: 0x1234_5678,
                flags: if payload.is_empty() {
                    TILE_AUTHORITY_FLAG_EMPTY
                } else {
                    TILE_AUTHORITY_FLAG_NON_EMPTY
                },
            },
            encoded_blob: payload,
            dependencies: vec![TileAuthorityDependency {
                tile_x,
                tile_z,
                position,
                revision: 7,
                content_hash: 0xfeed,
            }],
            chunk_states: vec![TileAuthorityChunkState {
                position,
                revision: 7,
                content_hash: 0xfeed,
            }],
            chunk_tile_refs: vec![TileAuthorityChunkTileRef {
                position,
                tile_x,
                tile_z,
            }],
        }
    }

    #[test]
    fn tile_authority_cache_commits_blob_before_index_round_trip() {
        let world_path = test_world("br-authority-round-trip-world");
        let cache_root = unique_test_dir("br-authority-round-trip-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let payload = b"encoded-final-tile".to_vec();

        let snapshot = cache
            .commit_tile(&key, None, authority_commit(1, -2, payload.clone()), true)
            .expect("commit tile");
        let loaded = cache
            .load_index(&key)
            .expect("load index")
            .expect("index present");
        assert_eq!(loaded, snapshot);

        let entry = loaded.tiles[0];
        assert_eq!(
            cache.read_blob(&key, entry).expect("read blob"),
            Some(payload)
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_empty_tile_has_no_blob_payload() {
        let world_path = test_world("br-authority-empty-world");
        let cache_root = unique_test_dir("br-authority-empty-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);

        let snapshot = cache
            .commit_tile(&key, None, authority_commit(0, 0, Vec::new()), false)
            .expect("commit empty tile");
        let entry = snapshot.tiles[0];

        assert!(entry.is_empty());
        assert_eq!(entry.blob_len, 0);
        assert_eq!(
            cache.read_blob(&key, entry).expect("read empty"),
            Some(Vec::new())
        );
        assert!(!cache.root_for_key(&key).join("tiles.blob").exists());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_batch_commit_uses_one_generation() {
        let world_path = test_world("br-authority-batch-world");
        let cache_root = unique_test_dir("br-authority-batch-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);

        let snapshot = cache
            .commit_tiles(
                &key,
                None,
                vec![
                    authority_commit(0, 0, b"first".to_vec()),
                    authority_commit(1, 0, b"second".to_vec()),
                ],
                false,
            )
            .expect("commit batch");
        let loaded = cache
            .load_index(&key)
            .expect("load batch index")
            .expect("index present");

        assert_eq!(snapshot.generation, 1);
        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.tiles.len(), 2);
        assert_eq!(
            loaded.tile(0, 0).map(|entry| entry.validation_value),
            Some(0x1234_5678)
        );
        assert_eq!(
            loaded.dependencies_for_tile(1, 0),
            &[TileAuthorityDependency {
                tile_x: 1,
                tile_z: 0,
                position: ChunkPos {
                    x: 8,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                revision: 7,
                content_hash: 0xfeed,
            }]
        );
        assert_eq!(
            loaded.tiles_for_chunk(ChunkPos {
                x: 8,
                z: 0,
                dimension: Dimension::Overworld,
            }),
            &[TileAuthorityChunkTileRef {
                position: ChunkPos {
                    x: 8,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                tile_x: 1,
                tile_z: 0,
            }]
        );
        assert_eq!(
            cache.read_blob(&key, loaded.tiles[0]).expect("read first"),
            Some(b"first".to_vec())
        );
        assert_eq!(
            cache.read_blob(&key, loaded.tiles[1]).expect("read second"),
            Some(b"second".to_vec())
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_blob_reader_reuses_open_file_for_entries() {
        let world_path = test_world("br-authority-blob-reader-world");
        let cache_root = unique_test_dir("br-authority-blob-reader-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let snapshot = cache
            .commit_tiles(
                &key,
                None,
                vec![
                    authority_commit(0, 0, b"first".to_vec()),
                    authority_commit(1, 0, b"second".to_vec()),
                ],
                false,
            )
            .expect("commit batch");
        let reader = TileAuthorityBlobReader::open(&cache, &key)
            .expect("open blob reader")
            .expect("blob exists");

        assert_eq!(
            reader
                .read_entry(snapshot.tile(0, 0).expect("first entry"))
                .expect("read first"),
            Some(b"first".to_vec())
        );
        assert_eq!(
            reader
                .read_entry(snapshot.tile(1, 0).expect("second entry"))
                .expect("read second"),
            Some(b"second".to_vec())
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_rejects_version_mismatch() {
        let world_path = test_world("br-authority-version-world");
        let cache_root = unique_test_dir("br-authority-version-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        cache
            .commit_tile(&key, None, authority_commit(0, 0, b"tile".to_vec()), false)
            .expect("commit tile");
        let header_path = cache.root_for_key(&key).join("header.bin");
        let mut header = fs::read(&header_path).expect("read header");
        header[8..12].copy_from_slice(&(TILE_AUTHORITY_CACHE_VERSION + 1).to_le_bytes());
        fs::write(&header_path, header).expect("write bad header");

        assert!(cache.load_index(&key).is_err());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_rejects_corrupt_index_generation() {
        let world_path = test_world("br-authority-corrupt-world");
        let cache_root = unique_test_dir("br-authority-corrupt-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        cache
            .commit_tile(&key, None, authority_commit(0, 0, b"tile".to_vec()), false)
            .expect("commit tile");
        let tiles_path = authority_generation_path(&cache.root_for_key(&key), "tiles", 1);
        let mut tiles = fs::read(&tiles_path).expect("read tiles");
        tiles[20..28].copy_from_slice(&999_u64.to_le_bytes());
        fs::write(&tiles_path, tiles).expect("write corrupt tiles");

        assert!(cache.load_index(&key).is_err());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_blob_range_miss_when_truncated() {
        let world_path = test_world("br-authority-truncated-world");
        let cache_root = unique_test_dir("br-authority-truncated-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let snapshot = cache
            .commit_tile(&key, None, authority_commit(0, 0, b"tile".to_vec()), false)
            .expect("commit tile");
        fs::write(cache.root_for_key(&key).join("tiles.blob"), b"short").expect("truncate blob");

        assert_eq!(
            cache.read_blob(&key, snapshot.tiles[0]).expect("read blob"),
            None
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_rejects_payload_with_mismatched_xxh3_fingerprint() {
        let world_path = test_world("br-authority-payload-fingerprint-world");
        let cache_root = unique_test_dir("br-authority-payload-fingerprint-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let snapshot = cache
            .commit_tile(&key, None, authority_commit(0, 0, b"tile".to_vec()), false)
            .expect("commit tile");
        let entry = snapshot.tile(0, 0).expect("tile entry");
        let payload_offset = entry
            .blob_offset
            .checked_add(TILE_AUTHORITY_BLOB_HEADER_LEN)
            .expect("payload offset");
        let path = cache.root_for_key(&key).join("tiles.blob");
        let mut bytes = fs::read(&path).expect("read blob");
        bytes[usize::try_from(payload_offset).expect("payload offset fits usize")] ^= 0xff;
        fs::write(&path, bytes).expect("corrupt payload");

        assert_eq!(cache.read_blob(&key, entry).expect("read blob"), None);
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_reuses_retired_extent_after_grace_generations() {
        let world_path = test_world("br-authority-extent-reuse-world");
        let cache_root = unique_test_dir("br-authority-extent-reuse-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);

        let first = cache
            .commit_tile(&key, None, authority_commit(0, 0, vec![1; 96]), false)
            .expect("commit first tile");
        let first_offset = first.tile(0, 0).expect("first tile entry").blob_offset;
        let second = cache
            .commit_tile(
                &key,
                Some(&first),
                authority_commit(0, 0, vec![2; 96]),
                false,
            )
            .expect("replace first tile");

        let third = cache
            .commit_tile(
                &key,
                Some(&second),
                authority_commit(1, 0, vec![3; 96]),
                false,
            )
            .expect("advance grace generation");
        let fourth = cache
            .commit_tile(
                &key,
                Some(&third),
                authority_commit(2, 0, vec![4; 96]),
                false,
            )
            .expect("reuse retired extent");

        assert_eq!(
            fourth.tile(2, 0).expect("reused tile entry").blob_offset,
            first_offset
        );
        assert_eq!(
            cache
                .read_blob(&key, fourth.tile(2, 0).expect("reused tile entry"))
                .expect("read reused tile"),
            Some(vec![4; 96])
        );
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_skips_blob_write_for_matching_payload_fingerprint() {
        let world_path = test_world("br-authority-matching-payload-world");
        let cache_root = unique_test_dir("br-authority-matching-payload-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let first = cache
            .commit_tile(&key, None, authority_commit(0, 0, vec![7; 96]), false)
            .expect("commit first tile");
        let second = cache
            .commit_tile(
                &key,
                Some(&first),
                authority_commit(0, 0, vec![7; 96]),
                false,
            )
            .expect("commit matching tile");

        assert_eq!(
            second.tile(0, 0).expect("matching tile entry").blob_offset,
            first.tile(0, 0).expect("first tile entry").blob_offset
        );
        assert!(second.free_extents.is_empty());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn chunk_fingerprint_validation_runs_in_parallel_and_detects_changes() {
        let position = ChunkPos {
            x: 12,
            z: -8,
            dimension: Dimension::Overworld,
        };
        let initial = validate_chunk_fingerprints_parallel(vec![ChunkFingerprintInput {
            position,
            revision: 1,
            bytes: Arc::<[u8]>::from(b"initial chunk bytes".as_slice()),
        }]);
        let changed = validate_chunk_fingerprints_parallel(vec![ChunkFingerprintInput {
            position,
            revision: 2,
            bytes: Arc::<[u8]>::from(b"changed chunk bytes".as_slice()),
        }]);

        assert_eq!(initial.len(), 1);
        assert_eq!(changed.len(), 1);
        assert_ne!(initial[0].content_hash, changed[0].content_hash);
        assert_eq!(changed[0].revision, 2);
    }

    #[test]
    fn chunk_fingerprint_index_returns_only_tiles_for_changed_chunks() {
        let position = ChunkPos {
            x: 2,
            z: 3,
            dimension: Dimension::Overworld,
        };
        let initial = validate_chunk_fingerprints_parallel(vec![ChunkFingerprintInput {
            position,
            revision: 1,
            bytes: Arc::<[u8]>::from(b"before".as_slice()),
        }]);
        let changed = validate_chunk_fingerprints_parallel(vec![ChunkFingerprintInput {
            position,
            revision: 2,
            bytes: Arc::<[u8]>::from(b"after".as_slice()),
        }]);
        let snapshot = TileAuthorityIndexSnapshot {
            chunk_states: initial,
            chunk_tile_refs: vec![TileAuthorityChunkTileRef {
                position,
                tile_x: 4,
                tile_z: -1,
            }],
            ..TileAuthorityIndexSnapshot::default()
        };

        assert!(
            snapshot
                .tiles_changed_by_chunk_states(&snapshot.chunk_states)
                .is_empty()
        );
        assert_eq!(
            snapshot.tiles_changed_by_chunk_states(&changed),
            vec![TileAuthorityChunkTileRef {
                position,
                tile_x: 4,
                tile_z: -1,
            }]
        );
    }

    #[cfg(feature = "async")]
    #[test]
    fn chunk_fingerprint_async_validation_returns_background_result() {
        let position = ChunkPos {
            x: 9,
            z: 4,
            dimension: Dimension::Overworld,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build runtime");
        let states = runtime
            .block_on(validate_chunk_fingerprints_async(vec![
                ChunkFingerprintInput {
                    position,
                    revision: 3,
                    bytes: Arc::<[u8]>::from(b"async chunk bytes".as_slice()),
                },
            ]))
            .expect("validate chunk asynchronously");

        assert_eq!(states[0].position, position);
        assert_eq!(states[0].revision, 3);
    }

    #[test]
    fn tile_authority_cache_retains_checkpoint_and_replays_wal_generations() {
        let world_path = test_world("br-gen-prune-world");
        let cache_root = unique_test_dir("br-gen-prune-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let mut snapshot = None;

        for tile_x in 0..4 {
            snapshot = Some(
                cache
                    .commit_tile(
                        &key,
                        snapshot.as_ref(),
                        authority_commit(tile_x, 0, format!("tile-{tile_x}").into_bytes()),
                        false,
                    )
                    .expect("commit tile"),
            );
        }

        let root = cache.root_for_key(&key);
        for stem in ["chunks", "tiles", "tile_deps", "chunk_tiles"] {
            assert!(authority_generation_path(&root, stem, 1).exists());
            assert!(!authority_generation_path(&root, stem, 2).exists());
            assert!(!authority_generation_path(&root, stem, 3).exists());
            assert!(!authority_generation_path(&root, stem, 4).exists());
        }
        let loaded = cache
            .load_index(&key)
            .expect("load latest index")
            .expect("latest index present");
        assert_eq!(loaded.generation, 4);
        assert_eq!(loaded.tiles.len(), 4);
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_wal_replay_remains_authoritative_after_header_reopen() {
        let world_path = test_world("br-authority-generation-world");
        let cache_root = unique_test_dir("br-authority-generation-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let first = cache
            .commit_tile(&key, None, authority_commit(0, 0, b"first".to_vec()), false)
            .expect("commit first tile");
        let header_path = cache.root_for_key(&key).join("header.bin");
        let first_header = fs::read(&header_path).expect("read first header");
        let second = cache
            .commit_tile(
                &key,
                Some(&first),
                authority_commit(1, 0, b"second".to_vec()),
                false,
            )
            .expect("commit second tile");
        assert_eq!(second.generation, 2);

        fs::write(&header_path, first_header).expect("restore first header");
        let loaded = cache
            .load_index(&key)
            .expect("load previous generation")
            .expect("index present");

        assert_eq!(loaded.generation, 2);
        assert_eq!(loaded.tiles.len(), 2);
        assert_eq!((loaded.tiles[0].tile_x, loaded.tiles[0].tile_z), (0, 0));
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_replays_wal_without_rewriting_checkpoint_indexes() {
        let world_path = test_world("br-authority-wal-replay-world");
        let cache_root = unique_test_dir("br-authority-wal-replay-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let first = cache
            .commit_tile(&key, None, authority_commit(0, 0, b"first".to_vec()), false)
            .expect("commit checkpoint tile");
        let second = cache
            .commit_tile(
                &key,
                Some(&first),
                authority_commit(1, 0, b"second".to_vec()),
                false,
            )
            .expect("commit wal tile");
        let root = cache.root_for_key(&key);
        let header = decode_tile_authority_header(
            &fs::read(root.join("header.bin")).expect("read checkpoint header"),
            &key,
        )
        .expect("decode checkpoint header");

        assert_eq!(header.generation, first.generation);
        assert_eq!(second.generation, first.generation + 1);
        assert!(root.join("authority.wal").exists());
        let loaded = cache
            .load_index(&key)
            .expect("replay authority wal")
            .expect("authority index");
        assert_eq!(loaded.generation, second.generation);
        assert!(loaded.tile(1, 0).is_some());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }

    #[test]
    fn tile_authority_cache_ignores_torn_wal_tail_after_checkpoint() {
        let world_path = test_world("br-authority-wal-torn-world");
        let cache_root = unique_test_dir("br-authority-wal-torn-cache");
        let key = test_authority_key(&world_path);
        let cache = TileAuthorityCache::new(&cache_root);
        let first = cache
            .commit_tile(&key, None, authority_commit(0, 0, b"first".to_vec()), false)
            .expect("commit checkpoint tile");
        cache
            .commit_tile(
                &key,
                Some(&first),
                authority_commit(1, 0, b"second".to_vec()),
                false,
            )
            .expect("commit wal tile");
        let wal_path = authority_wal_path(&cache.root_for_key(&key));
        let mut wal = fs::read(&wal_path).expect("read authority wal");
        wal.pop();
        fs::write(&wal_path, wal).expect("truncate authority wal tail");

        let loaded = cache
            .load_index(&key)
            .expect("load checkpoint after torn wal")
            .expect("authority index");
        assert_eq!(loaded.generation, first.generation);
        assert!(loaded.tile(0, 0).is_some());
        assert!(loaded.tile(1, 0).is_none());
        let _ = fs::remove_dir_all(world_path);
        let _ = fs::remove_dir_all(cache_root);
    }
}

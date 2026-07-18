use anyhow::{Context, Result, bail};
use bedrock_world::{
    BedrockWorld, CancelFlag, ChunkPos, ChunkRecordQuery, ChunkRecordQueryResult, Dimension,
    ParsedChunkRecordValue, query_chunk_records_many_blocking_with_control,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use xxhash_rust::xxh3::xxh3_128;

const CACHE_VERSION: u16 = 2;
const INDEX_FILE: &str = "index.bin";
const MAP_INFO_CACHE_DIRECTORY: &str = "map-info";
const MAX_MAP_INFO_QUERY_WORKERS: usize = 4;
const MIN_TILES_PER_QUERY_WORKER: usize = 8;
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A tile address for BMCBL-owned map information.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MapInfoTileKey {
    /// Bedrock dimension identifier.
    pub dimension_id: i32,
    /// Tile X coordinate.
    pub tile_x: i32,
    /// Tile Z coordinate.
    pub tile_z: i32,
    /// Number of chunks on one tile edge.
    pub chunks_per_tile: u16,
}

/// A compact entity marker owned by one information tile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapInfoEntity {
    /// Absolute X coordinate in blocks.
    pub block_x: f32,
    /// Absolute Z coordinate in blocks.
    pub block_z: f32,
    /// Bedrock entity identifier, when available.
    pub identifier: Option<String>,
}

/// A compact block-entity marker owned by one information tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapInfoBlockEntity {
    /// Absolute X coordinate in blocks.
    pub block_x: i32,
    /// Absolute Z coordinate in blocks.
    pub block_z: i32,
}

/// An aggregate count for one chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapInfoChunkCount {
    /// Chunk X coordinate.
    pub chunk_x: i32,
    /// Chunk Z coordinate.
    pub chunk_z: i32,
    /// Number of matching records in the chunk.
    pub count: u32,
}

/// A hardcoded spawn area described in block coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapInfoBlockRect {
    /// Inclusive minimum block X coordinate.
    pub min_block_x: i32,
    /// Inclusive minimum block Z coordinate.
    pub min_block_z: i32,
    /// Exclusive maximum block X coordinate.
    pub max_block_x: i32,
    /// Exclusive maximum block Z coordinate.
    pub max_block_z: i32,
}

/// Persisted overlay data for one tile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapInfoTilePayload {
    /// Entity markers within the tile's chunk range.
    pub entities: Vec<MapInfoEntity>,
    /// Block-entity markers within the tile's chunk range.
    pub block_entities: Vec<MapInfoBlockEntity>,
    /// Pending tick counts grouped by chunk.
    pub pending_tick_counts: Vec<MapInfoChunkCount>,
    /// Hardcoded spawn areas anchored in the tile.
    pub hardcoded_spawn_areas: Vec<MapInfoBlockRect>,
}

/// Aggregated payloads for a visible set of tiles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapInfoOverlaySnapshot {
    /// Entity markers from all requested tiles.
    pub entities: Vec<MapInfoEntity>,
    /// Block-entity markers from all requested tiles.
    pub block_entities: Vec<MapInfoBlockEntity>,
    /// Pending tick counts from all requested tiles.
    pub pending_tick_counts: Vec<MapInfoChunkCount>,
    /// Hardcoded spawn areas from all requested tiles.
    pub hardcoded_spawn_areas: Vec<MapInfoBlockRect>,
    /// Number of tiles decoded from persistent cache.
    pub cached_tile_count: usize,
    /// Number of tiles rebuilt from world records.
    pub rebuilt_tile_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MapInfoIndex {
    version: u16,
    entries: BTreeMap<MapInfoTileKey, MapInfoIndexEntry>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct MapInfoIndexEntry {
    payload_hash: u128,
}

/// Loads valid information tiles and rebuilds only changed tile dependencies.
///
/// All LevelDB access and NBT parsing are blocking. Call this function from a
/// bounded background task, never from a GPUI render or input callback.
pub fn load_map_info_tiles_blocking(
    world_path: &Path,
    dimension: Dimension,
    chunks_per_tile: u16,
    tile_coordinates: &[(i32, i32)],
    cancel: &CancelFlag,
    max_workers: usize,
) -> Result<MapInfoOverlaySnapshot> {
    let keys = requested_tile_keys(dimension, chunks_per_tile, tile_coordinates)?;
    if keys.is_empty() {
        return Ok(MapInfoOverlaySnapshot::default());
    }
    let cache = MapInfoCache::for_world(world_path);
    let mut index = cache.load_index()?;

    // Map edits invalidate their owning information tiles explicitly. Query only
    // missing tiles so the initial build does not read every LevelDB record twice.
    let mut payloads = BTreeMap::new();
    let mut rebuild_keys = Vec::new();
    let mut cached_tile_count = 0usize;
    for key in &keys {
        let Some(entry) = index.entries.get(key).copied() else {
            rebuild_keys.push(*key);
            continue;
        };
        match cache.load_tile(*key, entry.payload_hash) {
            Ok(payload) => {
                cached_tile_count = cached_tile_count.saturating_add(1);
                payloads.insert(*key, payload);
            }
            Err(error) => {
                tracing::debug!(?error, ?key, "rebuilding invalid map information tile");
                rebuild_keys.push(*key);
            }
        }
    }
    if !rebuild_keys.is_empty() {
        cancel_if_requested(cancel)?;
        let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
            .context("open world for map information cache")?;
        let records = query_map_info_records_parallel(&world, &rebuild_keys, cancel, max_workers)?;
        let records_by_tile = records_by_tile(records, chunks_per_tile);
        for key in &rebuild_keys {
            let payload = records_by_tile
                .get(key)
                .map_or_else(MapInfoTilePayload::default, |records| {
                    MapInfoTilePayload::from_records(records)
                });
            let payload_hash = cache.store_tile(*key, &payload)?;
            index
                .entries
                .insert(*key, MapInfoIndexEntry { payload_hash });
            payloads.insert(*key, payload);
        }
        cache.store_index(&index)?;
    }
    Ok(MapInfoOverlaySnapshot::from_payloads(
        payloads.into_values(),
        cached_tile_count,
        rebuild_keys.len(),
    ))
}

fn query_map_info_records_parallel(
    world: &BedrockWorld,
    keys: &[MapInfoTileKey],
    cancel: &CancelFlag,
    max_workers: usize,
) -> Result<Vec<ChunkRecordQueryResult>> {
    let worker_count = map_info_query_worker_count(keys.len(), max_workers);
    if worker_count == 1 {
        return query_chunk_records_many_blocking_with_control(
            world,
            chunks_for_keys(keys)?,
            map_info_record_query(),
            cancel,
        )
        .map_err(Into::into);
    }

    let batch_size = keys.len().div_ceil(worker_count);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .thread_name(|index| format!("map-info-{index}"))
        .build()
        .context("build map information query pool")?;
    let batches = pool.install(|| {
        keys.par_chunks(batch_size)
            .map(|batch| {
                cancel_if_requested(cancel)?;
                query_chunk_records_many_blocking_with_control(
                    world,
                    chunks_for_keys(batch)?,
                    map_info_record_query(),
                    cancel,
                )
                .map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()
    })?;
    Ok(batches.into_iter().flatten().collect())
}

fn map_info_query_worker_count(tile_count: usize, max_workers: usize) -> usize {
    let useful_workers = tile_count.div_ceil(MIN_TILES_PER_QUERY_WORKER).max(1);
    useful_workers
        .min(max_workers.max(1))
        .min(MAX_MAP_INFO_QUERY_WORKERS)
}

/// Loads only valid persisted information tiles without opening the world or
/// fingerprinting chunks. Callers can publish this partial snapshot immediately
/// and then run `load_map_info_tiles_blocking` to validate and rebuild misses.
pub fn load_cached_map_info_tiles_blocking(
    world_path: &Path,
    dimension: Dimension,
    chunks_per_tile: u16,
    tile_coordinates: &[(i32, i32)],
    cancel: &CancelFlag,
) -> Result<MapInfoOverlaySnapshot> {
    let keys = requested_tile_keys(dimension, chunks_per_tile, tile_coordinates)?;
    if keys.is_empty() {
        return Ok(MapInfoOverlaySnapshot::default());
    }
    let cache = MapInfoCache::for_world(world_path);
    let index = cache.load_index()?;
    let mut payloads = BTreeMap::new();
    for key in keys {
        cancel_if_requested(cancel)?;
        let Some(entry) = index.entries.get(&key).copied() else {
            continue;
        };
        match cache.load_tile(key, entry.payload_hash) {
            Ok(payload) => {
                payloads.insert(key, payload);
            }
            Err(error) => {
                tracing::debug!(?error, ?key, "skipping invalid cached map information tile");
            }
        }
    }
    let cached_tile_count = payloads.len();
    Ok(MapInfoOverlaySnapshot::from_payloads(
        payloads.into_values(),
        cached_tile_count,
        0,
    ))
}

/// Removes the cached tiles that own edited chunks.
///
/// The operation touches only BMCBL's cache files. It is intentionally
/// independent of terrain rendering cache ownership.
pub fn invalidate_map_info_tiles_for_chunks(
    world_path: &Path,
    chunks_per_tile: u16,
    chunks: &BTreeSet<ChunkPos>,
) -> Result<usize> {
    if chunks_per_tile == 0 || chunks.is_empty() {
        return Ok(0);
    }
    let cache = MapInfoCache::for_world(world_path);
    let mut index = cache.load_index()?;
    let affected_keys = chunks
        .iter()
        .map(|chunk| MapInfoTileKey {
            dimension_id: chunk.dimension.id(),
            tile_x: chunk.x.div_euclid(i32::from(chunks_per_tile)),
            tile_z: chunk.z.div_euclid(i32::from(chunks_per_tile)),
            chunks_per_tile,
        })
        .collect::<BTreeSet<_>>();
    let mut removed: usize = 0;
    for key in affected_keys {
        if index.entries.remove(&key).is_some() {
            cache.remove_tile(key)?;
            removed = removed.saturating_add(1);
        }
    }
    if removed > 0 {
        cache.store_index(&index)?;
    }
    Ok(removed)
}

impl MapInfoTilePayload {
    fn from_records(records: &[ChunkRecordQueryResult]) -> Self {
        let mut payload = Self::default();
        let mut pending_tick_counts = BTreeMap::<(i32, i32), u32>::new();
        for result in records {
            for record in &result.records {
                match &record.value {
                    ParsedChunkRecordValue::Entities(entities) => {
                        for entity in entities {
                            let Some(position) = entity.position else {
                                continue;
                            };
                            payload.entities.push(MapInfoEntity {
                                block_x: position[0] as f32,
                                block_z: position[2] as f32,
                                identifier: entity.identifier.clone(),
                            });
                        }
                    }
                    ParsedChunkRecordValue::BlockEntities(block_entities) => {
                        for entity in block_entities {
                            let Some(position) = entity.position else {
                                continue;
                            };
                            payload.block_entities.push(MapInfoBlockEntity {
                                block_x: position[0],
                                block_z: position[2],
                            });
                        }
                    }
                    ParsedChunkRecordValue::PendingTicks(ticks) => {
                        let count = u32::try_from(ticks.len()).unwrap_or(u32::MAX);
                        let entry = pending_tick_counts
                            .entry((result.pos.x, result.pos.z))
                            .or_default();
                        *entry = entry.saturating_add(count);
                    }
                    ParsedChunkRecordValue::HardcodedSpawnAreas(areas) => {
                        for area in areas {
                            payload.hardcoded_spawn_areas.push(MapInfoBlockRect {
                                min_block_x: area.min[0],
                                min_block_z: area.min[2],
                                max_block_x: area.max[0].saturating_add(1),
                                max_block_z: area.max[2].saturating_add(1),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        payload.pending_tick_counts = pending_tick_counts
            .into_iter()
            .map(|((chunk_x, chunk_z), count)| MapInfoChunkCount {
                chunk_x,
                chunk_z,
                count,
            })
            .collect();
        payload
    }
}

impl MapInfoOverlaySnapshot {
    fn from_payloads(
        payloads: impl IntoIterator<Item = MapInfoTilePayload>,
        cached_tile_count: usize,
        rebuilt_tile_count: usize,
    ) -> Self {
        let mut snapshot = Self {
            cached_tile_count,
            rebuilt_tile_count,
            ..Self::default()
        };
        for payload in payloads {
            snapshot.entities.extend(payload.entities);
            snapshot.block_entities.extend(payload.block_entities);
            snapshot
                .pending_tick_counts
                .extend(payload.pending_tick_counts);
            snapshot
                .hardcoded_spawn_areas
                .extend(payload.hardcoded_spawn_areas);
        }
        snapshot
    }
}

struct MapInfoCache {
    root: PathBuf,
}

impl MapInfoCache {
    fn for_world(world_path: &Path) -> Self {
        Self {
            root: crate::utils::file_ops::cache_subdir(MAP_INFO_CACHE_DIRECTORY)
                .join(format!("v{CACHE_VERSION}"))
                .join(world_cache_id(world_path)),
        }
    }

    fn load_index(&self) -> Result<MapInfoIndex> {
        let path = self.root.join(INDEX_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MapInfoIndex {
                    version: CACHE_VERSION,
                    entries: BTreeMap::new(),
                });
            }
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        match postcard::from_bytes::<MapInfoIndex>(&bytes) {
            Ok(index) if index.version == CACHE_VERSION => Ok(index),
            Ok(_) | Err(_) => Ok(MapInfoIndex {
                version: CACHE_VERSION,
                entries: BTreeMap::new(),
            }),
        }
    }

    fn store_index(&self, index: &MapInfoIndex) -> Result<()> {
        let bytes = postcard::to_allocvec(index).context("encode map information index")?;
        write_atomic(&self.root.join(INDEX_FILE), &bytes)
    }

    fn load_tile(&self, key: MapInfoTileKey, expected_hash: u128) -> Result<MapInfoTilePayload> {
        let path = self.tile_path(key);
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if xxh3_128(&bytes) != expected_hash {
            bail!("map information tile checksum did not match index");
        }
        postcard::from_bytes(&bytes).context("decode map information tile")
    }

    fn store_tile(&self, key: MapInfoTileKey, payload: &MapInfoTilePayload) -> Result<u128> {
        let bytes = postcard::to_allocvec(payload).context("encode map information tile")?;
        let payload_hash = xxh3_128(&bytes);
        write_atomic(&self.tile_path(key), &bytes)?;
        Ok(payload_hash)
    }

    fn remove_tile(&self, key: MapInfoTileKey) -> Result<()> {
        let path = self.tile_path(key);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }

    fn tile_path(&self, key: MapInfoTileKey) -> PathBuf {
        self.root.join(format!(
            "tile.d{}.x{}.z{}.bin",
            key.dimension_id, key.tile_x, key.tile_z
        ))
    }
}

fn requested_tile_keys(
    dimension: Dimension,
    chunks_per_tile: u16,
    tile_coordinates: &[(i32, i32)],
) -> Result<Vec<MapInfoTileKey>> {
    if chunks_per_tile == 0 {
        bail!("map information tiles require at least one chunk per edge");
    }
    Ok(tile_coordinates
        .iter()
        .map(|&(tile_x, tile_z)| MapInfoTileKey {
            dimension_id: dimension.id(),
            tile_x,
            tile_z,
            chunks_per_tile,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn chunks_for_keys(keys: &[MapInfoTileKey]) -> Result<Vec<ChunkPos>> {
    let mut chunks = Vec::new();
    for key in keys {
        let edge = i32::from(key.chunks_per_tile);
        if edge == 0 {
            bail!("map information tiles require at least one chunk per edge");
        }
        let start_x = key
            .tile_x
            .checked_mul(edge)
            .context("map information tile X overflow")?;
        let start_z = key
            .tile_z
            .checked_mul(edge)
            .context("map information tile Z overflow")?;
        let end_x = start_x
            .checked_add(edge)
            .context("map information tile X range overflow")?;
        let end_z = start_z
            .checked_add(edge)
            .context("map information tile Z range overflow")?;
        for chunk_z in start_z..end_z {
            for chunk_x in start_x..end_x {
                chunks.push(ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                    dimension: Dimension::from_id(key.dimension_id),
                });
            }
        }
    }
    Ok(chunks)
}

fn records_by_tile(
    records: Vec<ChunkRecordQueryResult>,
    chunks_per_tile: u16,
) -> BTreeMap<MapInfoTileKey, Vec<ChunkRecordQueryResult>> {
    let edge = i32::from(chunks_per_tile).max(1);
    let mut grouped = BTreeMap::new();
    for record in records {
        let key = MapInfoTileKey {
            dimension_id: record.pos.dimension.id(),
            tile_x: record.pos.x.div_euclid(edge),
            tile_z: record.pos.z.div_euclid(edge),
            chunks_per_tile,
        };
        grouped.entry(key).or_insert_with(Vec::new).push(record);
    }
    grouped
}

fn map_info_record_query() -> ChunkRecordQuery {
    ChunkRecordQuery {
        entities: true,
        block_entities: true,
        pending_ticks: true,
        hardcoded_spawn_areas: true,
    }
}

fn world_cache_id(world_path: &Path) -> String {
    let stable_path = world_path
        .canonicalize()
        .unwrap_or_else(|_| world_path.to_path_buf());
    format!(
        "{:032x}",
        xxh3_128(stable_path.as_os_str().as_encoded_bytes())
    )
}

fn cancel_if_requested(cancel: &CancelFlag) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("map information cache request cancelled");
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("map information cache path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temporary_path = path.with_extension(format!(
        "bin.{}.tmp",
        TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = File::create(&temporary_path)
        .with_context(|| format!("create {}", temporary_path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", temporary_path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", temporary_path.display()))?;
    drop(file);
    if let Err(rename_error) = fs::rename(&temporary_path, path) {
        if path.exists() {
            fs::remove_file(path).with_context(|| format!("replace {}", path.display()))?;
            fs::rename(&temporary_path, path)
                .with_context(|| format!("replace {}", path.display()))?;
        } else {
            return Err(rename_error).with_context(|| format!("rename {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_key_uses_euclidean_coordinates_for_negative_chunks() {
        let keys = requested_tile_keys(Dimension::Overworld, 8, &[(-1, -1)]).expect("keys");
        let chunks = chunks_for_keys(&keys).expect("chunks");

        assert_eq!(
            chunks.first().map(|chunk| (chunk.x, chunk.z)),
            Some((-8, -8))
        );
        assert_eq!(
            chunks.last().map(|chunk| (chunk.x, chunk.z)),
            Some((-1, -1))
        );
    }

    #[test]
    fn invalidation_deduplicates_tiles_owned_by_multiple_chunks() {
        let chunks = BTreeSet::from([
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 7,
                z: 7,
                dimension: Dimension::Overworld,
            },
        ]);
        let keys = chunks
            .iter()
            .map(|chunk| MapInfoTileKey {
                dimension_id: chunk.dimension.id(),
                tile_x: chunk.x.div_euclid(8),
                tile_z: chunk.z.div_euclid(8),
                chunks_per_tile: 8,
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn map_info_query_workers_are_bounded_and_require_enough_tiles() {
        assert_eq!(map_info_query_worker_count(1, 8), 1);
        assert_eq!(map_info_query_worker_count(8, 8), 1);
        assert_eq!(map_info_query_worker_count(9, 8), 2);
        assert_eq!(map_info_query_worker_count(64, 2), 2);
        assert_eq!(map_info_query_worker_count(128, 32), 4);
    }
}

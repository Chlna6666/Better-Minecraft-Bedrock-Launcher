//! Writable Bedrock world helpers for render-tool integrations.
//!
//! Rendering entry points stay read-only by default. This module is the
//! explicit write boundary for applications that need to edit records and then
//! invalidate affected rendered tiles, overlays, or metadata.

use crate::Result;
use std::collections::BTreeSet;
use std::path::Path;

pub use bedrock_world::{
    biome::{Biome2d, Biome3d, HeightMap2d, BiomeStorage},
    block::{BlockEntityRecord, BlockPos, BlockEntity},
    chunk::{
        ChunkPos, ChunkRecordTag, ChunkVersion, Dimension, HardcodedSpawnAreaKind,
        HardcodedSpawnArea,
    },
    GlobalRecordKind, Global,
    entity::{ActorRecord, ActorSource, ActorUid, Actor},
    map_item::{KnownFields, MapItemId, Pixels, SavedData},
    nbt::NbtTag,
    query::{
        ChunkDetail, RegionOverlayQuery, RegionOverlayQueryOptions, SelectionStats,
        SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize, VillageOverlayIndex, WriteGuard,
        block_tip, chunk_detail, region_overlays, selection_stats,
        query_slime_chunk_windows,
    },
    surface::{CancelFlag, WorldScanOptions},
    world::{World, OpenOptions},
};

/// Describes the render-side state that should be refreshed after a world edit.
///
/// Applications can use this as a conservative cache invalidation contract:
/// affected chunks imply tile/image refresh, overlay changes imply region
/// overlay reload, and metadata changes imply a manifest/index reload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapEditInvalidation {
    affected_chunks: BTreeSet<ChunkPos>,
    refresh_metadata: bool,
    refresh_overlays: bool,
    clear_tile_cache: bool,
}

impl MapEditInvalidation {
    /// Creates an invalidation that only refreshes metadata panels or indices.
    #[must_use]
    pub const fn metadata() -> Self {
        Self {
            affected_chunks: BTreeSet::new(),
            refresh_metadata: true,
            refresh_overlays: false,
            clear_tile_cache: false,
        }
    }

    /// Creates an invalidation that refreshes overlays but not rendered tiles.
    #[must_use]
    pub const fn overlays() -> Self {
        Self {
            affected_chunks: BTreeSet::new(),
            refresh_metadata: false,
            refresh_overlays: true,
            clear_tile_cache: false,
        }
    }

    /// Creates an invalidation for one edited chunk.
    #[must_use]
    pub fn chunk(pos: ChunkPos) -> Self {
        let mut invalidation = Self::default();
        invalidation.affected_chunks.insert(pos);
        invalidation.refresh_overlays = true;
        invalidation.clear_tile_cache = true;
        invalidation
    }

    /// Creates an invalidation for multiple edited chunks.
    #[must_use]
    pub fn chunks(positions: impl IntoIterator<Item = ChunkPos>) -> Self {
        let mut invalidation = Self::default();
        invalidation.affected_chunks.extend(positions);
        if !invalidation.affected_chunks.is_empty() {
            invalidation.refresh_overlays = true;
            invalidation.clear_tile_cache = true;
        }
        invalidation
    }

    /// Merges another invalidation into this one.
    pub fn merge(&mut self, other: Self) {
        self.affected_chunks.extend(other.affected_chunks);
        self.refresh_metadata |= other.refresh_metadata;
        self.refresh_overlays |= other.refresh_overlays;
        self.clear_tile_cache |= other.clear_tile_cache;
    }

    /// Returns chunk positions that must be rerendered.
    #[must_use]
    pub const fn affected_chunks(&self) -> &BTreeSet<ChunkPos> {
        &self.affected_chunks
    }

    /// Returns true when world metadata or tile manifests should be refreshed.
    #[must_use]
    pub const fn refresh_metadata(&self) -> bool {
        self.refresh_metadata
    }

    /// Returns true when entity, village, HSA, or block-entity overlays should reload.
    #[must_use]
    pub const fn refresh_overlays(&self) -> bool {
        self.refresh_overlays
    }

    /// Returns true when rendered tile image caches may be stale.
    #[must_use]
    pub const fn clear_tile_cache(&self) -> bool {
        self.clear_tile_cache
    }

    /// Marks the invalidation as requiring metadata refresh.
    #[must_use]
    pub const fn with_metadata(mut self) -> Self {
        self.refresh_metadata = true;
        self
    }

    /// Marks the invalidation as requiring overlay refresh.
    #[must_use]
    pub const fn with_overlays(mut self) -> Self {
        self.refresh_overlays = true;
        self
    }

    /// Marks the invalidation as requiring tile-cache cleanup.
    #[must_use]
    pub const fn with_tile_cache_clear(mut self) -> Self {
        self.clear_tile_cache = true;
        self
    }
}

/// Explicit writable world facade for map viewers and editor tools.
///
/// Use this instead of turning the normal render source writable. The wrapped
/// [`World`] is opened with `read_only = false`; callers are still
/// expected to collect explicit user confirmation before invoking mutating
/// methods.
pub struct MapWorldEditor {
    world: World,
}

impl MapWorldEditor {
    /// Opens a writable Bedrock world from the world folder path.
    ///
    /// The path must be the Minecraft world root, not the nested `db`
    /// directory. Pre-LevelDB `chunks.dat` worlds remain read-only in
    /// `bedrock-world`; opening them here succeeds only when the backend allows
    /// the requested writes.
    ///
    /// # Errors
    ///
    /// Returns world-format detection, storage, or `LevelDB` open errors.
    pub fn open_writable(world_path: impl AsRef<Path>) -> Result<Self> {
        let options = OpenOptions {
            read_only: false,
            ..OpenOptions::default()
        };
        Self::open_with_options(world_path, options)
    }

    /// Opens a world with custom options.
    ///
    /// This is intended for tooling that needs a specific format hint. If
    /// `options.read_only` is true, mutating methods will return the underlying
    /// `bedrock-world` read-only error.
    ///
    /// # Errors
    ///
    /// Returns world-format detection, storage, or `LevelDB` open errors.
    pub fn open_with_options(
        world_path: impl AsRef<Path>,
        options: OpenOptions,
    ) -> Result<Self> {
        let world = World::open(world_path, options)?;
        Ok(Self { world })
    }

    /// Wraps an existing world handle.
    #[must_use]
    pub const fn from_world(world: World) -> Self {
        Self { world }
    }

    /// Returns the wrapped world handle for advanced `bedrock-world` calls.
    #[must_use]
    pub const fn world(&self) -> &World {
        &self.world
    }

    /// Consumes this facade and returns the wrapped world.
    #[must_use]
    pub fn into_world(self) -> World {
        self.world
    }

    /// Reads one Bedrock map item by id.
    ///
    /// # Errors
    ///
    /// Returns storage or NBT parse errors.
    pub fn map_item(&self, id: &MapItemId) -> Result<Option<SavedData>> {
        Ok(self.world.map_item(id)?)
    }

    /// Reads Bedrock map items.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or NBT parse errors.
    pub fn map_items(&self, options: WorldScanOptions) -> Result<Vec<SavedData>> {
        Ok(self.world.map_items(options)?)
    }

    /// Saves one Bedrock map item after `bedrock-world` round-trip validation.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn save_map_item(&self, item: &SavedData) -> Result<MapEditInvalidation> {
        self.world.save_map_item(item)?;
        Ok(MapEditInvalidation::metadata())
    }

    /// Deletes one Bedrock map item.
    ///
    /// # Errors
    ///
    /// Returns read-only or storage errors.
    pub fn delete_map_item(&self, id: &MapItemId) -> Result<MapEditInvalidation> {
        self.world.delete_map_item(id)?;
        Ok(MapEditInvalidation::metadata())
    }

    /// Reads one global record by kind.
    ///
    /// # Errors
    ///
    /// Returns storage or NBT parse errors.
    pub fn global(&self, kind: GlobalRecordKind) -> Result<Option<Global>> {
        Ok(self.world.global(kind)?)
    }

    /// Scans typed global records.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or NBT parse errors.
    pub fn globals(&self, options: WorldScanOptions) -> Result<Vec<Global>> {
        Ok(self.world.globals(options)?)
    }

    /// Writes one global record after `bedrock-world` roundtrip validation.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn save_global(&self, record: &Global) -> Result<MapEditInvalidation> {
        self.world.save_global(record)?;
        Ok(MapEditInvalidation::metadata())
    }

    /// Deletes one global record.
    ///
    /// # Errors
    ///
    /// Returns read-only or storage errors.
    pub fn delete_global(&self, kind: GlobalRecordKind) -> Result<MapEditInvalidation> {
        self.world.delete_global(kind)?;
        Ok(MapEditInvalidation::metadata())
    }

    /// Reads the 16x16 height map from `Data2D` or `Data3D`.
    ///
    /// # Errors
    ///
    /// Returns storage or heightmap parse errors.
    pub fn heightmap(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        Ok(self.world.heightmap(pos)?)
    }

    /// Writes the height map for a chunk.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn put_heightmap(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<MapEditInvalidation> {
        self.world
            .put_heightmap(pos, version, height_map)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Writes a full `Data3D` biome payload for a chunk.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn put_biome_storage(&self, pos: ChunkPos, biome: Biome3d) -> Result<MapEditInvalidation> {
        self.world.put_biome_storage(pos, biome)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Scans hardcoded spawn areas.
    ///
    /// # Errors
    ///
    /// Returns storage, cancellation, or HSA validation errors.
    pub fn hardcoded_spawn_areas(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<HardcodedSpawnArea>)>> {
        Ok(self.world.hardcoded_spawn_areas(options)?)
    }

    /// Replaces hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn save_hardcoded_spawn_areas(
        &self,
        pos: ChunkPos,
        areas: &[HardcodedSpawnArea],
    ) -> Result<MapEditInvalidation> {
        self.world.save_hardcoded_spawn_areas(pos, areas)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Deletes hardcoded spawn areas for one chunk.
    ///
    /// # Errors
    ///
    /// Returns read-only or storage errors.
    pub fn delete_hardcoded_spawn_areas(&self, pos: ChunkPos) -> Result<MapEditInvalidation> {
        self.world.delete_hardcoded_spawn_areas(pos)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Reads all block entities from a chunk.
    ///
    /// # Errors
    ///
    /// Returns storage or NBT parse errors.
    pub fn block_entities(&self, pos: ChunkPos) -> Result<Vec<BlockEntityRecord>> {
        Ok(self.world.block_entities(pos)?)
    }

    /// Replaces all block entities for a chunk after coordinate validation.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn put_block_entities(
        &self,
        pos: ChunkPos,
        entities: &[BlockEntity],
    ) -> Result<MapEditInvalidation> {
        self.world.put_block_entities(pos, entities)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Edits one block entity in place.
    ///
    /// # Errors
    ///
    /// Returns validation, read-only, serialization, or storage errors.
    pub fn edit_block_entity_at<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<MapEditInvalidation>
    where
        F: FnOnce(&mut NbtTag) -> bedrock_world::error::Result<()>,
    {
        self.world.edit_block_entity_at(pos, block, edit)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Deletes one block entity by absolute block position.
    ///
    /// # Errors
    ///
    /// Returns read-only or storage errors.
    pub fn delete_block_entity_at(
        &self,
        pos: ChunkPos,
        block: BlockPos,
    ) -> Result<MapEditInvalidation> {
        self.world.delete_block_entity_at(pos, block)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Reads modern and legacy actors associated with a chunk.
    ///
    /// # Errors
    ///
    /// Returns storage, NBT parse, or actor digest validation errors.
    pub fn actors(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        Ok(self.world.actors(pos)?)
    }

    /// Writes a modern `actorprefix` actor and updates the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn put_actor(&self, pos: ChunkPos, actor: &Actor) -> Result<MapEditInvalidation> {
        self.world.put_actor(pos, actor)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Deletes a modern actor and removes it from the chunk `digp` digest.
    ///
    /// # Errors
    ///
    /// Returns read-only, digest validation, or storage errors.
    pub fn delete_actor(&self, pos: ChunkPos, uid: ActorUid) -> Result<MapEditInvalidation> {
        self.world.delete_actor(pos, uid)?;
        Ok(MapEditInvalidation::chunk(pos).with_metadata())
    }

    /// Moves a modern actor between chunk digests.
    ///
    /// # Errors
    ///
    /// Returns read-only, validation, serialization, or storage errors.
    pub fn move_actor(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: &Actor,
    ) -> Result<MapEditInvalidation> {
        self.world.move_actor(from, to, actor)?;
        Ok(MapEditInvalidation::chunks([from, to]).with_metadata())
    }
}

#[cfg(test)]
mod tests {
    use super::{ChunkPos, Dimension, MapEditInvalidation};

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn invalidation_merges_chunk_and_metadata_flags() {
        let mut invalidation = MapEditInvalidation::chunk(chunk(1, 2));
        invalidation.merge(MapEditInvalidation::metadata());
        invalidation.merge(MapEditInvalidation::chunk(chunk(3, 4)));

        assert!(invalidation.refresh_metadata());
        assert!(invalidation.refresh_overlays());
        assert!(invalidation.clear_tile_cache());
        assert!(invalidation.affected_chunks().contains(&chunk(1, 2)));
        assert!(invalidation.affected_chunks().contains(&chunk(3, 4)));
    }
}

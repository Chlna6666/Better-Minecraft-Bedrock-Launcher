//! Asynchronous adapters for Minecraft Bedrock world operations.

use super::*;

impl<S> World<S>
where
    S: StorageBackend,
{
    #[cfg(feature = "async")]
    /// List players.
    pub async fn players_async(&self) -> Result<Vec<PlayerId>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.players())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Classify keys.
    pub async fn classify_keys_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<BTreeMap<String, usize>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.classify_keys(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List chunk positions.
    pub async fn chunk_positions_async(&self, options: WorldScanOptions) -> Result<Vec<ChunkPos>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.chunk_positions(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions.
    pub async fn render_chunk_positions_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.render_chunk_positions(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions in region.
    pub async fn region_render_chunk_positions_async(
        &self,
        region: Region,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || {
            world.region_chunk_positions(region, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Discover chunk bounds.
    pub async fn discover_chunk_bounds_async(
        &self,
        dimension: crate::Dimension,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkBounds>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || {
            world.discover_chunk_bounds(dimension, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Nearest loaded chunk to spawn.
    pub async fn nearest_loaded_chunk_to_spawn_async(
        &self,
        dimension: crate::Dimension,
        spawn_block_x: i32,
        spawn_block_z: i32,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkPos>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || {
            world.nearest_loaded_chunk_to_spawn(
                dimension,
                spawn_block_x,
                spawn_block_z,
                options,
            )
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Parse chunk.
    pub async fn scan_chunk_async(
        &self,
        pos: ChunkPos,
        options: ScanOptions,
    ) -> Result<crate::scan::Chunk> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.scan_chunk(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunk.
    pub async fn load_render_chunk_async(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_data(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunks.
    pub async fn load_render_chunks_async(
        &self,
        positions: Vec<ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<Vec<ChunkData>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || {
            world.query_chunk_data_many(positions, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render region.
    pub async fn load_render_region_async(
        &self,
        region: Region,
        options: RegionLoadOptions,
    ) -> Result<RegionLoad> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_region(region, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan entities.
    pub async fn scan_entities_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<Actor>, ScanReport)> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.scan_entities(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan block entities.
    pub async fn scan_block_entities_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<BlockEntity>, ScanReport)> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.scan_block_entities(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan items.
    pub async fn scan_items_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, ScanReport)> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.scan_items(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async adapter for [`Self::villages`].
    #[cfg(feature = "async")]
    pub async fn villages_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<Entry>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.villages(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::map_item`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn map_item_async(&self, id: MapItemId) -> Result<Option<SavedData>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.map_item(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::map_items`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn map_items_async(&self, options: WorldScanOptions) -> Result<Vec<SavedData>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.map_items(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::save_map_item`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn save_map_item_async(&self, record: SavedData) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.save_map_item(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_map_item`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_map_item_async(&self, id: MapItemId) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.delete_map_item(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::global`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn global_async(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<Global>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.global(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::globals`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn globals_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<Global>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.globals(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::save_global`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn save_global_async(&self, record: Global) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.save_global(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_global`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_global_async(&self, kind: GlobalRecordKind) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.delete_global(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::heightmap`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or heightmap parse errors.
    #[cfg(feature = "async")]
    pub async fn heightmap_async(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.heightmap(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_heightmap`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_heightmap_async(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.put_heightmap(pos, version, height_map))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_biome_storage`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_biome_storage_async(&self, pos: ChunkPos, biome: Biome3d) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.put_biome_storage(pos, biome))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::hardcoded_spawn_areas`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or HSA parse errors.
    #[cfg(feature = "async")]
    pub async fn hardcoded_spawn_areas_async(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<HardcodedSpawnArea>)>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.hardcoded_spawn_areas(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::save_hardcoded_spawn_areas`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn save_hardcoded_spawn_areas_async(
        &self,
        pos: ChunkPos,
        areas: Vec<HardcodedSpawnArea>,
    ) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.save_hardcoded_spawn_areas(pos, &areas))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_hardcoded_spawn_areas`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_hardcoded_spawn_areas_async(&self, pos: ChunkPos) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.delete_hardcoded_spawn_areas(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::block_entities`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or block-entity parse errors.
    #[cfg(feature = "async")]
    pub async fn block_entities_async(&self, pos: ChunkPos) -> Result<Vec<BlockEntityRecord>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.block_entities(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_block_entities`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_block_entities_async(
        &self,
        pos: ChunkPos,
        entities: Vec<BlockEntity>,
    ) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.put_block_entities(pos, &entities))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::edit_block_entity_at`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn edit_block_entity_at_async<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut NbtTag) -> Result<()> + Send + 'static,
    {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.edit_block_entity_at(pos, block, edit))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_block_entity_at`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_block_entity_at_async(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.delete_block_entity_at(pos, block))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::actors`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or actor digest validation errors.
    #[cfg(feature = "async")]
    pub async fn actors_async(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.actors(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_actor`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_actor_async(&self, pos: ChunkPos, actor: Actor) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.put_actor(pos, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_actor`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_actor_async(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.delete_actor(pos, uid))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::move_actor`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn move_actor_async(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: Actor,
    ) -> Result<()> {
        let world = self.task_clone();
        tokio::task::spawn_blocking(move || world.move_actor(from, to, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    #[must_use]
    fn task_clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            options: self.options.clone(),
            storage: self.storage.clone(),
            format: self.format,
        }
    }
}

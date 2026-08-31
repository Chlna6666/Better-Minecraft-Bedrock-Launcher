//! Async wrappers for Minecraft Bedrock world operations.

use super::*;

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    #[cfg(feature = "async")]
    /// List players.
    pub async fn list_players(&self) -> Result<Vec<PlayerId>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_players_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Classify keys.
    pub async fn classify_keys(
        &self,
        options: WorldScanOptions,
    ) -> Result<BTreeMap<String, usize>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.classify_keys_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List chunk positions.
    pub async fn list_chunk_positions(&self, options: WorldScanOptions) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_chunk_positions_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions.
    pub async fn list_render_chunk_positions(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.list_render_chunk_positions_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// List render chunk positions in region.
    pub async fn list_render_chunk_positions_in_region(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.list_chunk_positions_in_region_blocking(region, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Discover chunk bounds.
    pub async fn discover_chunk_bounds(
        &self,
        dimension: crate::Dimension,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkBounds>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.discover_chunk_bounds_blocking(dimension, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Nearest loaded chunk to spawn.
    pub async fn nearest_loaded_chunk_to_spawn(
        &self,
        dimension: crate::Dimension,
        spawn_block_x: i32,
        spawn_block_z: i32,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkPos>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.nearest_loaded_chunk_to_spawn_blocking(
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
    pub async fn parse_chunk(
        &self,
        pos: ChunkPos,
        options: WorldParseOptions,
    ) -> Result<ParsedChunkData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.parse_chunk_with_options_blocking(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunk.
    pub async fn load_render_chunk(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_data_blocking(pos, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render chunks.
    pub async fn load_render_chunks(
        &self,
        positions: Vec<ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<Vec<ChunkData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || {
            world.query_chunk_data_many_blocking(positions, options)
        })
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Load render region.
    pub async fn load_render_region(
        &self,
        region: WorldChunkQueryRegion,
        options: WorldChunkQueryRegionLoadOptions,
    ) -> Result<WorldChunkQueryRegionData> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.query_chunk_region_blocking(region, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan entities.
    pub async fn scan_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedEntity>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_entities_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan block entities.
    pub async fn scan_block_entities(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ParsedBlockEntity>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_block_entities_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan items.
    pub async fn scan_items(
        &self,
        options: WorldScanOptions,
    ) -> Result<(Vec<ItemStack>, WorldParseReport)> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_items_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan maps.
    pub async fn scan_maps(&self) -> Result<Vec<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_maps_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan villages.
    pub async fn scan_villages(&self) -> Result<Vec<ParsedVillageData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_villages_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Scan globals.
    pub async fn scan_globals(&self) -> Result<Vec<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_globals_blocking())
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::read_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn read_map_record(&self, id: MapRecordId) -> Result<Option<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.read_map_record_blocking(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_map_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or map parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_map_records(&self, options: WorldScanOptions) -> Result<Vec<ParsedMapData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_map_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::write_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn write_map_record(&self, record: ParsedMapData) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.write_map_record_blocking(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_map_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_map_record(&self, id: MapRecordId) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_map_record_blocking(&id))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::read_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn read_global_record(
        &self,
        kind: GlobalRecordKind,
    ) -> Result<Option<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.read_global_record_blocking(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_global_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or global parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_global_records(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ParsedGlobalData>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_global_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::write_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn write_global_record(&self, record: ParsedGlobalData) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.write_global_record_blocking(&record))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_global_record_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_global_record(&self, kind: GlobalRecordKind) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_global_record_blocking(kind))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::get_heightmap_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or heightmap parse errors.
    #[cfg(feature = "async")]
    pub async fn get_heightmap(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.get_heightmap_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_heightmap_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_heightmap(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_heightmap_blocking(pos, version, height_map))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_biome_storage_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_biome_storage(&self, pos: ChunkPos, biome: Biome3d) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_biome_storage_blocking(pos, biome))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::scan_hsa_records_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, cancellation, or HSA parse errors.
    #[cfg(feature = "async")]
    pub async fn scan_hsa_records(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<(ChunkPos, Vec<ParsedHardcodedSpawnArea>)>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.scan_hsa_records_blocking(options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_hsa_for_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_hsa_for_chunk(
        &self,
        pos: ChunkPos,
        areas: Vec<ParsedHardcodedSpawnArea>,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_hsa_for_chunk_blocking(pos, &areas))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_hsa_for_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_hsa_for_chunk(&self, pos: ChunkPos) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_hsa_for_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::block_entities_in_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or block-entity parse errors.
    #[cfg(feature = "async")]
    pub async fn block_entities_in_chunk(&self, pos: ChunkPos) -> Result<Vec<BlockEntityRecord>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.block_entities_in_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_block_entities_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_block_entities(
        &self,
        pos: ChunkPos,
        entities: Vec<ParsedBlockEntity>,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_block_entities_blocking(pos, &entities))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::edit_block_entity_at_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn edit_block_entity_at<F>(
        &self,
        pos: ChunkPos,
        block: BlockPos,
        edit: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut NbtTag) -> Result<()> + Send + 'static,
    {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.edit_block_entity_at_blocking(pos, block, edit))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_block_entity_at_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_block_entity_at(&self, pos: ChunkPos, block: BlockPos) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_block_entity_at_blocking(pos, block))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::actors_in_chunk_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, storage, or actor digest validation errors.
    #[cfg(feature = "async")]
    pub async fn actors_in_chunk(&self, pos: ChunkPos) -> Result<Vec<ActorRecord>> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.actors_in_chunk_blocking(pos))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::put_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn put_actor(&self, pos: ChunkPos, actor: ParsedEntity) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.put_actor_blocking(pos, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::delete_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, or storage errors.
    #[cfg(feature = "async")]
    pub async fn delete_actor(&self, pos: ChunkPos, uid: ActorUid) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.delete_actor_blocking(pos, uid))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    /// Async wrapper for [`Self::move_actor_blocking`].
    ///
    /// # Errors
    ///
    /// Returns join, read-only, validation, or storage errors.
    #[cfg(feature = "async")]
    pub async fn move_actor(
        &self,
        from: ChunkPos,
        to: ChunkPos,
        actor: ParsedEntity,
    ) -> Result<()> {
        let world = self.blocking_clone();
        tokio::task::spawn_blocking(move || world.move_actor_blocking(from, to, &actor))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    #[must_use]
    fn blocking_clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            options: self.options.clone(),
            storage: self.storage.clone(),
            format: self.format,
        }
    }
}

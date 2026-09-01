//! Minecraft player discovery, chunk, SubChunk, biome, and surface queries.

use super::*;

const CHUNK_ANCHOR_TAGS: [ChunkRecordTag; 8] = [
    ChunkRecordTag::Version,
    ChunkRecordTag::VersionOld,
    ChunkRecordTag::LegacyVersion,
    ChunkRecordTag::Data3D,
    ChunkRecordTag::Data2D,
    ChunkRecordTag::Data2DLegacy,
    ChunkRecordTag::LegacyTerrain,
    ChunkRecordTag::FinalizedState,
];
const MAX_ENCODED_CHUNK_KEY_BYTES: usize = 14;

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Checks whether one chunk has a canonical persisted Bedrock chunk record.
    pub fn is_chunk_saved(&self, pos: ChunkPos) -> Result<bool> {
        Ok(self.chunk_keys_exist([pos], false)?.pop().unwrap_or(false))
    }

    pub(crate) fn chunk_keys_exist(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        include_any_record: bool,
    ) -> Result<Vec<bool>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let key_count = positions.len().saturating_mul(CHUNK_ANCHOR_TAGS.len());
        let mut keys = crate::storage::StorageKeyBatchBuilder::with_capacity(
            key_count.saturating_mul(MAX_ENCODED_CHUNK_KEY_BYTES),
            key_count,
        );
        for &pos in &positions {
            for tag in CHUNK_ANCHOR_TAGS {
                let encoded = ChunkKey::new(pos, tag).encode_inline();
                keys.push(encoded.as_bytes());
            }
        }
        let keys = keys.finish();
        let values = self.storage().get_many(keys.keys())?;

        let mut exists = positions
            .iter()
            .enumerate()
            .map(|(position_index, _)| {
                let base = position_index.saturating_mul(CHUNK_ANCHOR_TAGS.len());
                (0..CHUNK_ANCHOR_TAGS.len()).any(|tag_index| {
                    values
                        .get(base.saturating_add(tag_index))
                        .is_some_and(Option::is_some)
                })
            })
            .collect::<Vec<_>>();

        if include_any_record {
            for (&pos, exists) in positions.iter().zip(&mut exists) {
                if *exists {
                    continue;
                }
                let anchor_key = ChunkKey::new(pos, ChunkRecordTag::Version).encode_inline();
                let prefix = &anchor_key.as_bytes()[..anchor_key.len().saturating_sub(1)];
                self.storage().for_each_prefix_key(
                    prefix,
                    StorageReadOptions::default(),
                    &mut |key| {
                        if ChunkKey::decode(key).is_ok_and(|key| key.pos == pos) {
                            *exists = true;
                            return Ok(StorageVisitorControl::Stop);
                        }
                        Ok(StorageVisitorControl::Continue)
                    },
                )?;
            }
        }

        Ok(exists)
    }

    /// List players.
    pub fn players(&self) -> Result<Vec<PlayerId>> {
        let mut players = Vec::new();
        if self.storage().get(b"~local_player")?.is_some() {
            players.push(PlayerId::Local);
        }
        self.storage().for_each_prefix_key(
            b"player_",
            StorageReadOptions::default(),
            &mut |key| {
                if let Some(player) = PlayerId::from_storage_key(key) {
                    players.push(player);
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(players)
    }

    /// Classify keys.
    pub fn classify_keys(
        &self,
        options: WorldScanOptions,
    ) -> Result<BTreeMap<String, usize>> {
        let mut counts = BTreeMap::new();
        let mut allocation_free_counts = HashMap::<BedrockDbKeyKind, usize>::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                let kind = BedrockDbKeyKind::classify(key);
                if matches!(
                    kind,
                    BedrockDbKeyKind::Other | BedrockDbKeyKind::Village | BedrockDbKeyKind::Global
                ) {
                    let key = BedrockDbKey::decode(key);
                    *counts.entry(key.summary_kind()).or_default() += 1;
                } else {
                    *allocation_free_counts.entry(kind).or_default() += 1;
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        for (kind, count) in allocation_free_counts {
            *counts.entry(kind.summary_kind()).or_default() += count;
        }
        emit_progress(&options, entries_seen);
        Ok(counts)
    }

    /// List chunk positions.
    pub fn chunk_positions(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let mut positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    positions.insert(chunk_key.pos);
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(positions.into_iter().collect())
    }

    /// List render chunk positions.
    pub fn render_chunk_positions(
        &self,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let started = Instant::now();
        log::debug!(
            "listing render chunk positions (threading={:?}, queue_depth={}, progress_interval={})",
            options.threading,
            options.pipeline.queue_depth,
            options.pipeline.progress_interval
        );
        let mut positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        let outcome =
            self.storage()
                .for_each_key(to_storage_read_options(&options), &mut |key| {
                    check_cancelled(&options)?;
                    entries_seen = entries_seen.saturating_add(1);
                    if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                        if chunk_key.tag.is_render_chunk_record() {
                            positions.insert(chunk_key.pos);
                        }
                    }
                    if entries_seen.is_multiple_of(8192) {
                        emit_progress(&options, entries_seen);
                    }
                    Ok(StorageVisitorControl::Continue)
                })?;
        let positions = positions.into_iter().collect::<Vec<_>>();
        log::debug!(
            "render chunk position listing complete (entries_seen={}, positions={}, visited={}, tables_scanned={}, worker_threads={}, queue_wait_ms={}, cancel_checks={}, elapsed_ms={})",
            entries_seen,
            positions.len(),
            outcome.visited,
            outcome.tables_scanned,
            outcome.worker_threads,
            outcome.queue_wait_ms,
            outcome.cancel_checks,
            started.elapsed().as_millis()
        );
        Ok(positions)
    }

    #[allow(clippy::too_many_lines)]
    /// List render chunk positions in region.
    pub fn region_chunk_positions(
        &self,
        region: Region,
        options: WorldScanOptions,
    ) -> Result<Vec<ChunkPos>> {
        let started = Instant::now();
        validate_render_region(region)?;
        let x_count = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
        let z_count = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
        let capacity = usize::try_from(x_count.saturating_mul(z_count))
            .map_err(|_| BedrockWorldError::Validation("render region is too large".to_string()))?;
        let mut positions = Vec::with_capacity(capacity);
        for z in region.min_chunk_z..=region.max_chunk_z {
            for x in region.min_chunk_x..=region.max_chunk_x {
                positions.push(ChunkPos {
                    x,
                    z,
                    dimension: region.dimension,
                });
            }
        }
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let worker_count = options.threading.resolve_checked(positions.len())?;
        log::debug!(
            "indexing render chunk region (dimension={:?}, min=({}, {}), max=({}, {}), workers={})",
            region.dimension,
            region.min_chunk_x,
            region.min_chunk_z,
            region.max_chunk_x,
            region.max_chunk_z,
            worker_count
        );
        if worker_count == 1 {
            let render_positions = positions
                .into_iter()
                .filter_map(
                    |pos| match self.has_render_chunk_records(pos, &options) {
                        Ok(true) => Some(Ok(pos)),
                        Ok(false) => None,
                        Err(error) => Some(Err(error)),
                    },
                )
                .collect::<Result<Vec<_>>>()?;
            log::debug!(
                "render chunk region index complete (dimension={:?}, candidates={}, positions={}, workers={}, queue_depth=0, elapsed_ms={})",
                region.dimension,
                capacity,
                render_positions.len(),
                worker_count,
                started.elapsed().as_millis()
            );
            return Ok(render_positions);
        }

        let scan_options = WorldScanOptions {
            threading: WorldThreadingOptions::Single,
            pipeline: options.pipeline,
            cancel: options.cancel.clone(),
            progress: options.progress.clone(),
        };
        let next_position = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let queue_depth = options
            .pipeline
            .resolve_queue_depth(worker_count, positions.len());
        let (sender, receiver) = mpsc::sync_channel::<Result<Option<ChunkPos>>>(queue_depth);
        let executor = world_executor(worker_count)?;
        executor.pool.scope(|scope| {
            for worker_index in 0..worker_count {
                let next_position = Arc::clone(&next_position);
                let sender = sender.clone();
                let positions = &positions;
                let scan_options = scan_options.clone();
                scope.spawn(move |_| {
                    log::trace!("render region index worker {worker_index} started");
                    loop {
                        if scan_options
                            .cancel
                            .as_ref()
                            .is_some_and(CancelFlag::is_cancelled)
                        {
                            return;
                        }
                        let index = next_position.fetch_add(1, Ordering::Relaxed);
                        let Some(pos) = positions.get(index).copied() else {
                            log::trace!("render region index worker {worker_index} finished");
                            return;
                        };
                        let result = self
                            .has_render_chunk_records(pos, &scan_options)
                            .map(|is_renderable| is_renderable.then_some(pos));
                        if sender.send(result).is_err() {
                            return;
                        }
                    }
                });
            }
            drop(sender);

            let mut render_positions = Vec::new();
            for result in receiver {
                if let Some(pos) = result? {
                    render_positions.push(pos);
                }
            }
            render_positions.sort();
            log::debug!(
                "render chunk region index complete (dimension={:?}, candidates={}, positions={}, workers={}, queue_depth={}, elapsed_ms={})",
                region.dimension,
                positions.len(),
                render_positions.len(),
                worker_count,
                queue_depth,
                started.elapsed().as_millis()
            );
            Ok(render_positions)
        })
    }

    /// Discover chunk bounds.
    pub fn discover_chunk_bounds(
        &self,
        dimension: crate::Dimension,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkBounds>> {
        let mut bounds: Option<ChunkBounds> = None;
        let mut seen_positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos.dimension == dimension && seen_positions.insert(chunk_key.pos)
                    {
                        match &mut bounds {
                            Some(bounds) => bounds.include(chunk_key.pos),
                            None => bounds = Some(ChunkBounds::from_first(chunk_key.pos)),
                        }
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(bounds)
    }

    /// Nearest loaded chunk to spawn.
    pub fn nearest_loaded_chunk_to_spawn(
        &self,
        dimension: crate::Dimension,
        spawn_block_x: i32,
        spawn_block_z: i32,
        options: WorldScanOptions,
    ) -> Result<Option<ChunkPos>> {
        let spawn_chunk = BlockPos {
            x: spawn_block_x,
            y: 0,
            z: spawn_block_z,
        }
        .to_chunk_pos(dimension);
        let mut best = None::<(i64, ChunkPos)>;
        let mut seen_positions = BTreeSet::new();
        let mut entries_seen = 0usize;
        self.storage()
            .for_each_key(to_storage_read_options(&options), &mut |key| {
                check_cancelled(&options)?;
                entries_seen = entries_seen.saturating_add(1);
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos.dimension == dimension && seen_positions.insert(chunk_key.pos)
                    {
                        let dx = i64::from(chunk_key.pos.x) - i64::from(spawn_chunk.x);
                        let dz = i64::from(chunk_key.pos.z) - i64::from(spawn_chunk.z);
                        let distance = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
                        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                            best = Some((distance, chunk_key.pos));
                        }
                    }
                }
                if entries_seen.is_multiple_of(8192) {
                    emit_progress(&options, entries_seen);
                }
                Ok(StorageVisitorControl::Continue)
            })?;
        Ok(best.map(|(_, pos)| pos))
    }

    /// Get player.
    pub fn player(&self, id: &PlayerId) -> Result<Option<PlayerData>> {
        let Some(key) = id.storage_key() else {
            if *id == PlayerId::LegacyLevelDat {
                let document = self.read_level_dat()?;
                return Ok(Some(PlayerData::from_nbt(id.clone(), document.root)?));
            }
            return Ok(None);
        };
        self.storage()
            .get(key.as_ref())?
            .map(|bytes| PlayerData::from_raw(id.clone(), bytes))
            .transpose()
    }

    /// Put player.
    pub fn save_player(&self, player: &PlayerData) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = player.id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no LevelDB key".to_string(),
            ));
        };
        self.storage().put(key.as_ref(), &player.raw)
    }

    /// Deletes an exact LevelDB-backed player record.
    ///
    /// Legacy level.dat pseudo players are intentionally rejected because they are not
    /// independent LevelDB records.
    pub fn delete_player(&self, id: &PlayerId) -> Result<()> {
        self.ensure_writable()?;
        let Some(key) = id.storage_key() else {
            return Err(BedrockWorldError::Validation(
                "player id has no deletable LevelDB key".to_string(),
            ));
        };
        let mut transaction = self.transaction();
        transaction.delete_raw_key(Bytes::copy_from_slice(key.as_ref()));
        transaction.commit()
    }

    /// Get chunk.
    pub fn chunk(&self, pos: ChunkPos) -> Result<LevelChunk> {
        let mut records = Vec::new();
        let prefix = chunk_record_prefix(pos);
        self.storage().for_each_prefix(
            &prefix,
            StorageReadOptions::default(),
            &mut |raw_key, value| {
                let key = ChunkKey::decode(raw_key).map_err(|error| {
                    BedrockWorldError::CorruptWorld(format!(
                        "invalid chunk record key under prefix for {pos:?}: {error}"
                    ))
                })?;
                if key.pos == pos {
                    records.push(ChunkRecord {
                        key,
                        value: value.clone(),
                    });
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        let version = records
            .iter()
            .find(|record| record.key.tag == ChunkRecordTag::Version)
            .and_then(|record| record.value.first().copied());
        Ok(LevelChunk {
            pos,
            version,
            records,
        })
    }

    /// Scans the world storage using the selected retention options.
    pub fn scan(&self, options: ScanOptions) -> Result<WorldScan> {
        let level_dat = self.read_level_dat()?;
        scan_storage(level_dat, self.storage(), options)
    }

    /// Scans the known records for one chunk using the selected retention options.
    pub fn scan_chunk(
        &self,
        pos: ChunkPos,
        options: ScanOptions,
    ) -> Result<crate::scan::Chunk> {
        let chunk = self.chunk(pos)?;
        Ok(crate::scan::Chunk::new(
            pos,
            &chunk.records,
            options,
        ))
    }

    /// Reads one persisted SubChunk using the requested decode mode.
    pub fn subchunk(
        &self,
        pos: ChunkPos,
        y: i8,
        mode: SubChunkDecodeMode,
    ) -> Result<Option<crate::SubChunk>> {
        let key = ChunkKey::subchunk(pos, y);
        self.storage()
            .get(&key.encode())?
            .map(|value| SubChunk::read(y, value, mode))
            .transpose()
    }

    /// Get biome storage.
    pub fn biome_storage(
        &self,
        pos: ChunkPos,
        y: i32,
    ) -> Result<Option<BiomeStorage>> {
        let Some(biome_data) = self.biome_data(pos)? else {
            return Ok(None);
        };
        for storage in biome_data.storages {
            if biome_storage_contains_y(&storage, y) {
                return Ok(Some(storage));
            }
        }
        Ok(None)
    }

    /// Get biome storages.
    pub fn biome_storages(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<Vec<BiomeStorage>>> {
        Ok(self
            .biome_data(pos)?
            .map(|biome_data| biome_data.storages))
    }

    fn has_render_chunk_records(
        &self,
        pos: ChunkPos,
        options: &WorldScanOptions,
    ) -> Result<bool> {
        let prefix = chunk_record_prefix(pos);
        let mut found = false;
        self.storage().for_each_prefix_key(
            &prefix,
            to_storage_read_options(options),
            &mut |key| {
                check_cancelled(options)?;
                if let BedrockDbKey::Chunk(chunk_key) = BedrockDbKey::decode(key) {
                    if chunk_key.pos == pos && chunk_key.tag.is_render_chunk_record() {
                        found = true;
                        return Ok(StorageVisitorControl::Stop);
                    }
                }
                Ok(StorageVisitorControl::Continue)
            },
        )?;
        Ok(found)
    }

    /// Get height at.
    pub fn height(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<i16>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .height_map(pos)?
            .and_then(|heights| heights[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get height map.
    pub fn height_map(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<i16>; 16]; 16]>> {
        if let Some(biome_data) = self
            .biome_data(pos)
            .map_err(|error| BedrockWorldError::CorruptWorld(format!("height data: {error}")))?
        {
            return Ok(Some(render_height_map_from_biome_data(pos, &biome_data)));
        }
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        if let Some(value) = self.storage().get(&key)? {
            let terrain = LegacyTerrain::parse(value)?;
            return Ok(Some(render_height_map_from_legacy_terrain(&terrain)));
        }
        Ok(None)
    }

    /// Get legacy biome colors.
    pub fn legacy_biome_colors(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<u32>; 16]; 16]>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(None);
        };
        let terrain = LegacyTerrain::parse(value)?;
        Ok(Some(render_biome_colors_from_legacy_terrain(&terrain)))
    }

    /// Get legacy biome samples.
    pub fn legacy_biome_samples(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<[[Option<LegacyBiomeSample>; 16]; 16]>> {
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let Some(value) = self.storage().get(&key)? else {
            return Ok(None);
        };
        let terrain = LegacyTerrain::parse(value)?;
        Ok(Some(render_biomes_from_legacy_terrain(&terrain)))
    }

    /// Get legacy biome color.
    pub fn legacy_biome_color(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<u32>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .legacy_biome_colors(pos)?
            .and_then(|colors| colors[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get legacy biome sample.
    pub fn legacy_biome_sample(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<LegacyBiomeSample>> {
        validate_local_column(local_x, local_z)?;
        Ok(self
            .legacy_biome_samples(pos)?
            .and_then(|samples| samples[usize::from(local_z)][usize::from(local_x)]))
    }

    /// Get biome id.
    pub fn biome_id(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
        y: i32,
    ) -> Result<Option<u32>> {
        validate_local_column(local_x, local_z)?;
        let Some(storage) = self.biome_storage(pos, y)? else {
            return Ok(None);
        };
        Ok(biome_id_from_storage(&storage, local_x, local_z, y))
    }

    /// Get surface column.
    pub fn surface_column(
        &self,
        pos: ChunkPos,
        local_x: u8,
        local_z: u8,
        options: SurfaceColumnOptions,
    ) -> Result<Option<SurfaceColumn>> {
        validate_local_column(local_x, local_z)?;
        let chunk = self.query_chunk_data(
            pos,
            ChunkLoadOptions::for_data_request(
                ChunkDataRequest::new()
                    .surface_columns(ExactSurfaceSubchunkPolicy::Full)
                    .biome(BiomeDataRequirement::SurfaceColumns),
            ),
        )?;
        let Some(sample) = chunk.column_sample_at(local_x, local_z) else {
            return Ok(None);
        };
        let biome_id = sample.biome.map(|biome| match biome {
            TerrainColumnBiome::Id(id) => id,
            TerrainColumnBiome::Legacy(sample) => u32::from(sample.biome_id),
        });
        let (water_depth, under_water_block_name) = if options.transparent_water {
            sample.water.as_ref().map_or((0, None), |water| {
                (
                    water.depth,
                    water
                        .underwater_block_state
                        .as_ref()
                        .map(|state| state.name.clone()),
                )
            })
        } else {
            (0, None)
        };
        Ok(Some(SurfaceColumn {
            y: i32::from(sample.surface_y),
            block_name: sample.surface_block_state.name.clone(),
            biome_id,
            water_depth,
            under_water_block_name,
            is_fallback: sample.source == TerrainSampleSource::LegacyFallback,
        }))
    }

    /// Load render chunk.
    pub fn query_chunk_data(
        &self,
        pos: ChunkPos,
        options: ChunkLoadOptions,
    ) -> Result<ChunkData> {
        let (mut chunks, _) = self.query_chunk_data_with_stats([pos], options)?;
        chunks.pop().ok_or_else(|| {
            BedrockWorldError::CorruptWorld("exact render load returned no chunk".to_string())
        })
    }

    /// Loads only canonical terrain column samples for one chunk.
    ///
    /// The request remains configurable for subchunk, biome, block-entity, storage,
    /// cancellation, and threading policy, but this entry point always retains packed
    /// palette indices rather than materializing full 3D index arrays.
    pub fn load_surface_columns(
        &self,
        pos: ChunkPos,
        mut options: ChunkLoadOptions,
    ) -> Result<Option<TerrainColumnSamples>> {
        let mut request = options.data_request.clone();
        if !request
            .subchunks
            .iter()
            .any(|requirement| matches!(requirement, SubchunkDataRequirement::SurfaceColumns(_)))
        {
            return Err(BedrockWorldError::Validation(
                "surface-column loads require a SurfaceColumns data requirement".to_string(),
            ));
        }
        request.subchunks.retain(|requirement| {
            !matches!(
                requirement,
                SubchunkDataRequirement::Layer(_)
                    | SubchunkDataRequirement::CaveSlice(_)
                    | SubchunkDataRequirement::Full3dIndices
            )
        });
        options.data_request = request;
        Ok(self.query_chunk_data(pos, options)?.column_samples)
    }

    /// Load render chunks.
    pub fn query_chunk_data_many(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<Vec<ChunkData>> {
        Ok(self
            .query_chunk_data_with_stats(positions, options)?
            .0)
    }

    /// Load render chunks with stats.
    pub fn query_chunk_data_with_stats(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        options: ChunkLoadOptions,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        let started = Instant::now();
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            log::debug!("loading render chunks skipped (chunks=0)");
            return Ok((Vec::new(), ChunkLoadStats::default()));
        }
        let mut positions = positions;
        sort_render_chunk_positions(&mut positions, options.priority);
        let worker_count = options.threading.resolve_checked(positions.len())?;
        log::debug!(
            "loading render chunks (chunks={}, workers={}, data_request={:?}, queue_depth={}, priority={:?})",
            positions.len(),
            worker_count,
            options.data_request,
            options
                .pipeline
                .resolve_queue_depth(worker_count, positions.len()),
            options.priority
        );
        self.load_render_chunks_exact_batch_sorted(
            positions,
            options,
            worker_count,
            started,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn load_render_chunks_exact_batch_sorted(
        &self,
        positions: Vec<ChunkPos>,
        options: ChunkLoadOptions,
        worker_count: usize,
        started: Instant,
    ) -> Result<(Vec<ChunkData>, ChunkLoadStats)> {
        check_render_load_cancelled(&options)?;
        let mut raw_chunks = positions
            .iter()
            .copied()
            .map(|pos| RawChunkData {
                pos,
                biome_record: None,
                subchunks: BTreeMap::new(),
                block_entities: None,
                legacy_terrain: None,
            })
            .collect::<Vec<_>>();

        let mut keys = Vec::new();
        let mut requests = Vec::new();
        for (chunk_index, pos) in positions.iter().copied().enumerate() {
            if request_needs_legacy_terrain(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::LegacyTerrain,
                );
            }
            if request_needs_biome_record(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data3D,
                );
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data2D,
                );
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::Data2DLegacy,
                );
            }
            if !request_uses_hint_surface_subchunks(&options) {
                for y in planned_render_subchunk_ys(pos, &options, None)? {
                    push_render_record_request(
                        &mut keys,
                        &mut requests,
                        chunk_index,
                        pos,
                        RenderRecordKind::Subchunk(y),
                    );
                }
            }
            if request_loads_block_entities(&options) {
                push_render_record_request(
                    &mut keys,
                    &mut requests,
                    chunk_index,
                    pos,
                    RenderRecordKind::BlockEntity,
                );
            }
        }

        let mut keys_requested = keys.len();
        let mut exact_get_batches = 0usize;
        let mut db_read_ms = 0u128;
        let storage_read_options = to_render_storage_read_options(&options);
        let db_started = Instant::now();
        let values = self
            .storage()
            .get_many_ordered(&keys, storage_read_options.clone())?;
        db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
        exact_get_batches = exact_get_batches.saturating_add(usize::from(!keys.is_empty()));
        let mut keys_found = apply_render_record_values(&mut raw_chunks, &requests, values)?;

        if request_needs_legacy_terrain_fallback(&options) {
            let mut fallback_keys = Vec::new();
            let mut fallback_requests = Vec::new();
            for (chunk_index, raw) in raw_chunks.iter().enumerate() {
                if raw.subchunks.is_empty() && raw.legacy_terrain.is_none() {
                    push_render_record_request(
                        &mut fallback_keys,
                        &mut fallback_requests,
                        chunk_index,
                        raw.pos,
                        RenderRecordKind::LegacyTerrain,
                    );
                }
            }
            if !fallback_keys.is_empty() {
                let db_started = Instant::now();
                let values = self
                    .storage()
                    .get_many_ordered(&fallback_keys, storage_read_options.clone())?;
                db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
                exact_get_batches = exact_get_batches.saturating_add(1);
                keys_requested = keys_requested.saturating_add(fallback_keys.len());
                keys_found = keys_found.saturating_add(apply_render_record_values(
                    &mut raw_chunks,
                    &fallback_requests,
                    values,
                )?);
            }
        }

        if request_uses_hint_surface_subchunks(&options) {
            let mut needed_keys = Vec::new();
            let mut needed_requests = Vec::new();
            for (chunk_index, raw) in raw_chunks.iter().enumerate() {
                let biome_data = parse_render_biome_record(raw.biome_record.as_ref())?;
                let height_map = if let Some(biome_data) = biome_data.as_ref() {
                    Some(render_height_map_from_biome_data(raw.pos, biome_data))
                } else {
                    legacy_height_map_from_raw(raw.legacy_terrain.as_ref())?
                };
                for y in planned_render_subchunk_ys(raw.pos, &options, height_map.as_ref())? {
                    if raw.subchunks.contains_key(&y) {
                        continue;
                    }
                    push_render_record_request(
                        &mut needed_keys,
                        &mut needed_requests,
                        chunk_index,
                        raw.pos,
                        RenderRecordKind::Subchunk(y),
                    );
                }
            }
            if !needed_keys.is_empty() {
                let db_started = Instant::now();
                let values = self
                    .storage()
                    .get_many_ordered(&needed_keys, storage_read_options.clone())?;
                db_read_ms = db_read_ms.saturating_add(db_started.elapsed().as_millis());
                exact_get_batches = exact_get_batches.saturating_add(1);
                keys_requested = keys_requested.saturating_add(needed_keys.len());
                keys_found = keys_found.saturating_add(apply_render_record_values(
                    &mut raw_chunks,
                    &needed_requests,
                    values,
                )?);
            }
        }

        check_render_load_cancelled(&options)?;
        let decode_started = Instant::now();
        let (mut chunks, decode_timing) = if worker_count == 1 {
            let mut chunks = Vec::with_capacity(raw_chunks.len());
            let mut timing = ChunkDecodeTiming::default();
            for raw in raw_chunks {
                check_render_load_cancelled(&options)?;
                let (chunk, chunk_timing) = render_chunk_from_raw(raw, &options)?;
                timing.add(chunk_timing);
                chunks.push(chunk);
                emit_render_load_progress(&options, chunks.len());
            }
            (chunks, timing)
        } else {
            let executor = world_executor(worker_count)?;
            let decoded = executor.pool.install(|| {
                raw_chunks
                    .into_par_iter()
                    .map(|raw| {
                        check_render_load_cancelled(&options)?;
                        render_chunk_from_raw(raw, &options)
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
            let mut chunks = Vec::with_capacity(decoded.len());
            let mut timing = ChunkDecodeTiming::default();
            for (chunk, chunk_timing) in decoded {
                timing.add(chunk_timing);
                chunks.push(chunk);
            }
            (chunks, timing)
        };
        let full_reload_ms =
            self.reload_incomplete_needed_exact_surface_chunks(&mut chunks, &options)?;
        let decode_ms = decode_started.elapsed().as_millis();
        let mut stats = render_load_stats(&chunks, worker_count, 0, started.elapsed().as_millis());
        stats.keys_requested = keys_requested;
        stats.keys_found = keys_found;
        stats.exact_get_batches = exact_get_batches;
        stats.prefix_scans = 0;
        stats.decode_ms = decode_ms;
        stats.db_read_ms = db_read_ms;
        stats.biome_parse_us = decode_timing.biome_parse_us;
        stats.subchunk_parse_us = decode_timing.subchunk_parse_us;
        stats.surface_scan_us = decode_timing.surface_scan_us;
        stats.block_entity_parse_us = decode_timing.block_entity_parse_us;
        stats.biome_parse_ms = stats.biome_parse_us / 1_000;
        stats.subchunk_parse_ms = stats.subchunk_parse_us / 1_000;
        stats.surface_scan_ms = stats.surface_scan_us / 1_000;
        stats.block_entity_parse_ms = stats.block_entity_parse_us / 1_000;
        stats.full_reload_ms = full_reload_ms;
        stats.detected_format = self.format;
        stats.legacy_pocket_chunks = if self.format == WorldFormat::PocketChunksDat {
            stats.legacy_terrain_records
        } else {
            0
        };
        log_render_load_complete(&stats);
        Ok((chunks, stats))
    }

    fn reload_incomplete_needed_exact_surface_chunks(
        &self,
        chunks: &mut [ChunkData],
        options: &ChunkLoadOptions,
    ) -> Result<u128> {
        if !request_uses_hint_surface_subchunks(options) {
            return Ok(0);
        }

        let mut full_options = options.clone();
        exact_surface_full_request(&mut full_options);
        let mut reload_indexes = Vec::new();
        let mut reload_positions = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            if needed_exact_surface_chunk_requires_full_reload(chunk)? {
                reload_indexes.push(index);
                reload_positions.push(chunk.pos);
            }
        }
        if reload_positions.is_empty() {
            return Ok(0);
        }
        check_render_load_cancelled(options)?;
        let started = Instant::now();
        let worker_count = options.threading.resolve_checked(reload_positions.len())?;
        full_options.threading = if worker_count <= 1 {
            WorldThreadingOptions::Single
        } else {
            WorldThreadingOptions::Fixed(worker_count)
        };
        let (reloaded, stats) =
            self.query_chunk_data_with_stats(reload_positions, full_options)?;
        for (chunk_index, reloaded_chunk) in reload_indexes.into_iter().zip(reloaded) {
            if let Some(chunk) = chunks.get_mut(chunk_index) {
                *chunk = reloaded_chunk;
            }
        }
        let elapsed = started.elapsed().as_millis().max(stats.load_ms);
        log::debug!(
            "hint surface full reload complete (chunks={}, workers={}, load_ms={}, db_read_ms={}, decode_ms={})",
            stats.requested_chunks,
            stats.worker_threads,
            stats.load_ms,
            stats.db_read_ms,
            stats.decode_ms
        );
        Ok(elapsed)
    }

    /// Load render region.
    pub fn query_chunk_region(
        &self,
        region: Region,
        options: RegionLoadOptions,
    ) -> Result<RegionLoad> {
        if region.min_chunk_x > region.max_chunk_x || region.min_chunk_z > region.max_chunk_z {
            return Err(BedrockWorldError::Validation(format!(
                "invalid render region: min=({}, {}) max=({}, {})",
                region.min_chunk_x, region.min_chunk_z, region.max_chunk_x, region.max_chunk_z
            )));
        }
        let chunk_count_x = i64::from(region.max_chunk_x) - i64::from(region.min_chunk_x) + 1;
        let chunk_count_z = i64::from(region.max_chunk_z) - i64::from(region.min_chunk_z) + 1;
        let capacity = usize::try_from(chunk_count_x.saturating_mul(chunk_count_z))
            .map_err(|_| BedrockWorldError::Validation("render region is too large".to_string()))?;
        let mut positions = Vec::with_capacity(capacity);
        for z in region.min_chunk_z..=region.max_chunk_z {
            for x in region.min_chunk_x..=region.max_chunk_x {
                positions.push(ChunkPos {
                    x,
                    z,
                    dimension: region.dimension,
                });
            }
        }
        let (chunks, stats) =
            self.query_chunk_data_with_stats(positions, options.into())?;
        Ok(RegionLoad {
            region,
            chunks,
            stats,
        })
    }

    /// Get block state at.
    pub fn block_state(
        &self,
        dimension: crate::Dimension,
        block_pos: BlockPos,
    ) -> Result<Option<BlockState>> {
        let chunk_pos = block_pos.to_chunk_pos(dimension);
        let (_, block_y, _) = block_pos.in_chunk_offset();
        let subchunk_y = block_y_to_subchunk_y(block_y)?;
        let subchunk = self.subchunk(chunk_pos, subchunk_y, SubChunkDecodeMode::FullIndices)?;
        let (local_x, _, local_z) = block_pos.in_chunk_offset();
        let local_y = u8::try_from(block_y - i32::from(subchunk_y) * 16).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={block_y} is outside subchunk bounds"))
        })?;
        if let Some(subchunk) = subchunk {
            if let Some(state) = subchunk.block_state_at(local_x, local_y, local_z) {
                return Ok(Some(state.clone()));
            }
            if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z) {
                return Ok(Some(
                    crate::chunk::legacy_terrain::legacy_numeric_block_state(
                        id,
                        subchunk.legacy_block_data_at(local_x, local_y, local_z),
                    ),
                ));
            }
        }
        self.legacy_terrain_block_state_at(chunk_pos, local_x, block_y, local_z)
    }

    /// Decodes the subchunk layer containing the requested world Y coordinate.
    pub fn subchunk_layer(
        &self,
        pos: ChunkPos,
        y: i32,
        mode: SubChunkDecodeMode,
    ) -> Result<Option<SubChunk>> {
        let subchunk_y = block_y_to_subchunk_y(y)?;
        self.subchunk(pos, subchunk_y, mode)
    }
}

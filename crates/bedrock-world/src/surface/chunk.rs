//! Internal render loading, biome projection, and terrain sampling helpers.

//! Chunk render-load planning and decoded terrain assembly.

use super::*;
use crate::surface::{is_air_block_name, is_water_block_name};
use crate::chunk::{BlockStatePaletteEntry, LegacyTerrain};

pub(crate) fn check_cancelled(options: &WorldScanOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelFlag::is_cancelled)
    {
        return Err(BedrockWorldError::Cancelled {
            operation: "world scan",
        });
    }
    Ok(())
}

pub(crate) fn emit_progress(options: &WorldScanOptions, entries_seen: usize) {
    if let Some(progress) = &options.progress {
        progress.emit(WorldScanProgress { entries_seen });
    }
}

pub(crate) fn check_render_load_cancelled(options: &ChunkLoadOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelFlag::is_cancelled)
    {
        return Err(BedrockWorldError::Cancelled {
            operation: "render chunk load",
        });
    }
    Ok(())
}

pub(crate) fn emit_render_load_progress(options: &ChunkLoadOptions, completed_chunks: usize) {
    if completed_chunks.is_multiple_of(options.pipeline.resolve_progress_interval()) {
        if let Some(progress) = &options.progress {
            progress.emit(WorldScanProgress {
                entries_seen: completed_chunks,
            });
        }
    }
}

pub(crate) fn sort_render_chunk_positions(positions: &mut [ChunkPos], priority: ChunkLoadPriority) {
    match priority {
        ChunkLoadPriority::RowMajor => positions.sort(),
        ChunkLoadPriority::DistanceFrom { chunk_x, chunk_z } => positions.sort_by_key(|pos| {
            let dx = i64::from(pos.x) - i64::from(chunk_x);
            let dz = i64::from(pos.z) - i64::from(chunk_z);
            (
                dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
                pos.z,
                pos.x,
                pos.dimension,
            )
        }),
    }
}

pub(crate) fn push_render_record_request(
    keys: &mut Vec<Bytes>,
    requests: &mut Vec<RenderRecordRequest>,
    chunk_index: usize,
    pos: ChunkPos,
    kind: RenderRecordKind,
) {
    let key = match kind {
        RenderRecordKind::LegacyTerrain => {
            ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode()
        }
        RenderRecordKind::Data3D => ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
        RenderRecordKind::Data2D => ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
        RenderRecordKind::Data2DLegacy => ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
        RenderRecordKind::Subchunk(y) => ChunkKey::subchunk(pos, y).encode(),
        RenderRecordKind::BlockEntity => ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
    };
    keys.push(key);
    requests.push(RenderRecordRequest { chunk_index, kind });
}

pub(crate) fn apply_render_record_values(
    chunks: &mut [RawChunkData],
    requests: &[RenderRecordRequest],
    values: Vec<Option<Bytes>>,
) -> Result<usize> {
    let mut found = 0usize;
    for (request, value) in requests.iter().copied().zip(values) {
        let Some(value) = value else {
            continue;
        };
        found = found.saturating_add(1);
        let Some(chunk) = chunks.get_mut(request.chunk_index) else {
            continue;
        };
        match request.kind {
            RenderRecordKind::LegacyTerrain => {
                chunk.legacy_terrain = Some(value);
            }
            RenderRecordKind::Data3D => {
                if chunk.biome_record.is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "chunk {:?} contains mixed biome records including Data3D",
                        chunk.pos
                    )));
                }
                chunk.biome_record = Some((ChunkRecordTag::Data3D, value));
            }
            RenderRecordKind::Data2D | RenderRecordKind::Data2DLegacy => {
                if chunk.biome_record.is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "chunk {:?} contains mixed biome records including {:?}",
                        chunk.pos, request.kind
                    )));
                }
                chunk.biome_record = Some((request.kind.biome_tag(), value));
            }
            RenderRecordKind::Subchunk(y) => {
                chunk.subchunks.insert(y, value);
            }
            RenderRecordKind::BlockEntity => {
                chunk.block_entities = Some(value);
            }
        }
    }
    Ok(found)
}

pub(crate) fn planned_render_subchunk_ys(
    pos: ChunkPos,
    options: &ChunkLoadOptions,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
) -> Result<BTreeSet<i8>> {
    let mut subchunk_ys = BTreeSet::new();
    let request = options.data_request.clone();
    let (min_y, max_y) = pos.subchunk_index_range(crate::ChunkVersion::New);
    for requirement in request.subchunks {
        match requirement {
            SubchunkDataRequirement::SurfaceColumns(subchunks) => match subchunks {
                ExactSurfaceSubchunkPolicy::Full => {
                    for y in min_y..=max_y {
                        subchunk_ys.insert(y);
                    }
                }
                ExactSurfaceSubchunkPolicy::HintThenVerify => {
                    if let Some(height_map) = height_map {
                        insert_needed_surface_subchunks(
                            &mut subchunk_ys,
                            Some(height_map),
                            min_y,
                            max_y,
                        );
                    } else {
                        for y in min_y..=max_y {
                            subchunk_ys.insert(y);
                        }
                    }
                }
            },
            SubchunkDataRequirement::Layer(y) | SubchunkDataRequirement::CaveSlice(y) => {
                subchunk_ys.insert(block_y_to_subchunk_y(y)?);
            }
            SubchunkDataRequirement::Full3dIndices => {
                for y in min_y..=max_y {
                    subchunk_ys.insert(y);
                }
            }
        }
    }
    Ok(subchunk_ys)
}

pub(crate) fn request_needs_biome_record(options: &ChunkLoadOptions) -> bool {
    let request = &options.data_request;
    request.height_map || !matches!(request.biome, BiomeDataRequirement::None)
}

pub(crate) fn request_needs_legacy_terrain(options: &ChunkLoadOptions) -> bool {
    options.data_request.height_map
        || request_builds_column_samples(options)
        || !matches!(options.data_request.biome, BiomeDataRequirement::None)
}

pub(crate) fn request_needs_legacy_terrain_fallback(options: &ChunkLoadOptions) -> bool {
    !request_needs_legacy_terrain(options) && !options.data_request.subchunks.is_empty()
}

pub(crate) fn request_loads_block_entities(options: &ChunkLoadOptions) -> bool {
    options.data_request.block_entities
}

pub(crate) fn request_builds_column_samples(options: &ChunkLoadOptions) -> bool {
    options
        .data_request
        .subchunks
        .iter()
        .any(|requirement| matches!(requirement, SubchunkDataRequirement::SurfaceColumns(_)))
}

pub(crate) fn request_uses_hint_surface_subchunks(options: &ChunkLoadOptions) -> bool {
    options.data_request.subchunks.iter().any(|requirement| {
        matches!(
            requirement,
            SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::HintThenVerify)
        )
    })
}

pub(crate) fn exact_surface_full_request(options: &mut ChunkLoadOptions) {
    let mut request = options.data_request.clone();
    for requirement in &mut request.subchunks {
        if matches!(
            requirement,
            SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::HintThenVerify)
        ) {
            *requirement =
                SubchunkDataRequirement::SurfaceColumns(ExactSurfaceSubchunkPolicy::Full);
        }
    }
    options.data_request = request;
}

pub(crate) fn insert_render_biome_storages(
    render_biomes: &mut BTreeMap<i32, BiomeStorage>,
    biome_data: Option<BiomeData>,
    request: &ChunkDataRequest,
) {
    let Some(biome_data) = biome_data else {
        return;
    };
    match request.biome {
        BiomeDataRequirement::SurfaceColumns | BiomeDataRequirement::All => {
            for storage in biome_data.storages {
                let key = storage.y.unwrap_or(i32::MIN);
                render_biomes.insert(key, storage);
            }
        }
        BiomeDataRequirement::Layer(y) => {
            let mut fallback = None;
            for storage in biome_data.storages {
                if biome_storage_contains_y(&storage, y) {
                    render_biomes.insert(biome_storage_bucket_y(y), storage);
                    return;
                }
                fallback.get_or_insert(storage);
            }
            if let Some(storage) = fallback {
                render_biomes.insert(biome_storage_bucket_y(y), storage);
            }
        }
        BiomeDataRequirement::None => {}
    }
}

pub(crate) fn parse_render_biome_record(
    record: Option<&(ChunkRecordTag, Bytes)>,
) -> Result<Option<BiomeData>> {
    let Some((tag, value)) = record else {
        return Ok(None);
    };
    let data = match tag {
        ChunkRecordTag::Data3D => parse_data3d(value),
        ChunkRecordTag::Data2D => parse_legacy_data2d(value),
        ChunkRecordTag::Data2DLegacy => parse_data2d_legacy(value),
        _ => unreachable!("render biome record contains only biome tags"),
    }
    .map_err(|error| BedrockWorldError::CorruptWorld(format!("biome data: {error}")))?;
    Ok(Some(data))
}

pub(crate) fn render_height_map_from_biome_data(
    pos: ChunkPos,
    biome_data: &BiomeData,
) -> [[Option<i16>; 16]; 16] {
    let mut heights = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let index = height_map_index(local_x, local_z);
            heights[usize::from(local_z)][usize::from(local_x)] = biome_data
                .height_map
                .get(index)
                .and_then(|height| normalize_biome_height(pos, biome_data.version, *height));
        }
    }
    heights
}

pub(crate) fn normalize_biome_height(
    pos: ChunkPos,
    version: crate::ChunkVersion,
    stored_height: i16,
) -> Option<i16> {
    let (min_y, _) = pos.y_range(version);
    i16::try_from(i32::from(stored_height) + min_y).ok()
}

pub(crate) fn legacy_height_map_from_raw(
    raw_legacy_terrain: Option<&Bytes>,
) -> Result<Option<[[Option<i16>; 16]; 16]>> {
    let Some(raw_legacy_terrain) = raw_legacy_terrain else {
        return Ok(None);
    };
    let terrain = LegacyTerrain::parse(raw_legacy_terrain.clone())?;
    Ok(Some(render_height_map_from_legacy_terrain(&terrain)))
}

pub(crate) fn render_height_map_from_legacy_terrain(
    terrain: &LegacyTerrain,
) -> [[Option<i16>; 16]; 16] {
    let mut heights = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            heights[usize::from(local_z)][usize::from(local_x)] =
                terrain.height_at(local_x, local_z).map(i16::from);
        }
    }
    heights
}

pub(crate) fn render_biomes_from_legacy_terrain(
    terrain: &LegacyTerrain,
) -> [[Option<LegacyBiomeSample>; 16]; 16] {
    let mut samples = [[None; 16]; 16];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            samples[usize::from(local_z)][usize::from(local_x)] =
                terrain.biome_sample_at(local_x, local_z);
        }
    }
    samples
}

pub(crate) fn render_biome_colors_from_legacy_terrain(
    terrain: &LegacyTerrain,
) -> [[Option<u32>; 16]; 16] {
    let mut colors = [[None; 16]; 16];
    let samples = render_biomes_from_legacy_terrain(terrain);
    for local_z in 0..16 {
        for local_x in 0..16 {
            colors[local_z][local_x] = samples[local_z][local_x].map(LegacyBiomeSample::rgb_u32);
        }
    }
    colors
}

struct SurfaceProjection<'a> {
    min_subchunk_y: i8,
    subchunks: Vec<Option<&'a SubChunk>>,
    storages: Vec<Option<Vec<SurfaceStorageProjection<'a>>>>,
}

struct SurfaceStorageProjection<'a> {
    states: &'a [BlockState],
    roles: Vec<TerrainSurfaceRole>,
    indices: Cow<'a, [u16]>,
}

struct ProjectedSurfaceEntry<'a> {
    entry: BlockStatePaletteEntry<'a>,
    role: TerrainSurfaceRole,
}

struct ProjectedSurfaceStatesAt<'projection, 'chunk> {
    storages: std::iter::Rev<
        std::iter::Enumerate<std::slice::Iter<'projection, SurfaceStorageProjection<'chunk>>>,
    >,
    block_index: usize,
}

impl<'chunk> Iterator for ProjectedSurfaceStatesAt<'_, 'chunk> {
    type Item = ProjectedSurfaceEntry<'chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        for (storage_index, storage) in &mut self.storages {
            let Some(palette_index) = storage.indices.get(self.block_index).copied() else {
                continue;
            };
            let palette_index = usize::from(palette_index);
            let Some(state) = storage.states.get(palette_index) else {
                continue;
            };
            let Some(role) = storage.roles.get(palette_index).copied() else {
                continue;
            };
            if role != TerrainSurfaceRole::Air {
                return Some(ProjectedSurfaceEntry {
                    entry: BlockStatePaletteEntry {
                        state,
                        storage_index,
                    },
                    role,
                });
            }
        }
        None
    }
}

impl<'a> SurfaceProjection<'a> {
    fn new(subchunks: &'a BTreeMap<i8, SubChunk>) -> Option<Self> {
        let (&min_subchunk_y, _) = subchunks.first_key_value()?;
        let (&max_subchunk_y, _) = subchunks.last_key_value()?;
        let span = i16::from(max_subchunk_y)
            .saturating_sub(i16::from(min_subchunk_y))
            .saturating_add(1);
        let len = usize::try_from(span).ok()?;
        let mut projected = vec![None; len];
        let mut storage_projection = (0..len).map(|_| None).collect::<Vec<_>>();
        for (&y, subchunk) in subchunks {
            let index = i16::from(y).saturating_sub(i16::from(min_subchunk_y));
            if let Ok(index) = usize::try_from(index) {
                if let Some(slot) = projected.get_mut(index) {
                    *slot = Some(subchunk);
                    storage_projection[index] = match &subchunk.format {
                        crate::chunk::SubChunkFormat::Paletted { storages, .. } => storages
                            .iter()
                            .map(|storage| {
                                Some(SurfaceStorageProjection {
                                    states: &storage.states,
                                    roles: storage
                                        .states
                                        .iter()
                                        .map(|state| terrain_surface_role(&state.name))
                                        .collect(),
                                    indices: storage.surface_indices()?,
                                })
                            })
                            .collect::<Option<Vec<_>>>(),
                        _ => None,
                    };
                }
            }
        }
        Some(Self {
            min_subchunk_y,
            subchunks: projected,
            storages: storage_projection,
        })
    }

    fn get(&self, y: i8) -> Option<&'a SubChunk> {
        let index = i16::from(y).checked_sub(i16::from(self.min_subchunk_y))?;
        self.subchunks
            .get(usize::try_from(index).ok()?)?
            .as_ref()
            .copied()
    }

    fn surface_states_at(
        &self,
        y: i8,
        local_x: u8,
        local_y: u8,
        local_z: u8,
    ) -> Option<ProjectedSurfaceStatesAt<'_, 'a>> {
        let index =
            usize::try_from(i16::from(y).checked_sub(i16::from(self.min_subchunk_y))?).ok()?;
        let storages = self.storages.get(index)?.as_ref()?;
        Some(ProjectedSurfaceStatesAt {
            storages: storages.iter().enumerate().rev(),
            block_index: block_storage_index(local_x, local_y, local_z),
        })
    }
}

pub(crate) fn build_terrain_column_samples(
    pos: ChunkPos,
    version: crate::ChunkVersion,
    subchunks: &BTreeMap<i8, SubChunk>,
    legacy_terrain: Option<&LegacyTerrain>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Result<TerrainColumnSamples> {
    let mut columns = TerrainColumnSamples::new();
    let projection = SurfaceProjection::new(subchunks);
    let (min_y, max_y) = if legacy_terrain.is_some() && subchunks.is_empty() {
        (0, 127)
    } else {
        pos.y_range(version)
    };

    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            if let Some(sample) = sample_column_top_down(
                local_x,
                local_z,
                min_y,
                max_y,
                subchunks,
                projection.as_ref(),
                legacy_terrain,
                height_map,
                legacy_biomes,
                render_biomes,
            )? {
                columns.set(local_x, local_z, sample);
            }
        }
    }
    Ok(columns)
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn sample_column_top_down(
    local_x: u8,
    local_z: u8,
    min_y: i32,
    max_y: i32,
    subchunks: &BTreeMap<i8, SubChunk>,
    projection: Option<&SurfaceProjection<'_>>,
    legacy_terrain: Option<&LegacyTerrain>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Result<Option<TerrainColumnSample>> {
    let mut overlay: Option<TerrainColumnOverlay> = None;
    let mut top_water: Option<(i16, BlockState, TerrainSampleSource)> = None;
    let mut water_depth = 0_u8;
    for y in (min_y..=max_y).rev() {
        let height = i16::try_from(y).unwrap_or(if y < 0 { i16::MIN } else { i16::MAX });

        let subchunk_y = block_y_to_subchunk_y(y)?;
        let local_y = u8::try_from(y - i32::from(subchunk_y) * 16).map_err(|_| {
            BedrockWorldError::Validation(format!("block y={y} has invalid local subchunk offset"))
        })?;
        let mut saw_subchunk_layer = false;
        if let Some(subchunk) = projection.and_then(|projection| projection.get(subchunk_y)) {
            if let Some(projected_states) = projection.and_then(|projection| {
                projection.surface_states_at(subchunk_y, local_x, local_y, local_z)
            }) {
                for projected in projected_states {
                    saw_subchunk_layer = true;
                    if let Some(sample) = scan_terrain_surface_state(
                        local_x,
                        local_z,
                        y,
                        height,
                        projected.entry.state,
                        projected.role,
                        TerrainSampleSource::Subchunk,
                        &mut overlay,
                        &mut top_water,
                        &mut water_depth,
                        legacy_biomes,
                        render_biomes,
                    ) {
                        return Ok(Some(sample));
                    }
                }
            } else {
                for entry in subchunk.visible_block_surface_states_at(local_x, local_y, local_z) {
                    saw_subchunk_layer = true;
                    let role = terrain_surface_role(&entry.state.name);
                    if let Some(sample) = scan_terrain_surface_state(
                        local_x,
                        local_z,
                        y,
                        height,
                        entry.state,
                        role,
                        TerrainSampleSource::Subchunk,
                        &mut overlay,
                        &mut top_water,
                        &mut water_depth,
                        legacy_biomes,
                        render_biomes,
                    ) {
                        return Ok(Some(sample));
                    }
                }
            }
            if saw_subchunk_layer {
                continue;
            }
            if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z) {
                let data = subchunk
                    .legacy_block_data_at(local_x, local_y, local_z)
                    .unwrap_or(0);
                let state = legacy_world_block_state(id, data);
                let role = terrain_surface_role(&state.name);
                if let Some(sample) = scan_terrain_surface_state(
                    local_x,
                    local_z,
                    y,
                    height,
                    &state,
                    role,
                    TerrainSampleSource::Subchunk,
                    &mut overlay,
                    &mut top_water,
                    &mut water_depth,
                    legacy_biomes,
                    render_biomes,
                ) {
                    return Ok(Some(sample));
                }
                continue;
            }
        }

        if let Some((state, source)) =
            legacy_terrain_block_state_at(local_x, y, local_z, subchunks, legacy_terrain)
        {
            let role = terrain_surface_role(&state.name);
            if let Some(sample) = scan_terrain_surface_state(
                local_x,
                local_z,
                y,
                height,
                &state,
                role,
                source,
                &mut overlay,
                &mut top_water,
                &mut water_depth,
                legacy_biomes,
                render_biomes,
            ) {
                return Ok(Some(sample));
            }
        }
    }

    if let Some((water_height, water_state, water_source)) = top_water {
        let biome = terrain_biome_at(
            local_x,
            local_z,
            i32::from(water_height),
            legacy_biomes,
            render_biomes,
        );
        let relief_y = raw_height_at(height_map, local_x, local_z).unwrap_or(water_height);
        return Ok(Some(TerrainColumnSample {
            surface_y: water_height,
            surface_block_state: water_state.clone(),
            relief_y,
            relief_block_state: water_state.clone(),
            overlay,
            water: Some(TerrainColumnWater {
                surface_y: water_height,
                block_state: water_state,
                depth: water_depth,
                underwater_y: None,
                underwater_block_state: None,
                source: water_source,
            }),
            biome,
            source: water_source,
        }));
    }

    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_terrain_surface_state(
    local_x: u8,
    local_z: u8,
    y: i32,
    height: i16,
    state: &BlockState,
    role: TerrainSurfaceRole,
    source: TerrainSampleSource,
    overlay: &mut Option<TerrainColumnOverlay>,
    top_water: &mut Option<(i16, BlockState, TerrainSampleSource)>,
    water_depth: &mut u8,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Option<TerrainColumnSample> {
    match role {
        TerrainSurfaceRole::Air => {
            if top_water.is_some() {
                *water_depth = (*water_depth).saturating_add(1);
            }
            None
        }
        TerrainSurfaceRole::Overlay => {
            if let Some((water_height, water_state, water_source)) = top_water.take() {
                let biome = terrain_biome_at(local_x, local_z, y, legacy_biomes, render_biomes);
                return Some(TerrainColumnSample {
                    surface_y: water_height,
                    surface_block_state: water_state.clone(),
                    relief_y: height,
                    relief_block_state: state.clone(),
                    overlay: overlay.take(),
                    water: Some(TerrainColumnWater {
                        surface_y: water_height,
                        block_state: water_state,
                        depth: (*water_depth).saturating_add(1),
                        underwater_y: Some(height),
                        underwater_block_state: Some(state.clone()),
                        source: water_source,
                    }),
                    biome,
                    source: water_source,
                });
            }
            if overlay.is_none() {
                *overlay = Some(TerrainColumnOverlay {
                    y: height,
                    block_state: state.clone(),
                    source,
                });
            }
            None
        }
        TerrainSurfaceRole::Water => {
            if top_water.is_none() {
                *top_water = Some((height, state.clone(), source));
            } else {
                *water_depth = (*water_depth).saturating_add(1);
            }
            None
        }
        TerrainSurfaceRole::Primary => {
            let biome = terrain_biome_at(local_x, local_z, y, legacy_biomes, render_biomes);
            if let Some((water_height, water_state, water_source)) = top_water.take() {
                return Some(TerrainColumnSample {
                    surface_y: water_height,
                    surface_block_state: water_state.clone(),
                    relief_y: height,
                    relief_block_state: state.clone(),
                    overlay: overlay.take(),
                    water: Some(TerrainColumnWater {
                        surface_y: water_height,
                        block_state: water_state,
                        depth: (*water_depth).saturating_add(1),
                        underwater_y: Some(height),
                        underwater_block_state: Some(state.clone()),
                        source: water_source,
                    }),
                    biome,
                    source: water_source,
                });
            }
            Some(TerrainColumnSample {
                surface_y: height,
                surface_block_state: state.clone(),
                relief_y: height,
                relief_block_state: state.clone(),
                overlay: overlay.take(),
                water: None,
                biome,
                source,
            })
        }
    }
}

pub(crate) fn legacy_terrain_block_state_at(
    local_x: u8,
    y: i32,
    local_z: u8,
    subchunks: &BTreeMap<i8, SubChunk>,
    legacy_terrain: Option<&LegacyTerrain>,
) -> Option<(BlockState, TerrainSampleSource)> {
    let terrain = legacy_terrain?;
    if !(0..=127).contains(&y) {
        return None;
    }
    let legacy_y = u8::try_from(y).ok()?;
    let id = terrain.block_id_at(local_x, legacy_y, local_z)?;
    let data = terrain
        .block_data_at(local_x, legacy_y, local_z)
        .unwrap_or(0);
    let source = if subchunks.is_empty() {
        TerrainSampleSource::LegacyTerrain
    } else {
        TerrainSampleSource::LegacyFallback
    };
    Some((legacy_world_block_state(id, data), source))
}

pub(crate) fn terrain_biome_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    legacy_biomes: Option<&[[Option<LegacyBiomeSample>; 16]; 16]>,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Option<TerrainColumnBiome> {
    legacy_biomes
        .and_then(|samples| samples[usize::from(local_z)][usize::from(local_x)])
        .map(TerrainColumnBiome::Legacy)
        .or_else(|| {
            render_biome_id_at(local_x, local_z, y, render_biomes).map(TerrainColumnBiome::Id)
        })
}

pub(crate) fn render_biome_id_at(
    local_x: u8,
    local_z: u8,
    y: i32,
    render_biomes: &BTreeMap<i32, BiomeStorage>,
) -> Option<u32> {
    let direct = render_biomes
        .get(&biome_storage_bucket_y(y))
        .or_else(|| render_biomes.values().next())
        .and_then(|storage| {
            biome_id_from_storage(storage, local_x, local_z, y).filter(|id| *id != 0)
        });
    if direct.is_some() {
        return direct;
    }
    for storage in render_biomes.values().rev() {
        if storage.y.is_none() {
            if let Some(id) = storage
                .biome_id_at(local_x, 0, local_z)
                .filter(|id| *id != 0)
            {
                return Some(id);
            }
            continue;
        }
        for local_y in (0..16_u8).rev() {
            if let Some(id) = storage
                .biome_id_at(local_x, local_y, local_z)
                .filter(|id| *id != 0)
            {
                return Some(id);
            }
        }
    }
    None
}

pub(crate) fn render_chunk_from_raw(
    raw: RawChunkData,
    options: &ChunkLoadOptions,
) -> Result<(ChunkData, ChunkDecodeTiming)> {
    let mut timing = ChunkDecodeTiming::default();
    let biome_started = Instant::now();
    let legacy_terrain = raw.legacy_terrain.map(LegacyTerrain::parse).transpose()?;
    let version = raw.biome_record.as_ref().map_or_else(
        || {
            if legacy_terrain.is_some() {
                crate::ChunkVersion::Old
            } else {
                crate::ChunkVersion::New
            }
        },
        |(tag, _)| match tag {
            ChunkRecordTag::Data3D => crate::ChunkVersion::New,
            ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => crate::ChunkVersion::Old,
            _ => unreachable!("render biome record contains only biome tags"),
        },
    );
    let biome_data = parse_render_biome_record(raw.biome_record.as_ref())?;
    let height_map = biome_data
        .as_ref()
        .map(|biome_data| render_height_map_from_biome_data(raw.pos, biome_data))
        .or_else(|| {
            legacy_terrain
                .as_ref()
                .map(render_height_map_from_legacy_terrain)
        });
    let legacy_biomes = legacy_terrain
        .as_ref()
        .map(render_biomes_from_legacy_terrain);
    let legacy_biome_colors = legacy_terrain
        .as_ref()
        .map(render_biome_colors_from_legacy_terrain);
    let mut render_biomes = BTreeMap::new();
    let data_request = &options.data_request;
    insert_render_biome_storages(&mut render_biomes, biome_data, data_request);
    timing.biome_parse_us = biome_started.elapsed().as_micros();

    let mut subchunks = BTreeMap::new();
    let subchunk_started = Instant::now();
    let preferred_decode = options.data_request.preferred_decode_mode();
    let subchunk_decode = match (preferred_decode, options.subchunk_decode) {
        (SubChunkDecodeMode::FullIndices, SubChunkDecodeMode::PackedIndices) => {
            SubChunkDecodeMode::PackedIndices
        }
        (SubChunkDecodeMode::CountsOnly, SubChunkDecodeMode::CountsOnly) => {
            SubChunkDecodeMode::CountsOnly
        }
        (SubChunkDecodeMode::SurfaceColumns, SubChunkDecodeMode::SurfaceColumns) => {
            SubChunkDecodeMode::SurfaceColumns
        }
        _ => preferred_decode,
    };
    for (y, value) in raw.subchunks {
        check_render_load_cancelled(options)?;
        subchunks.insert(y, SubChunk::read(y, value, subchunk_decode)?);
    }
    timing.subchunk_parse_us = subchunk_started.elapsed().as_micros();

    let block_entity_started = Instant::now();
    let block_entities = if request_loads_block_entities(options) {
        if let Some(value) = raw.block_entities {
            let mut report = ScanReport::default();
            let block_entities = parse_block_entities_from_value(&value, &mut report);
            if !report.parse_errors.is_empty() {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "block entity record for {:?}: {}",
                    raw.pos,
                    report.parse_errors.join("; ")
                )));
            }
            block_entities
                .into_iter()
                .map(|entity| render_block_entity_from_nbt(entity.nbt))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    timing.block_entity_parse_us = block_entity_started.elapsed().as_micros();

    let surface_scan_started = Instant::now();
    let column_samples = if request_builds_column_samples(options) {
        Some(build_terrain_column_samples(
            raw.pos,
            version,
            &subchunks,
            legacy_terrain.as_ref(),
            height_map.as_ref(),
            legacy_biomes.as_ref(),
            &render_biomes,
        )?)
    } else {
        None
    };
    timing.surface_scan_us = surface_scan_started.elapsed().as_micros();

    Ok((
        ChunkData {
            pos: raw.pos,
            is_loaded: height_map.is_some()
                || legacy_biome_colors.is_some()
                || legacy_biomes.is_some()
                || !render_biomes.is_empty()
                || !subchunks.is_empty()
                || !block_entities.is_empty()
                || legacy_terrain.is_some(),
            height_map,
            legacy_biomes,
            legacy_biome_colors,
            biome_data: render_biomes,
            subchunks,
            block_entities,
            legacy_terrain,
            column_samples,
            version,
        },
        timing,
    ))
}

pub(crate) fn render_load_stats(
    chunks: &[ChunkData],
    worker_threads: usize,
    queue_wait_ms: u128,
    load_ms: u128,
) -> ChunkLoadStats {
    ChunkLoadStats {
        requested_chunks: chunks.len(),
        loaded_chunks: chunks.iter().filter(|chunk| chunk.is_loaded).count(),
        subchunks_decoded: chunks
            .iter()
            .map(|chunk| chunk.subchunks.len())
            .sum::<usize>(),
        worker_threads,
        queue_wait_ms,
        load_ms,
        keys_requested: 0,
        keys_found: 0,
        exact_get_batches: 0,
        prefix_scans: 0,
        decode_ms: 0,
        db_read_ms: 0,
        biome_parse_ms: 0,
        biome_parse_us: 0,
        subchunk_parse_ms: 0,
        subchunk_parse_us: 0,
        surface_scan_ms: 0,
        surface_scan_us: 0,
        block_entity_parse_ms: 0,
        block_entity_parse_us: 0,
        full_reload_ms: 0,
        legacy_terrain_records: chunks
            .iter()
            .filter(|chunk| chunk.legacy_terrain.is_some())
            .count(),
        legacy_biome_samples: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biomes.is_some())
            .count(),
        legacy_biome_colors: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biome_colors.is_some())
            .count(),
        terrain_source_legacy: chunks
            .iter()
            .filter(|chunk| chunk.legacy_terrain.is_some() && chunk.subchunks.is_empty())
            .count(),
        terrain_source_subchunk: chunks
            .iter()
            .filter(|chunk| !chunk.subchunks.is_empty())
            .count(),
        legacy_pocket_chunks: 0,
        detected_format: WorldFormat::LevelDb,
        computed_surface_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .map(TerrainColumnSamples::sampled_columns)
            .sum(),
        raw_height_mismatch_columns: chunks.iter().map(raw_height_mismatch_columns).sum(),
        missing_subchunk_columns: chunks.iter().map(missing_surface_columns).sum(),
        legacy_fallback_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| sample.source == TerrainSampleSource::LegacyFallback)
            .count(),
        legacy_biome_preferred_columns: chunks
            .iter()
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| matches!(sample.biome, Some(TerrainColumnBiome::Legacy(_))))
            .count(),
        modern_biome_fallback_columns: chunks
            .iter()
            .filter(|chunk| chunk.legacy_biomes.is_some())
            .filter_map(|chunk| chunk.column_samples.as_ref())
            .flat_map(TerrainColumnSamples::iter)
            .filter(|sample| matches!(sample.biome, Some(TerrainColumnBiome::Id(_))))
            .count(),
    }
}

pub(crate) fn log_render_load_complete(stats: &ChunkLoadStats) {
    log::debug!(
        "render chunk load complete (requested_chunks={}, loaded_chunks={}, missing_chunks={}, subchunks_decoded={}, legacy_terrain_records={}, legacy_biome_samples={}, legacy_biome_colors={}, terrain_source_legacy={}, terrain_source_subchunk={}, legacy_pocket_chunks={}, detected_format={:?}, computed_surface_columns={}, raw_height_mismatch_columns={}, missing_subchunk_columns={}, legacy_fallback_columns={}, legacy_biome_preferred_columns={}, modern_biome_fallback_columns={}, worker_threads={}, queue_wait_ms={}, load_ms={}, exact_get_batches={}, keys_requested={}, keys_found={}, prefix_scans={}, db_read_ms={}, decode_ms={}, biome_parse_ms={}, subchunk_parse_ms={}, surface_scan_ms={}, block_entity_parse_ms={}, full_reload_ms={})",
        stats.requested_chunks,
        stats.loaded_chunks,
        stats.requested_chunks.saturating_sub(stats.loaded_chunks),
        stats.subchunks_decoded,
        stats.legacy_terrain_records,
        stats.legacy_biome_samples,
        stats.legacy_biome_colors,
        stats.terrain_source_legacy,
        stats.terrain_source_subchunk,
        stats.legacy_pocket_chunks,
        stats.detected_format,
        stats.computed_surface_columns,
        stats.raw_height_mismatch_columns,
        stats.missing_subchunk_columns,
        stats.legacy_fallback_columns,
        stats.legacy_biome_preferred_columns,
        stats.modern_biome_fallback_columns,
        stats.worker_threads,
        stats.queue_wait_ms,
        stats.load_ms,
        stats.exact_get_batches,
        stats.keys_requested,
        stats.keys_found,
        stats.prefix_scans,
        stats.db_read_ms,
        stats.decode_ms,
        stats.biome_parse_ms,
        stats.subchunk_parse_ms,
        stats.surface_scan_ms,
        stats.block_entity_parse_ms,
        stats.full_reload_ms
    );
}

pub(crate) fn to_storage_read_options(options: &WorldScanOptions) -> StorageReadOptions {
    StorageReadOptions {
        threading: match options.threading {
            WorldThreadingOptions::Auto => StorageThreadingOptions::Auto,
            WorldThreadingOptions::Fixed(threads) => StorageThreadingOptions::Fixed(threads),
            WorldThreadingOptions::Single => StorageThreadingOptions::Single,
        },
        scan_mode: match options.threading {
            WorldThreadingOptions::Single => StorageScanMode::Sequential,
            WorldThreadingOptions::Auto | WorldThreadingOptions::Fixed(_) => {
                StorageScanMode::ParallelTables
            }
        },
        cache_policy: StorageCachePolicy::Bypass,
        pipeline: crate::storage::StoragePipelineOptions {
            queue_depth: options.pipeline.queue_depth,
            table_batch_size: options.pipeline.chunk_batch_size,
            progress_interval: options.pipeline.progress_interval,
        },
        cancel: options
            .cancel
            .as_ref()
            .map(|cancel| StorageCancelFlag::from_shared(cancel.0.clone())),
        progress: options.progress.as_ref().map(|progress| {
            let progress = progress.clone();
            StorageProgressSink::new(move |storage_progress| {
                progress.emit(WorldScanProgress {
                    entries_seen: storage_progress.entries_seen,
                });
            })
        }),
    }
}

pub(crate) fn to_render_storage_read_options(options: &ChunkLoadOptions) -> StorageReadOptions {
    StorageReadOptions {
        threading: match options.threading {
            WorldThreadingOptions::Auto => StorageThreadingOptions::Auto,
            WorldThreadingOptions::Fixed(threads) => StorageThreadingOptions::Fixed(threads),
            WorldThreadingOptions::Single => StorageThreadingOptions::Single,
        },
        scan_mode: StorageScanMode::Sequential,
        cache_policy: options.storage_cache_policy,
        pipeline: crate::storage::StoragePipelineOptions {
            queue_depth: options.pipeline.queue_depth,
            table_batch_size: options.pipeline.chunk_batch_size,
            progress_interval: options.pipeline.progress_interval,
        },
        cancel: options.cancel.as_ref().map(CancelFlag::to_storage_cancel),
        progress: None,
    }
}

pub(crate) fn chunk_record_prefix(pos: ChunkPos) -> Bytes {
    let mut bytes = Vec::with_capacity(if pos.dimension == crate::Dimension::Overworld {
        8
    } else {
        12
    });
    bytes.extend_from_slice(&pos.x.to_le_bytes());
    bytes.extend_from_slice(&pos.z.to_le_bytes());
    if pos.dimension != crate::Dimension::Overworld {
        bytes.extend_from_slice(&pos.dimension.id().to_le_bytes());
    }
    Bytes::from(bytes)
}

pub(crate) fn validate_render_region(region: Region) -> Result<()> {
    if region.min_chunk_x > region.max_chunk_x || region.min_chunk_z > region.max_chunk_z {
        return Err(BedrockWorldError::Validation(format!(
            "invalid render region: min=({}, {}) max=({}, {})",
            region.min_chunk_x, region.min_chunk_z, region.max_chunk_x, region.max_chunk_z
        )));
    }
    Ok(())
}

pub(crate) fn render_block_entity_from_nbt(nbt: NbtTag) -> ChunkBlockEntity {
    let root = match &nbt {
        NbtTag::Compound(root) => Some(root),
        _ => None,
    };
    ChunkBlockEntity {
        id: root
            .and_then(|root| nbt_string_field(root, "id"))
            .map(ToString::to_string),
        position: root.and_then(|root| {
            Some([
                nbt_int_field(root, "x")?,
                nbt_int_field(root, "y")?,
                nbt_int_field(root, "z")?,
            ])
        }),
        nbt,
    }
}

pub(crate) fn nbt_string_field<'a>(
    root: &'a indexmap::IndexMap<String, NbtTag>,
    key: &str,
) -> Option<&'a str> {
    match root.get(key) {
        Some(NbtTag::String(value)) => Some(value),
        _ => None,
    }
}

pub(crate) fn nbt_int_field(root: &indexmap::IndexMap<String, NbtTag>, key: &str) -> Option<i32> {
    match root.get(key) {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

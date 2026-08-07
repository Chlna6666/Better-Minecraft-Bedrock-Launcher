from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, got {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_pos = text.find(start)
    if start_pos < 0:
        raise RuntimeError(f"{label}: start marker missing")
    end_pos = text.find(end, start_pos)
    if end_pos < 0:
        raise RuntimeError(f"{label}: end marker missing")
    return text[:start_pos] + replacement + text[end_pos:]


# ---------------------------------------------------------------------------
# Persistent map-info cache: validate cache against the current Bedrock actor
# digest / actorprefix records instead of trusting an old payload forever.
# ---------------------------------------------------------------------------
path = ROOT / "src/core/minecraft/map_info_cache.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "use bedrock_world::{\n    BedrockWorld, CancelFlag, ChunkPos, ChunkRecordQuery, ChunkRecordQueryResult, Dimension,\n    ParsedChunkRecordValue, query_chunk_records_many_blocking_with_control,\n};",
    "use bedrock_world::{\n    BedrockWorld, CancelFlag, ChunkPos, ChunkRecordFingerprint, ChunkRecordQuery,\n    ChunkRecordQueryResult, Dimension, ParsedChunkRecordValue,\n    fingerprint_chunk_records_many_blocking_with_control,\n    query_chunk_records_many_blocking_with_control,\n};",
    "map-info imports",
)
text = replace_once(
    text,
    "use xxhash_rust::xxh3::xxh3_128;",
    "use xxhash_rust::xxh3::{Xxh3, xxh3_128};",
    "xxh3 import",
)
text = replace_once(text, "const CACHE_VERSION: u16 = 2;", "const CACHE_VERSION: u16 = 3;", "cache version")
text = replace_once(
    text,
    "    /// Number of tiles rebuilt from world records.\n    pub rebuilt_tile_count: usize,\n}",
    "    /// Number of tiles rebuilt from world records.\n    pub rebuilt_tile_count: usize,\n    /// Number of map-information tiles requested for this snapshot.\n    pub requested_tile_count: usize,\n}",
    "snapshot requested count",
)
text = replace_once(
    text,
    "struct MapInfoIndexEntry {\n    payload_hash: u128,\n}",
    "struct MapInfoIndexEntry {\n    payload_hash: u128,\n    /// Fingerprint of the selected LevelDB records, including digp and actorprefix data.\n    source_hash: u128,\n}",
    "index source hash",
)

start = "pub fn load_map_info_tiles_blocking(\n"
end = "fn query_map_info_records_parallel(\n"
replacement = r'''pub fn load_map_info_tiles_blocking(
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
    let requested_tile_count = keys.len();
    let cache = MapInfoCache::for_world(world_path);
    let mut index = cache.load_index()?;

    // Validate the compact persistent cache against exactly the records that feed it.
    // The fingerprint path does not decode NBT, but it does include the legacy Entity
    // record, the modern digp actor digest and every referenced actorprefix record.
    // This makes cache hits reliable even when Minecraft or another editor changed the
    // world behind BMCBL's back.
    cancel_if_requested(cancel)?;
    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .context("open world for map information cache validation")?;
    let source_hashes = map_info_source_hashes(&world, &keys, cancel)?;

    let mut payloads = BTreeMap::new();
    let mut rebuild_keys = Vec::new();
    let mut cached_tile_count = 0usize;
    for key in &keys {
        let Some(source_hash) = source_hashes.get(key).copied() else {
            rebuild_keys.push(*key);
            continue;
        };
        let Some(entry) = index.entries.get(key).copied() else {
            rebuild_keys.push(*key);
            continue;
        };
        if entry.source_hash != source_hash {
            rebuild_keys.push(*key);
            continue;
        }
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
        let records = query_map_info_records_parallel(&world, &rebuild_keys, cancel, max_workers)?;
        let records_by_tile = records_by_tile(records, chunks_per_tile);
        for key in &rebuild_keys {
            let payload = records_by_tile
                .get(key)
                .map_or_else(MapInfoTilePayload::default, |records| {
                    MapInfoTilePayload::from_records(records)
                });
            let payload_hash = cache.store_tile(*key, &payload)?;
            let source_hash = source_hashes.get(key).copied().unwrap_or_default();
            index.entries.insert(
                *key,
                MapInfoIndexEntry {
                    payload_hash,
                    source_hash,
                },
            );
            payloads.insert(*key, payload);
        }
        cache.store_index(&index)?;
    }

    Ok(MapInfoOverlaySnapshot::from_payloads(
        payloads.into_values(),
        cached_tile_count,
        rebuild_keys.len(),
        requested_tile_count,
    ))
}

fn map_info_source_hashes(
    world: &BedrockWorld,
    keys: &[MapInfoTileKey],
    cancel: &CancelFlag,
) -> Result<BTreeMap<MapInfoTileKey, u128>> {
    let fingerprints = fingerprint_chunk_records_many_blocking_with_control(
        world,
        chunks_for_keys(keys)?,
        map_info_record_query(),
        cancel,
    )?;
    Ok(source_hashes_by_tile(fingerprints, keys.first().map_or(1, |key| key.chunks_per_tile)))
}

fn source_hashes_by_tile(
    fingerprints: Vec<ChunkRecordFingerprint>,
    chunks_per_tile: u16,
) -> BTreeMap<MapInfoTileKey, u128> {
    let edge = i32::from(chunks_per_tile).max(1);
    let mut hashers = BTreeMap::<MapInfoTileKey, Xxh3>::new();
    for fingerprint in fingerprints {
        let key = MapInfoTileKey {
            dimension_id: fingerprint.pos.dimension.id(),
            tile_x: fingerprint.pos.x.div_euclid(edge),
            tile_z: fingerprint.pos.z.div_euclid(edge),
            chunks_per_tile,
        };
        let hasher = hashers.entry(key).or_insert_with(Xxh3::new);
        hasher.update(&fingerprint.pos.x.to_le_bytes());
        hasher.update(&fingerprint.pos.z.to_le_bytes());
        hasher.update(&fingerprint.pos.dimension.id().to_le_bytes());
        hasher.update(&fingerprint.value.to_le_bytes());
    }
    hashers
        .into_iter()
        .map(|(key, hasher)| (key, hasher.digest128()))
        .collect()
}

'''
text = replace_between(text, start, end, replacement, "map-info full loader")

text = replace_once(
    text,
    "    let cached_tile_count = payloads.len();\n    Ok(MapInfoOverlaySnapshot::from_payloads(\n        payloads.into_values(),\n        cached_tile_count,\n        0,\n    ))",
    "    let cached_tile_count = payloads.len();\n    Ok(MapInfoOverlaySnapshot::from_payloads(\n        payloads.into_values(),\n        cached_tile_count,\n        0,\n        keys.len(),\n    ))",
    "cached snapshot requested count",
)
text = replace_once(
    text,
    "    fn from_payloads(\n        payloads: impl IntoIterator<Item = MapInfoTilePayload>,\n        cached_tile_count: usize,\n        rebuilt_tile_count: usize,\n    ) -> Self {\n        let mut snapshot = Self {\n            cached_tile_count,\n            rebuilt_tile_count,\n            ..Self::default()\n        };",
    "    fn from_payloads(\n        payloads: impl IntoIterator<Item = MapInfoTilePayload>,\n        cached_tile_count: usize,\n        rebuilt_tile_count: usize,\n        requested_tile_count: usize,\n    ) -> Self {\n        let mut snapshot = Self {\n            cached_tile_count,\n            rebuilt_tile_count,\n            requested_tile_count,\n            ..Self::default()\n        };",
    "snapshot constructor signature",
)
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Overlay lifecycle: viewport-paged tile-aligned queries and explicit complete
# vs provisional cache state.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/model.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,\n    pub(super) entity_avatar_pool: BTreeMap<String, Arc<RenderImage>>,",
    "    pub(super) overlay_paint: Option<Arc<ProfessionalOverlayPaintCache>>,\n    /// True only after the current overlay scope was validated against LevelDB.\n    /// A fast disk-cache preview is intentionally provisional.\n    pub(super) overlay_complete: bool,\n    pub(super) entity_avatar_pool: BTreeMap<String, Arc<RenderImage>>,",
    "overlay complete state",
)
path.write_text(text, encoding="utf-8")

path = ROOT / "src/ui/window/map_viewer/overlays.rs"
text = path.read_text(encoding="utf-8")
text = replace_once(
    text,
    "            let changed = self.professional.overlay_bounds != Some(bounds)\n                || self.professional.overlay_paint.is_none();\n            if changed {\n                let mut overlay = (*cached).clone();\n                overlay.bind_entity_avatars(&self.professional.entity_avatar_pool);\n                self.professional.overlay_paint = Some(Arc::new(overlay));\n            }\n            self.professional.overlay_bounds = Some(bounds);\n            self.professional.pending_overlay_refresh = false;\n            if changed {\n                self.sync_professional_render_snapshot(cx);\n            }\n            return;",
    "            let was_complete = self.professional.overlay_complete;\n            let changed = self.professional.overlay_bounds != Some(bounds)\n                || self.professional.overlay_paint.is_none();\n            if changed {\n                let mut overlay = (*cached).clone();\n                overlay.bind_entity_avatars(&self.professional.entity_avatar_pool);\n                self.professional.overlay_paint = Some(Arc::new(overlay));\n            }\n            self.professional.overlay_bounds = Some(bounds);\n            self.professional.overlay_complete = true;\n            self.professional.pending_overlay_refresh = false;\n            if changed || !was_complete {\n                self.sync_professional_render_snapshot(cx);\n            }\n            return;",
    "validated memory cache state",
)
text = replace_once(
    text,
    "        if should_defer_overlay_query_for_visible_tiles(\n            self.render_batch_active,\n            self.tile_manager.has_visible_work(),\n        ) {",
    "        // Entity markers are lightweight, viewport-paged LevelDB reads. Do not let a\n        // long terrain render queue starve them; other professional overlays retain the\n        // conservative defer policy.\n        if !self.overlay_options.entities\n            && should_defer_overlay_query_for_visible_tiles(\n                self.render_batch_active,\n                self.tile_manager.has_visible_work(),\n            )\n        {",
    "entity query starvation",
)
text = replace_once(
    text,
    "        if self.professional.overlay_bounds == Some(bounds)\n            && self.professional.last_overlay_request_options == Some(options)\n            && self.professional.overlay_paint.is_some()\n        {",
    "        if self.professional.overlay_bounds == Some(bounds)\n            && self.professional.last_overlay_request_options == Some(options)\n            && self.professional.overlay_paint.is_some()\n            && self.professional.overlay_complete\n        {",
    "complete early return",
)
text = replace_once(
    text,
    "        self.professional.overlay_loading = true;\n        self.professional.pending_overlay_refresh = false;",
    "        self.professional.overlay_loading = true;\n        self.professional.overlay_complete = false;\n        self.professional.pending_overlay_refresh = false;",
    "query start complete state",
)
text = replace_once(
    text,
    "                            this.professional.overlay_bounds = Some(bounds);\n                            let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(\n                                &map_info, &villages,\n                            );",
    "                            this.professional.overlay_bounds = Some(bounds);\n                            // Cached tiles are an immediate preview only. The following full\n                            // stage validates source fingerprints and fills every missing/stale tile.\n                            this.professional.overlay_complete = false;\n                            let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(\n                                &map_info, &villages,\n                            );",
    "provisional disk preview state",
)
text = replace_once(
    text,
    "                            this.status = SharedString::from(format!(\n                                \"已显示缓存叠加层 · 缓存 {} · 正在补齐未缓存区域\",\n                                map_info.cached_tile_count\n                            ));",
    "                            this.status = SharedString::from(format!(\n                                \"已显示实体/叠加缓存 {}/{} · 正在校验并补齐当前区域\",\n                                map_info.cached_tile_count, map_info.requested_tile_count\n                            ));",
    "provisional status",
)
text = replace_once(
    text,
    "                        this.professional.overlay_bounds = Some(bounds);\n                        let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(\n                            &map_info, &villages,\n                        );",
    "                        this.professional.overlay_bounds = Some(bounds);\n                        this.professional.overlay_complete = true;\n                        let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(\n                            &map_info, &villages,\n                        );",
    "full result complete state",
)
text = replace_once(
    text,
    "                        this.status = SharedString::from(format!(\n                            \"地图叠加层已更新 · 缓存 {} · 重建 {}\",\n                            map_info.cached_tile_count, map_info.rebuilt_tile_count\n                        ));",
    "                        this.status = SharedString::from(format!(\n                            \"实体/叠加区域已完整 · 缓存 {}/{} · 重建 {}\",\n                            map_info.cached_tile_count,\n                            map_info.requested_tile_count,\n                            map_info.rebuilt_tile_count\n                        ));",
    "full status",
)
text = replace_once(
    text,
    "                    Err(error) => {\n                        if error.contains(\"cancelled\") || error.contains(\"cancel\") {",
    "                    Err(error) => {\n                        this.professional.overlay_complete = false;\n                        if error.contains(\"cancelled\") || error.contains(\"cancel\") {",
    "error complete state",
)

old_invalidate = r'''    pub(super) fn invalidate_professional_overlay_for_viewport_change(&mut self) {
        self.cancel_slime_window_candidate_query();
        self.professional.slime_window_candidates = None;
        if self.metadata_index_ready
            && self.chunk_bounds.is_some()
            && !self.available_tiles.is_empty()
        {
            return;
        }

        self.map_query_budget.next_generation(MapQueryKind::Overlay);
        if let Some(cancel) = self.professional.overlay_cancel.take() {
            cancel.cancel();
        }
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.professional.overlay_loading = false;
        self.professional.overlay_bounds = None;
        // Keep the last immutable paint cache while the new viewport query is
        // running. It follows the viewport and is replaced atomically on
        // completion, so dragging never flashes an empty overlay layer.
        self.professional.pending_overlay_refresh = true;
        self.professional.last_overlay_request_bounds = None;
        self.professional.last_overlay_request_options = None;
    }
'''
new_invalidate = r'''    pub(super) fn invalidate_professional_overlay_for_viewport_change(&mut self) {
        self.cancel_slime_window_candidate_query();
        self.professional.slime_window_candidates = None;

        // Entity/map-info data is demand-paged by the current viewport. An indexed world
        // must therefore invalidate its query scope too; the previous early return made a
        // partial whole-world cache look permanent after the camera moved elsewhere.
        self.map_query_budget.next_generation(MapQueryKind::Overlay);
        if let Some(cancel) = self.professional.overlay_cancel.take() {
            cancel.cancel();
        }
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.professional.overlay_loading = false;
        self.professional.overlay_complete = false;
        self.professional.overlay_bounds = None;
        // Retain the last immutable paint cache until the replacement arrives so a drag
        // never flashes an empty overlay layer. World coordinates keep old points harmless.
        self.professional.pending_overlay_refresh = true;
        self.professional.last_overlay_request_bounds = None;
        self.professional.last_overlay_request_options = None;
    }
'''
text = replace_once(text, old_invalidate, new_invalidate, "viewport overlay invalidation")

scope_start = "pub(super) fn map_info_query_scope(\n"
scope_end = "pub(super) fn accept_overlay_result(\n"
new_scope = r'''const MAP_INFO_PREFETCH_TILE_RADIUS: i32 = 1;
const MAP_INFO_PREFETCH_VISIBLE_TILE_LIMIT: i64 = 64;

pub(super) fn map_info_query_scope(
    metadata_index_ready: bool,
    dimension: Dimension,
    chunk_bounds: Option<ChunkBounds>,
    _available_tiles: &BTreeSet<(i32, i32)>,
    visible_bounds: Option<SlimeChunkBounds>,
    chunks_per_tile: u16,
) -> Option<MapInfoQueryScope> {
    let visible_bounds = visible_bounds?;
    if visible_bounds.dimension != dimension {
        return None;
    }
    let edge = i32::from(chunks_per_tile).max(1);
    let mut min_tile_x = visible_bounds.min_chunk_x.div_euclid(edge);
    let mut max_tile_x = visible_bounds.max_chunk_x.div_euclid(edge);
    let mut min_tile_z = visible_bounds.min_chunk_z.div_euclid(edge);
    let mut max_tile_z = visible_bounds.max_chunk_z.div_euclid(edge);

    // Align requests to persistent map-info tiles. Small camera movement inside the same
    // tile then reuses both the memory Overlay cache and the on-disk tile payloads.
    let visible_tile_width = i64::from(max_tile_x)
        .saturating_sub(i64::from(min_tile_x))
        .saturating_add(1);
    let visible_tile_height = i64::from(max_tile_z)
        .saturating_sub(i64::from(min_tile_z))
        .saturating_add(1);
    let visible_tile_count = visible_tile_width.saturating_mul(visible_tile_height);
    let prefetch = if visible_tile_count <= MAP_INFO_PREFETCH_VISIBLE_TILE_LIMIT {
        MAP_INFO_PREFETCH_TILE_RADIUS
    } else {
        0
    };
    min_tile_x = min_tile_x.saturating_sub(prefetch);
    max_tile_x = max_tile_x.saturating_add(prefetch);
    min_tile_z = min_tile_z.saturating_sub(prefetch);
    max_tile_z = max_tile_z.saturating_add(prefetch);

    let indexed_bounds = chunk_bounds.filter(|bounds| metadata_index_ready && bounds.dimension == dimension);
    if let Some(world_bounds) = indexed_bounds {
        min_tile_x = min_tile_x.max(world_bounds.min_chunk_x.div_euclid(edge));
        max_tile_x = max_tile_x.min(world_bounds.max_chunk_x.div_euclid(edge));
        min_tile_z = min_tile_z.max(world_bounds.min_chunk_z.div_euclid(edge));
        max_tile_z = max_tile_z.min(world_bounds.max_chunk_z.div_euclid(edge));
    }
    if min_tile_x > max_tile_x || min_tile_z > max_tile_z {
        return None;
    }

    let mut tile_coordinates = Vec::new();
    for tile_z in min_tile_z..=max_tile_z {
        for tile_x in min_tile_x..=max_tile_x {
            tile_coordinates.push((tile_x, tile_z));
        }
    }

    let mut bounds = SlimeChunkBounds {
        dimension,
        min_chunk_x: min_tile_x.saturating_mul(edge),
        max_chunk_x: max_tile_x
            .saturating_add(1)
            .saturating_mul(edge)
            .saturating_sub(1),
        min_chunk_z: min_tile_z.saturating_mul(edge),
        max_chunk_z: max_tile_z
            .saturating_add(1)
            .saturating_mul(edge)
            .saturating_sub(1),
    };
    if let Some(world_bounds) = indexed_bounds {
        bounds.min_chunk_x = bounds.min_chunk_x.max(world_bounds.min_chunk_x);
        bounds.max_chunk_x = bounds.max_chunk_x.min(world_bounds.max_chunk_x);
        bounds.min_chunk_z = bounds.min_chunk_z.max(world_bounds.min_chunk_z);
        bounds.max_chunk_z = bounds.max_chunk_z.min(world_bounds.max_chunk_z);
    }

    Some(MapInfoQueryScope {
        bounds,
        tile_coordinates,
        indexed_world: indexed_bounds.is_some(),
    })
}

'''
text = replace_between(text, scope_start, scope_end, new_scope, "viewport-paged map-info scope")
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Regression tests for the scope behavior that previously expanded to the
# complete world and prevented other areas from loading on demand.
# ---------------------------------------------------------------------------
path = ROOT / "src/ui/window/map_viewer/tests.rs"
text = path.read_text(encoding="utf-8")
anchor = r'''#[::core::prelude::v1::test]
fn entity_avatar_keys_accept_namespaced_identifiers() {
'''
tests = r'''#[::core::prelude::v1::test]
fn map_info_scope_stays_viewport_paged_when_world_index_is_ready() {
    let world_bounds = ChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -1024,
        max_chunk_x: 1024,
        min_chunk_z: -1024,
        max_chunk_z: 1024,
    };
    let visible = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 10,
        max_chunk_x: 20,
        min_chunk_z: 30,
        max_chunk_z: 40,
    };
    let scope = map_info_query_scope(
        true,
        Dimension::Overworld,
        Some(world_bounds),
        &BTreeSet::new(),
        Some(visible),
        8,
    )
    .expect("map info scope");

    assert!(scope.indexed_world);
    assert!(scope.bounds.min_chunk_x > world_bounds.min_chunk_x);
    assert!(scope.bounds.max_chunk_x < world_bounds.max_chunk_x);
    assert!(scope.bounds.min_chunk_z > world_bounds.min_chunk_z);
    assert!(scope.bounds.max_chunk_z < world_bounds.max_chunk_z);
    assert!(scope.tile_coordinates.len() <= 20);
}

#[::core::prelude::v1::test]
fn map_info_scope_is_stable_inside_the_same_cache_tile() {
    let first = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 9,
        max_chunk_x: 14,
        min_chunk_z: 17,
        max_chunk_z: 22,
    };
    let second = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 10,
        max_chunk_x: 15,
        min_chunk_z: 18,
        max_chunk_z: 23,
    };
    let first_scope = map_info_query_scope(
        false,
        Dimension::Overworld,
        None,
        &BTreeSet::new(),
        Some(first),
        8,
    )
    .expect("first map info scope");
    let second_scope = map_info_query_scope(
        false,
        Dimension::Overworld,
        None,
        &BTreeSet::new(),
        Some(second),
        8,
    )
    .expect("second map info scope");

    assert_eq!(first_scope.bounds, second_scope.bounds);
    assert_eq!(first_scope.tile_coordinates, second_scope.tile_coordinates);
}

'''
text = replace_once(text, anchor, tests + anchor, "map info scope tests")
path.write_text(text, encoding="utf-8")

print("map entity query/cache v2 patch applied")

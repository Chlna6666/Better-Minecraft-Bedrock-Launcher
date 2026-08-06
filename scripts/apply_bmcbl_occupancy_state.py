from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def item_end(text: str, brace: int) -> int:
    depth = 0
    in_string = False
    escaped = False
    index = brace
    while index < len(text):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise RuntimeError("unterminated Rust item")


def item_start_with_docs(text: str, signature_start: int) -> int:
    start = text.rfind("\n", 0, signature_start) + 1
    cursor = start
    while cursor > 0:
        previous_end = cursor - 1
        previous_start = text.rfind("\n", 0, previous_end) + 1
        line = text[previous_start:previous_end].strip()
        if line.startswith("///") or line.startswith("#[") or line == "":
            start = previous_start
            cursor = previous_start
        else:
            break
    return start


def replace_named_function(text: str, name: str, replacement: str) -> str:
    match = re.search(
        rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\(",
        text,
    )
    if match is None:
        raise RuntimeError(f"function {name} not found")
    brace = text.find("{", match.end())
    start = item_start_with_docs(text, match.start())
    end = item_end(text, brace)
    while end < len(text) and text[end] in " \t\r\n":
        end += 1
    return text[:start] + replacement.rstrip() + "\n\n" + text[end:]


def remove_named_functions(text: str, names: list[str]) -> str:
    for name in names:
        while re.search(
            rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\(",
            text,
        ):
            text = replace_named_function(text, name, "")
    return text


def remove_struct_and_impls(text: str, name: str) -> str:
    while True:
        match = re.search(rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+{name}\b", text)
        if match is None:
            break
        brace = text.find("{", match.end())
        start = item_start_with_docs(text, match.start())
        end = item_end(text, brace)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]
    while True:
        match = re.search(rf"(?m)^\s*impl(?:<[^>]+>)?\s+{name}\b", text)
        if match is None:
            break
        brace = text.find("{", match.end())
        start = item_start_with_docs(text, match.start())
        end = item_end(text, brace)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]
    return text


def remove_tests_with_needles(text: str, needles: tuple[str, ...]) -> str:
    cursor = 0
    while True:
        attr = text.find("#[test]", cursor)
        if attr < 0:
            return text
        fn_match = re.search(r"(?m)^\s*fn\s+\w+\s*\(", text[attr:])
        if fn_match is None:
            return text
        brace = text.find("{", attr + fn_match.end())
        if brace < 0:
            return text
        end = item_end(text, brace)
        block = text[attr:end]
        if not any(needle in block for needle in needles):
            cursor = end
            continue
        start = item_start_with_docs(text, attr)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]
        cursor = start


def patch_renderer_cache() -> None:
    path = "crates/bedrock-render/src/renderer/cache.rs"
    text = read(path)
    for name in ["TileManifestCacheKey", "TileManifestCacheSnapshot", "TileManifestCache"]:
        text = remove_struct_and_impls(text, name)
    text = remove_named_functions(
        text,
        [
            "tile_manifest_cache_path",
            "encode_tile_manifest_cache",
            "decode_tile_manifest_cache",
            "encode_manifest_bounds",
            "decode_manifest_bounds",
        ],
    )
    text = remove_tests_with_needles(text, ("tile_manifest", "TileManifest"))
    text = re.sub(r"(?m)^const TILE_MANIFEST_CACHE_.*\n", "", text)
    text = re.sub(r"(?m)^static TILE_MANIFEST_CACHE_.*\n", "", text)
    write(path, text)


def add_tile_occupancy_adapter() -> None:
    write(
        "src/ui/window/map_viewer/tile_occupancy.rs",
        r'''use super::model::*;
use super::prelude::*;

pub(super) struct LoadedTileOccupancy {
    pub(super) index: Arc<TileOccupancyIndex>,
    pub(super) source: TileOccupancyIndexSource,
}

pub(super) fn load_tile_occupancy_index(
    world_path: PathBuf,
    dimension: Dimension,
    layout: RenderLayout,
    cancel: RenderTaskControl,
) -> Result<LoadedTileOccupancy, String> {
    let request = TileOccupancyIndexRequest::new(
        world_path,
        file_ops::cache_subdir("bedrock-render"),
        dimension,
        layout,
    );
    let result = load_or_build_tile_occupancy_index_blocking(request, &cancel)
        .map_err(|error| format!("加载地图占用索引失败: {error}"))?;
    Ok(LoadedTileOccupancy {
        index: Arc::new(result.index),
        source: result.source,
    })
}

pub(super) fn materialize_occupancy_chunks(
    index: &TileOccupancyIndex,
    coord: (i32, i32),
) -> Option<TileChunkPositions> {
    index
        .chunk_positions(coord.0, coord.1)
        .map(|positions| Arc::<[ChunkPos]>::from(positions))
}

pub(super) fn occupancy_center_block(index: &TileOccupancyIndex) -> Option<(i32, i32)> {
    let bounds = index.bounds()?;
    let center_chunk_x = bounds
        .min_chunk_x
        .saturating_add(bounds.max_chunk_x)
        .div_euclid(2);
    let center_chunk_z = bounds
        .min_chunk_z
        .saturating_add(bounds.max_chunk_z)
        .div_euclid(2);
    Some((
        center_chunk_x.saturating_mul(16).saturating_add(8),
        center_chunk_z.saturating_mul(16).saturating_add(8),
    ))
}
''',
    )


def patch_modules_and_prelude() -> None:
    path = "src/ui/window/map_viewer.rs"
    text = read(path)
    text = text.replace("mod tile_manifest_legacy;\nmod tile_manifest;\n", "mod tile_occupancy;\n")
    write(path, text)

    path = "src/ui/window/map_viewer/prelude.rs"
    text = read(path)
    text = text.replace("SurfaceRenderOptions, TerrainLightingOptions, TileCoord, TileManifestProbeRequest,\n    TilePixelFormat, TileReadySource, TileStreamEventV2,", "SurfaceRenderOptions, TerrainLightingOptions, TileCoord, TileOccupancyIndex,\n    TileOccupancyIndexRequest, TileOccupancyIndexSource, TilePixelFormat, TileReadySource,\n    TileStreamEventV2, load_or_build_tile_occupancy_index_blocking,")
    write(path, text)


def patch_model() -> None:
    path = "src/ui/window/map_viewer/model.rs"
    text = read(path)
    text = re.sub(r"(?m)^pub\(super\) const TILE_MANIFEST_PROBE_.*\n", "", text)
    text = remove_struct_and_impls(text, "ManifestProbeDiagnostics")
    text = remove_struct_and_impls(text, "TileManifestProbeResult")
    text = text.replace(
        "    pub(super) tile_chunk_index: TileChunkIndex,\n",
        "    pub(super) tile_chunk_index: TileChunkIndex,\n    pub(super) tile_occupancy_index: Option<Arc<TileOccupancyIndex>>,\n",
        1,
    )
    for field in [
        "manifest_probe_in_flight: bool",
        "manifest_probe_diagnostics: ManifestProbeDiagnostics",
        "manifest_scanned_tiles: BTreeSet<(i32, i32)>",
        "manifest_probe_cancel: Option<RenderTaskControl>",
        "manifest_probe_request_id: Option<u64>",
    ]:
        text = re.sub(rf"(?m)^\s*pub\(super\) {re.escape(field)},\n", "", text)
    write(path, text)


def patch_tile_state() -> None:
    path = "src/ui/window/map_viewer/tile_state.rs"
    text = read(path)
    text = text.replace("    PendingManifest,\n", "    Empty,\n", 1)
    text = text.replace("pub(super) pending_manifest: usize", "pub(super) empty: usize")
    text = text.replace("TileLoadState::PendingManifest", "TileLoadState::Empty")
    text = text.replace("self.pending_manifest", "self.empty")
    text = text.replace("removed.pending_manifest", "removed.empty")
    text = remove_named_functions(
        text,
        [
            "pending_manifest",
            "ensure_pending_manifest",
            "mark_manifest_ready",
            "has_pending_manifest_for_tiles",
            "is_pending_manifest",
            "pending_manifest_coords_with_priority",
            "pending_manifest_count",
        ],
    )
    mark_empty = r'''    pub(super) fn mark_empty(&mut self, coord: (i32, i32)) -> Option<Arc<RenderImage>> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let (previous_bytes, dropped_image) = if let Some(entry) = self.entries.get_mut(&coord) {
            let previous_bytes = tile_entry_loaded_estimated_bytes(entry);
            let dropped_image = tile_entry_take_render_image(entry);
            self.state_counts.transition(entry.state, TileLoadState::Empty);
            entry.state = TileLoadState::Empty;
            entry.source_status = TileSourceStatus::Fresh;
            entry.priority = TilePriority::Prefetch;
            entry.attempts = 0;
            entry.retry_after = None;
            entry.last_error = None;
            (previous_bytes, dropped_image)
        } else {
            let mut entry = TileEntry::queued(TilePriority::Prefetch, sequence);
            entry.state = TileLoadState::Empty;
            entry.source_status = TileSourceStatus::Fresh;
            self.state_counts.increment(entry.state);
            self.entries.insert(coord, entry);
            (0, None)
        };
        self.loaded_estimated_bytes = self.loaded_estimated_bytes.saturating_sub(previous_bytes);
        dropped_image
    }

    pub(super) fn empty_count(&self) -> usize {
        self.state_counts.empty
    }

'''
    marker = "    pub(super) fn mark_invalid(\n"
    if marker not in text:
        raise RuntimeError("mark_invalid insertion marker missing")
    text = text.replace(marker, mark_empty + marker, 1)
    text = text.replace(
        "if matches!(entry.state, TileLoadState::Invalid)\n            || entry.source_status == TileSourceStatus::Fresh",
        "if matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)\n            || entry.source_status == TileSourceStatus::Fresh",
    )
    text = text.replace(
        "matches!(entry.state, TileLoadState::Loaded | TileLoadState::Invalid)",
        "matches!(entry.state, TileLoadState::Loaded | TileLoadState::Empty | TileLoadState::Invalid)",
    )
    text = text.replace("Pending manifest entries are cheap", "Resolved empty entries are cheap")
    write(path, text)


def patch_lifecycle() -> None:
    path = "src/ui/window/map_viewer/lifecycle.rs"
    text = read(path)
    text = text.replace("use super::tile_manifest::*;", "use super::tile_occupancy::*;")
    text = text.replace("TileLoadState::PendingManifest", "TileLoadState::Empty")
    text = text.replace(
        "            tile_chunk_index: BTreeMap::new(),\n",
        "            tile_chunk_index: BTreeMap::new(),\n            tile_occupancy_index: None,\n",
        1,
    )
    for init in [
        "            manifest_probe_in_flight: false,\n",
        "            manifest_probe_diagnostics: ManifestProbeDiagnostics::default(),\n",
        "            manifest_scanned_tiles: BTreeSet::new(),\n",
        "            manifest_probe_cancel: None,\n",
        "            manifest_probe_request_id: None,\n",
    ]:
        text = text.replace(init, "")

    cancel_metadata = r'''    pub(super) fn cancel_metadata_scan(&mut self) {
        cancel_metadata_flag(&mut self.metadata_cancel);
        self.metadata_loading = false;
    }'''
    text = replace_named_function(text, "cancel_metadata_scan", cancel_metadata)
    text = remove_named_functions(text, ["cancel_manifest_probe_for_interaction"])

    refresh_metadata = r'''    pub(super) fn refresh_metadata(&mut self, cx: &mut Context<Self>) {
        self.cancel_metadata_scan();
        self.cancel_professional_overlay_query();
        self.cancel_slime_window_candidate_query();
        self.professional.village_index = None;
        self.professional.overlay_bounds = None;
        self.professional.overlays = None;
        self.professional.overlay_paint = None;
        self.professional.pending_overlay_refresh = true;
        self.professional.slime_window_candidates = None;
        self.metadata_generation = self.metadata_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_active_render();
        self.metadata_loading = true;
        Self::drop_render_images(self.tile_manager.clear(), cx);
        self.clear_canvas_tile_snapshot(cx);
        self.last_visible_tile_signature = None;
        self.tile_reveal_state = TileRevealState::default();
        self.metadata_index_ready = false;
        self.tile_occupancy_index = None;
        self.available_tiles.clear();
        self.tile_chunk_index.clear();
        self.chunk_bounds = None;
        self.diagnostics = RenderDiagnostics::default();
        self.render_stats = RenderPipelineStats::default();
        self.status = SharedString::from("正在构建全局区块占用索引...");
        tracing::debug!(
            generation = self.metadata_generation,
            dimension = ?self.dimension,
            world = %self.world_path.display(),
            "map_viewer occupancy_index_start"
        );
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let dimension = self.dimension;
        let layout = self.active_layout;
        let recenter = self.recenter_on_next_metadata || !self.viewport.initialized;
        self.recenter_on_next_metadata = false;
        let pending_center_block = self.pending_center_block.take();
        let metadata_cancel = RenderTaskControl::new();
        let metadata_cancel_for_task = metadata_cancel.clone();
        let metadata_cancel_for_owner = metadata_cancel.clone();
        self.metadata_cancel = Some(metadata_cancel);

        // Unknown center tiles are immediately eligible for direct rendering while
        // the one-time key-space scan runs in the background.
        self.ensure_visible_tiles(cx);

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    load_tile_occupancy_index(
                        world_path,
                        dimension,
                        layout,
                        metadata_cancel_for_task,
                    )
                })
                .await;
            let Some(view) = handle.upgrade() else {
                metadata_cancel_for_owner.cancel();
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation
                    || metadata_cancel_for_owner.is_cancelled()
                {
                    metadata_cancel_for_owner.cancel();
                    return;
                }
                this.metadata_cancel = None;
                this.metadata_loading = false;
                match result {
                    Ok(result) => {
                        let tile_count = result.index.tile_count();
                        let chunk_count = result.index.chunk_count();
                        this.chunk_bounds = result.index.bounds();
                        if let Some((block_x, block_z)) = pending_center_block {
                            this.viewport.center_on_block(block_x, block_z, layout);
                        } else if recenter
                            && let Some((block_x, block_z)) =
                                occupancy_center_block(result.index.as_ref())
                        {
                            this.viewport.center_on_block(block_x, block_z, layout);
                        }
                        this.tile_occupancy_index = Some(result.index);
                        this.metadata_index_ready = true;
                        this.status = SharedString::from(format!(
                            "全局占用索引就绪 · {tile_count} 个瓦片 · {chunk_count} 个 chunk · {}",
                            match result.source {
                                TileOccupancyIndexSource::DiskCache => "磁盘缓存命中",
                                TileOccupancyIndexSource::KeySpaceScan => "LevelDB 单次扫描",
                            }
                        ));
                        this.clear_visible_error();
                        tracing::debug!(
                            generation,
                            tile_count,
                            chunk_count,
                            source = ?result.source,
                            bounds = ?this.chunk_bounds,
                            "map_viewer occupancy_index_ready"
                        );
                    }
                    Err(error) => {
                        this.metadata_index_ready = false;
                        tracing::warn!(generation, %error, "map_viewer occupancy_index_failed");
                        this.show_map_error(SharedString::from(error), cx);
                    }
                }
                this.last_visible_tile_signature = None;
                this.ensure_visible_tiles(cx);
                this.refresh_metadata_consumers_if_ready(cx);
                let colors = this.theme_colors(cx);
                this.sync_canvas_snapshot(colors, cx);
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }'''
    text = replace_named_function(text, "refresh_metadata", refresh_metadata)

    # Edit changes invalidate both tile payload identity and the occupancy sidecar.
    edit_refresh = r'''    pub(super) fn queue_edit_refresh_tiles_after_session_refresh(
        &mut self,
        affected_tiles: &[(i32, i32)],
        _affected_chunks: &BTreeSet<ChunkPos>,
        _tile_priority: TilePriority,
        _reuse_known_tile_index: bool,
        cx: &mut Context<Self>,
    ) {
        if affected_tiles.is_empty() {
            return;
        }
        tracing::debug!(
            affected_tiles = affected_tiles.len(),
            "map_viewer edit_invalidated_occupancy_index"
        );
        self.refresh_metadata(cx);
    }'''
    text = replace_named_function(text, "queue_edit_refresh_tiles_after_session_refresh", edit_refresh)

    text = text.replace("        self.manifest_probe_in_flight = false;\n", "")
    text = text.replace("        self.manifest_scanned_tiles.clear();\n", "")
    text = text.replace("        self.tile_occupancy_index = None;\n        self.available_tiles.clear();", "        self.tile_occupancy_index = None;\n        self.available_tiles.clear();", 1)

    # Remove all viewport probe methods and helpers.
    text = remove_named_functions(
        text,
        [
            "schedule_tile_manifest_probe",
            "resolve_scanned_manifest_misses",
            "mark_manifest_tile_empty",
        ],
    )

    # Remove the edit-refresh probe scheduling block.
    text = re.sub(
        r"\n\s*let edit_refresh_tiles = self.*?self\.schedule_tile_manifest_probe\(&\[\], &edit_refresh_tiles, tile_plan\.center, cx\);\n\s*\}\n",
        "\n",
        text,
        count=1,
        flags=re.S,
    )

    # Resolve current viewport against the compact index before queueing work.
    insert_marker = "    pub(super) fn schedule_viewport_work_refresh(&mut self, cx: &mut Context<Self>) {"
    helper = r'''    fn resolve_occupancy_tiles(
        &mut self,
        coords: &[(i32, i32)],
        cx: &mut Context<Self>,
    ) -> Vec<(i32, i32)> {
        let Some(index) = self.tile_occupancy_index.as_ref().map(Arc::clone) else {
            return coords.to_vec();
        };
        let mut renderable = Vec::with_capacity(coords.len());
        for coord in coords {
            if let Some(positions) = materialize_occupancy_chunks(index.as_ref(), *coord) {
                self.available_tiles.insert(*coord);
                self.tile_chunk_index.insert(*coord, positions);
                renderable.push(*coord);
            } else {
                self.available_tiles.remove(coord);
                self.tile_chunk_index.remove(coord);
                Self::drop_render_image(self.tile_manager.mark_empty(*coord), cx);
            }
        }
        renderable
    }

'''
    if insert_marker not in text:
        raise RuntimeError("schedule_viewport_work_refresh marker missing")
    text = text.replace(insert_marker, helper + insert_marker, 1)

    # Replace the large visible/prefetch classification with occupancy materialization.
    pattern = re.compile(
        r"\n\s*let mut visible_renderable_tiles = Vec::new\(\);.*?\n\s*if deferred_visible_work \{",
        re.S,
    )
    replacement = r'''
        let visible_work_limit = visible_tile_foreground_work_limit(tile_plan.is_interacting);
        let visible_candidates = tile_plan
            .visible
            .iter()
            .copied()
            .filter(|coord| {
                visible_tile_needs_foreground_work(
                    &self.tile_chunk_index,
                    &self.tile_manager,
                    *coord,
                )
            })
            .take(visible_work_limit)
            .collect::<Vec<_>>();
        let deferred_visible_work = visible_candidates.len()
            < tile_plan
                .visible
                .iter()
                .filter(|coord| {
                    visible_tile_needs_foreground_work(
                        &self.tile_chunk_index,
                        &self.tile_manager,
                        **coord,
                    )
                })
                .count();
        let visible_renderable_tiles = self.resolve_occupancy_tiles(&visible_candidates, cx);
        self.tile_manager.ensure_tiles_for_layout(
            &visible_renderable_tiles,
            TilePriority::Visible,
            self.render_texture_layout,
        );
        if tile_plan.prefetch_radius > 0 {
            let prefetch_candidates = tile_plan
                .prefetch
                .iter()
                .copied()
                .filter(|coord| {
                    !tile_plan
                        .visible_bounds
                        .is_some_and(|bounds| tile_bounds_contains(bounds, *coord))
                })
                .collect::<Vec<_>>();
            let prefetch_renderable_tiles =
                self.resolve_occupancy_tiles(&prefetch_candidates, cx);
            self.tile_manager.ensure_tiles_for_layout(
                &prefetch_renderable_tiles,
                TilePriority::Prefetch,
                self.render_texture_layout,
            );
        }
        if deferred_visible_work {'''
    text, count = pattern.subn(replacement, text, count=1)
    if count != 1:
        raise RuntimeError(f"visible occupancy classification replacement count={count}")

    # Drag path uses the same occupancy resolver.
    drag_pattern = re.compile(
        r"\n\s*let mut visible_renderable_tiles = Vec::new\(\);\n\s*for coord in &visible_tiles \{.*?\n\s*self\.tile_manager\.ensure_tiles_for_layout\(",
        re.S,
    )
    drag_replacement = r'''
        let visible_renderable_tiles = self.resolve_occupancy_tiles(&visible_tiles, cx);
        self.tile_manager.ensure_tiles_for_layout('''
    text, count = drag_pattern.subn(drag_replacement, text, count=1)
    if count != 1:
        raise RuntimeError(f"drag occupancy classification replacement count={count}")

    text = text.replace(
        "    fn has_current_viewport_work_or_pending_manifest(&self) -> bool {",
        "    fn has_current_viewport_work(&self) -> bool {",
    )
    text = re.sub(
        r"\n\s*\|\| self\n\s*\.tile_manager\n\s*\.has_pending_manifest_for_tiles\(&visible_tiles\)",
        "",
        text,
    )
    text = text.replace("has_current_viewport_work_or_pending_manifest", "has_current_viewport_work")
    text = text.replace("pending_manifest_count()", "empty_count()")
    text = text.replace("pending_manifest", "empty")
    text = text.replace("manifest_probe", "occupancy_index")
    text = text.replace("manifest_load", "occupancy_load")
    text = text.replace("manifest", "occupancy")
    write(path, text)


def patch_auxiliary_files() -> None:
    # Remove interaction-only cancellation calls.
    path = "src/ui/window/map_viewer/interactions.rs"
    text = read(path)
    text = re.sub(r"(?m)^\s*self\.cancel_manifest_probe_for_interaction\(\);\n", "", text)
    write(path, text)

    path = "src/ui/window/map_viewer/view_stable.rs"
    text = read(path)
    text = remove_named_functions(text, ["prepare_visible_manifest_probe"])
    text = re.sub(r"(?m)^\s*this\.prepare_visible_manifest_probe\(&visible_tiles, cx\);\n", "", text)
    write(path, text)

    path = "src/ui/window/map_viewer/lifecycle_stable.rs"
    text = read(path)
    text = remove_named_functions(text, ["select_manifest_probe_tiles"])
    text = text.replace("TILE_MANIFEST_PROBE_BATCH_TILES", "0")
    write(path, text)

    path = "src/ui/window/map_viewer/helpers.rs"
    text = read(path)
    text = remove_named_functions(text, ["manifest_probe_worker_count"])
    write(path, text)

    path = "src/ui/window/map_viewer/viewport.rs"
    text = read(path)
    text = remove_named_functions(
        text,
        [
            "select_manifest_probe_tiles",
            "select_manifest_probe_tiles_from_ordered",
            "push_ordered_manifest_probe_tiles",
        ],
    )
    write(path, text)

    path = "src/ui/window/map_viewer/bottom_panel.rs"
    text = read(path)
    text = text.replace("self.metadata_loading || self.manifest_probe_in_flight", "self.metadata_loading")
    write(path, text)

    path = "src/ui/window/map_viewer/overlays.rs"
    text = read(path)
    text = text.replace("            self.manifest_probe_in_flight,\n", "")
    text = text.replace("    manifest_probe_in_flight: bool,\n", "")
    text = text.replace("manifest_probe_in_flight || render_batch_active || has_visible_work", "render_batch_active || has_visible_work")
    write(path, text)

    path = "src/ui/window/map_viewer/editor.rs"
    text = read(path)
    text = re.sub(r"(?m)^\s*self\.record_manifest_probe_edit\([^;]*;\n", "", text)
    text = remove_named_functions(text, ["record_manifest_probe_edit"])
    text = re.sub(r"(?m)^\s*self\.manifest_scanned_tiles\.remove\(coord\);\n", "", text)
    write(path, text)

    # Diagnostics panel block is isolated by the literal label.
    path = "src/ui/window/map_viewer/panels.rs"
    text = read(path)
    text = re.sub(
        r"\n\s*\.child\(.*?\"Probe 诊断.*?\n\s*\)\n",
        "\n",
        text,
        count=1,
        flags=re.S,
    )
    text = re.sub(r"(?m)^.*manifest_probe_diagnostics.*\n", "", text)
    write(path, text)

    path = "src/ui/window/map_viewer/tests.rs"
    text = read(path)
    text = text.replace("use super::tile_manifest::*;\n", "use super::tile_occupancy::*;\n")
    text = remove_tests_with_needles(
        text,
        (
            "PendingManifest",
            "pending_manifest",
            "manifest_probe",
            "TileManifest",
            "tile_manifest",
        ),
    )
    write(path, text)


def remove_old_files() -> None:
    for relative in [
        "src/ui/window/map_viewer/tile_manifest.rs",
        "src/ui/window/map_viewer/tile_manifest_legacy.rs",
    ]:
        path = ROOT / relative
        if path.exists():
            path.unlink()


def cleanup_leveldb_warning() -> None:
    path = "crates/bedrock-leveldb/src/db.rs"
    text = read(path)
    text = text.replace("CompressionPolicy, NativeCacheOptions, OpenOptions", "CompressionPolicy, OpenOptions")
    write(path, text)


def main() -> None:
    patch_renderer_cache()
    add_tile_occupancy_adapter()
    patch_modules_and_prelude()
    patch_model()
    patch_tile_state()
    patch_lifecycle()
    patch_auxiliary_files()
    remove_old_files()
    cleanup_leveldb_warning()


if __name__ == "__main__":
    main()

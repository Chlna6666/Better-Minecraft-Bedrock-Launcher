use super::model::*;
use super::prelude::*;
use super::query_cache::{MapQueryCacheKey, MapQueryKind};
use super::viewport::*;

impl MapViewerWindowView {
    pub(super) fn preload_entity_avatar_pool(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |handle, cx| {
            let avatars = cx
                .background_spawn(async move { load_generated_entity_avatars_rgba() })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                let mut changed = false;
                for (identifier, width, height, pixels) in avatars {
                    if this
                        .professional
                        .entity_avatar_pool
                        .contains_key(&identifier)
                    {
                        continue;
                    }
                    match RenderImage::from_raw_pixels(
                        width,
                        height,
                        RenderImagePixelFormat::Rgba8,
                        pixels,
                    ) {
                        Ok(image) => {
                            this.professional
                                .entity_avatar_pool
                                .insert(identifier, Arc::new(image));
                            changed = true;
                        }
                        Err(error) => {
                            tracing::debug!(
                                ?error,
                                "failed to create preloaded entity avatar image"
                            );
                        }
                    }
                }
                if changed {
                    this.sync_entity_avatar_overlay_snapshot(cx);
                }
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn sync_entity_avatar_overlay_snapshot(&mut self, cx: &mut Context<Self>) {
        if let Some(current) = self.professional.overlay_paint.as_ref() {
            let mut updated = (**current).clone();
            updated.bind_entity_avatars(&self.professional.entity_avatar_pool);
            self.professional.overlay_paint = Some(Arc::new(updated));
        }
        let colors = self.theme_colors(cx);
        if self.viewport_interaction_active() {
            self.sync_interaction_tile_layer_snapshot(colors, cx);
        } else {
            self.sync_canvas_snapshot(colors, cx);
        }
        cx.notify();
    }

    fn sync_professional_render_snapshot(&mut self, cx: &mut Context<Self>) {
        let colors = self.theme_colors(cx);
        if self.viewport_interaction_active() {
            self.sync_interaction_tile_layer_snapshot(colors, cx);
        } else {
            self.sync_canvas_snapshot(colors, cx);
        }
        cx.notify();
    }

    pub(super) fn refresh_professional_overlays(&mut self, cx: &mut Context<Self>) {
        let chunks_per_tile = u16::try_from(self.active_layout.chunks_per_tile)
            .unwrap_or(CHUNKS_PER_TILE as u16)
            .max(1);
        let Some(query_scope) = map_info_query_scope(
            self.metadata_index_ready,
            self.dimension,
            self.chunk_bounds,
            &self.available_tiles,
            self.visible_slime_bounds(),
            chunks_per_tile,
        ) else {
            return;
        };
        let bounds = query_scope.bounds;
        let options = self.professional_overlay_prefetch_options();
        let cache_key = overlay_query_cache_key(self, bounds);
        if let Some(cached) = self
            .map_query_budget
            .cached::<ProfessionalOverlayPaintCache>(cache_key)
        {
            if self.professional.overlay_loading {
                self.cancel_professional_overlay_query();
            }
            let changed = self.professional.overlay_bounds != Some(bounds)
                || self.professional.overlay_paint.is_none();
            if changed {
                let mut overlay = (*cached).clone();
                overlay.bind_entity_avatars(&self.professional.entity_avatar_pool);
                self.professional.overlay_paint = Some(Arc::new(overlay));
            }
            self.professional.overlay_bounds = Some(bounds);
            self.professional.pending_overlay_refresh = false;
            if changed {
                self.sync_professional_render_snapshot(cx);
            }
            return;
        }
        if should_defer_overlay_query_for_visible_tiles(
            self.manifest_probe_in_flight,
            self.render_batch_active,
            self.tile_manager.has_visible_work(),
        ) {
            self.professional.pending_overlay_refresh = true;
            return;
        }
        self.refresh_village_index_if_needed(cx);
        if self.drag.is_some() && !query_scope.indexed_world {
            self.professional.pending_overlay_refresh = true;
            return;
        }
        if self.professional.overlay_loading {
            if self.professional.last_overlay_request_bounds == Some(bounds)
                && self.professional.last_overlay_request_options == Some(options)
            {
                return;
            }
            self.cancel_professional_overlay_query();
        }
        if self.professional.overlay_bounds == Some(bounds)
            && self.professional.last_overlay_request_options == Some(options)
            && self.professional.overlay_paint.is_some()
        {
            self.professional.pending_overlay_refresh = false;
            return;
        }
        let cancel = CancelFlag::new();
        let overlay_generation = self.professional.overlay_generation.saturating_add(1);
        self.professional.overlay_generation = overlay_generation;
        let query_generation = self.map_query_budget.next_generation(MapQueryKind::Overlay);
        self.professional.overlay_cancel = Some(cancel.clone());
        self.professional.overlay_loading = true;
        self.professional.pending_overlay_refresh = false;
        self.professional.last_overlay_request_bounds = Some(bounds);
        self.professional.last_overlay_request_options = Some(options);
        self.status = SharedString::from("正在加载专业地图叠加层...");
        cx.notify();

        let metadata_generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let village_index = self.professional.village_index.clone();
        let query_budget = self.map_query_budget.clone();
        let max_workers = self.cpu_budget.thread_count();
        let tile_coordinates = query_scope.tile_coordinates;
        cx.spawn(async move |handle, cx| {
            let cached_world_path = world_path.clone();
            let cached_tile_coordinates = tile_coordinates.clone();
            let cached_cancel = cancel.clone();
            let cached_village_index = village_index.clone();
            let cached_result = {
                let _query_permit = query_budget.acquire().await;
                cx.background_spawn(async move {
                    let map_info = if tile_bound_map_info_requested(options) {
                        load_cached_map_info_tiles_blocking(
                            &cached_world_path,
                            bounds.dimension,
                            chunks_per_tile,
                            &cached_tile_coordinates,
                            &cached_cancel,
                        )
                        .map_err(|error| error.to_string())?
                    } else {
                        MapInfoOverlaySnapshot::default()
                    };
                    let mut villages = Vec::new();
                    if options.include_villages {
                        if let Some(index) = cached_village_index {
                            villages = index.query(bounds, options.max_items_per_kind);
                        }
                    }
                    Ok::<_, String>((map_info, villages))
                })
                .await
            };
            if let Some(view) = handle.upgrade() {
                view.update(cx, move |this, cx| {
                    if !this
                        .map_query_budget
                        .is_current(MapQueryKind::Overlay, query_generation)
                    {
                        return;
                    }
                    if !accept_overlay_result(
                        this.metadata_generation,
                        this.professional.overlay_generation,
                        this.professional.last_overlay_request_bounds,
                        this.professional.last_overlay_request_options,
                        metadata_generation,
                        overlay_generation,
                        bounds,
                        options,
                    ) {
                        return;
                    }
                    if let Ok((map_info, villages)) = cached_result {
                        if map_info.cached_tile_count > 0 || !villages.is_empty() {
                            this.professional.overlay_bounds = Some(bounds);
                            let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(
                                &map_info, &villages,
                            );
                            overlay.bind_entity_avatars(&this.professional.entity_avatar_pool);
                            let overlay = Arc::new(overlay);
                            this.professional.overlay_paint = Some(overlay);
                            this.status = SharedString::from(format!(
                                "已显示缓存叠加层 · 缓存 {} · 正在补齐未缓存区域",
                                map_info.cached_tile_count
                            ));
                        }
                    }
                    cx.notify();
                })?;
            }

            let full_result = {
                let _query_permit = query_budget.acquire().await;
                cx.background_spawn(async move {
                    let map_info = if tile_bound_map_info_requested(options) {
                        load_map_info_tiles_blocking(
                            &world_path,
                            bounds.dimension,
                            chunks_per_tile,
                            &tile_coordinates,
                            &cancel,
                            max_workers,
                        )
                        .map_err(|error| error.to_string())?
                    } else {
                        MapInfoOverlaySnapshot::default()
                    };
                    let mut villages = Vec::new();
                    if options.include_villages {
                        if let Some(index) = village_index {
                            villages = index.query(bounds, options.max_items_per_kind);
                        }
                    }
                    Ok::<_, String>((map_info, villages))
                })
                .await
            };
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if !this
                    .map_query_budget
                    .is_current(MapQueryKind::Overlay, query_generation)
                {
                    return;
                }
                if !accept_overlay_result(
                    this.metadata_generation,
                    this.professional.overlay_generation,
                    this.professional.last_overlay_request_bounds,
                    this.professional.last_overlay_request_options,
                    metadata_generation,
                    overlay_generation,
                    bounds,
                    options,
                ) {
                    return;
                }
                this.professional.overlay_loading = false;
                this.professional.overlay_cancel = None;
                match full_result {
                    Ok((map_info, villages)) => {
                        this.professional.overlay_bounds = Some(bounds);
                        let mut overlay = ProfessionalOverlayPaintCache::from_map_info_snapshot(
                            &map_info, &villages,
                        );
                        overlay.bind_entity_avatars(&this.professional.entity_avatar_pool);
                        let overlay = Arc::new(overlay);
                        this.map_query_budget.cache(cache_key, Arc::clone(&overlay));
                        this.professional.overlay_paint = Some(overlay);
                        this.professional.overlays = None;
                        this.status = SharedString::from(format!(
                            "专业地图叠加层已更新 · 缓存 {} · 重建 {}",
                            map_info.cached_tile_count, map_info.rebuilt_tile_count
                        ));
                    }
                    Err(error) => {
                        if error.contains("cancelled") || error.contains("cancel") {
                            this.status = SharedString::from("专业地图叠加层查询已取消");
                        } else {
                            this.status = SharedString::from(error);
                        }
                    }
                }
                if this.professional.pending_overlay_refresh {
                    this.professional.pending_overlay_refresh = false;
                    this.refresh_professional_overlays(cx);
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn refresh_village_index_if_needed(&mut self, cx: &mut Context<Self>) {
        if self.professional.village_index.is_some() || self.professional.village_index_loading {
            return;
        }
        let cache_key = village_query_cache_key(self);
        if let Some(cached) = self
            .map_query_budget
            .cached::<VillageOverlayIndex>(cache_key)
        {
            self.professional.village_index = Some(cached);
            return;
        }
        let cancel = CancelFlag::new();
        let generation = self.professional.village_index_generation.saturating_add(1);
        self.professional.village_index_generation = generation;
        let query_generation = self
            .map_query_budget
            .next_generation(MapQueryKind::VillageIndex);
        self.professional.village_index_loading = true;
        self.professional.village_index_cancel = Some(cancel.clone());
        let metadata_generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        self.status = SharedString::from("正在建立村庄索引...");
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    VillageOverlayIndex::build_blocking_with_control(&world, &cancel)
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if !this
                    .map_query_budget
                    .is_current(MapQueryKind::VillageIndex, query_generation)
                {
                    return;
                }
                if this.metadata_generation != metadata_generation
                    || this.professional.village_index_generation != generation
                {
                    return;
                }
                this.professional.village_index_loading = false;
                this.professional.village_index_cancel = None;
                match result {
                    Ok(index) => {
                        this.map_query_budget.cache(cache_key, Arc::clone(&index));
                        this.professional.village_index = Some(index);
                        this.professional.pending_overlay_refresh = true;
                        this.refresh_professional_overlays(cx);
                    }
                    Err(error) => {
                        if !error.contains("cancel") {
                            this.status = SharedString::from(error);
                        }
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn cancel_professional_overlay_query(&mut self) {
        if let Some(cancel) = self.professional.overlay_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = self.professional.village_index_cancel.take() {
            cancel.cancel();
        }
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.professional.village_index_generation =
            self.professional.village_index_generation.saturating_add(1);
        self.professional.overlay_loading = false;
        self.professional.village_index_loading = false;
        self.professional.last_overlay_request_bounds = None;
        self.professional.last_overlay_request_options = None;
        self.map_query_budget.next_generation(MapQueryKind::Overlay);
        self.map_query_budget
            .next_generation(MapQueryKind::VillageIndex);
    }

    pub(super) fn cancel_slime_window_candidate_query(&mut self) {
        if let Some(cancel) = self.professional.slime_window_candidates_cancel.take() {
            cancel.cancel();
        }
        self.professional.slime_window_candidates_generation = self
            .professional
            .slime_window_candidates_generation
            .saturating_add(1);
        self.professional.slime_window_candidates_loading = false;
        self.professional.slime_window_candidates_request_bounds = None;
        self.professional.slime_window_candidates_request_size = None;
        self.map_query_budget
            .next_generation(MapQueryKind::SlimeCandidates);
    }

    pub(super) fn invalidate_professional_overlay_for_viewport_change(&mut self) {
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

    pub(super) fn invalidate_map_info_cache_after_edit(
        &mut self,
        chunks: BTreeSet<ChunkPos>,
        cx: &mut Context<Self>,
    ) {
        let chunks_per_tile = u16::try_from(self.active_layout.chunks_per_tile)
            .unwrap_or(CHUNKS_PER_TILE as u16)
            .max(1);
        let generation = self
            .professional
            .map_info_invalidation_generation
            .saturating_add(1);
        self.professional.map_info_invalidation_generation = generation;
        self.professional.pending_overlay_refresh = true;
        self.professional.village_index = None;
        self.status = SharedString::from("正在作废已修改区块的地图信息缓存...");
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    invalidate_map_info_tiles_for_chunks(&world_path, chunks_per_tile, &chunks)
                        .map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.professional.map_info_invalidation_generation != generation {
                    return;
                }
                match result {
                    Ok(removed) => {
                        this.status = SharedString::from(format!(
                            "已作废 {removed} 个地图信息瓦片，正在更新叠加层..."
                        ));
                    }
                    Err(error) => {
                        this.status = SharedString::from(format!(
                            "地图信息缓存作废失败，将执行记录校验: {error}"
                        ));
                    }
                }
                this.refresh_professional_overlays(cx);
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn professional_overlay_query_options(&self) -> RegionOverlayQueryOptions {
        RegionOverlayQueryOptions {
            include_slime: self.overlay_options.slime_chunks,
            include_entities: self.overlay_options.entities,
            include_block_entities: self.overlay_options.block_entities,
            include_pending_ticks: self.overlay_options.pending_ticks,
            include_villages: self.overlay_options.villages,
            include_hardcoded_spawn_areas: self.overlay_options.hardcoded_spawn_areas,
            max_chunks: 4_096,
            max_items_per_kind: 10_000,
        }
    }

    pub(super) fn professional_overlay_prefetch_options(&self) -> RegionOverlayQueryOptions {
        RegionOverlayQueryOptions {
            include_slime: true,
            include_entities: true,
            include_block_entities: true,
            include_pending_ticks: true,
            include_villages: true,
            include_hardcoded_spawn_areas: true,
            max_chunks: 4_096,
            max_items_per_kind: 10_000,
        }
    }

    pub(super) fn refresh_professional_render_caches(&mut self, cx: &mut Context<Self>) {
        self.refresh_slime_overlay_run_cache(cx);
        self.refresh_slime_window_candidate_cache(cx);
    }

    pub(super) fn refresh_slime_overlay_run_cache(&mut self, cx: &mut Context<Self>) {
        let Some(bounds) = self.visible_slime_bounds() else {
            self.cancel_slime_overlay_run_query();
            return;
        };
        let cache_key = MapQueryCacheKey::new(
            MapQueryKind::SlimeRuns,
            &self.world_path,
            bounds.dimension.id(),
            (
                bounds.min_chunk_x,
                bounds.max_chunk_x,
                bounds.min_chunk_z,
                bounds.max_chunk_z,
            ),
            0,
        );
        if let Some(cached) = self
            .map_query_budget
            .cached::<SlimeOverlayRunCache>(cache_key)
        {
            if self.professional.slime_overlay_runs_loading {
                self.cancel_slime_overlay_run_query();
            }
            let changed = self
                .professional
                .slime_overlay_runs
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, &cached));
            self.professional.slime_overlay_runs = Some(cached);
            self.professional.slime_overlay_runs_loading = false;
            self.professional.slime_overlay_runs_request_bounds = None;
            if changed {
                self.sync_professional_render_snapshot(cx);
            }
            return;
        }

        if self.professional.slime_overlay_runs_loading {
            if self.professional.slime_overlay_runs_request_bounds == Some(bounds) {
                return;
            }
            self.cancel_slime_overlay_run_query();
        }

        let generation = self
            .professional
            .slime_overlay_runs_generation
            .saturating_add(1);
        self.professional.slime_overlay_runs_generation = generation;
        let query_generation = self
            .map_query_budget
            .next_generation(MapQueryKind::SlimeRuns);
        let cancel = CancelFlag::new();
        let cancel_for_task = cancel.clone();
        let query_budget = self.map_query_budget.clone();
        self.professional.slime_overlay_runs_cancel = Some(cancel);
        self.professional.slime_overlay_runs_loading = true;
        self.professional.slime_overlay_runs_request_bounds = Some(bounds);
        let metadata_generation = self.metadata_generation;
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    if cancel_for_task.is_cancelled() {
                        return None;
                    }
                    SlimeOverlayRunCache::build(bounds).map(Arc::new)
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if !this
                    .map_query_budget
                    .is_current(MapQueryKind::SlimeRuns, query_generation)
                    || this.metadata_generation != metadata_generation
                    || this.professional.slime_overlay_runs_generation != generation
                    || this.visible_slime_bounds() != Some(bounds)
                {
                    return;
                }
                this.professional.slime_overlay_runs_loading = false;
                this.professional.slime_overlay_runs_cancel = None;
                this.professional.slime_overlay_runs_request_bounds = None;
                if let Some(cache) = result {
                    this.map_query_budget.cache(cache_key, Arc::clone(&cache));
                    this.professional.slime_overlay_runs = Some(cache);
                    let colors = this.theme_colors(cx);
                    if this.viewport_interaction_active() {
                        this.sync_interaction_tile_layer_snapshot(colors, cx);
                    } else {
                        this.sync_canvas_snapshot(colors, cx);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn cancel_slime_overlay_run_query(&mut self) {
        if let Some(cancel) = self.professional.slime_overlay_runs_cancel.take() {
            cancel.cancel();
        }
        self.professional.slime_overlay_runs_generation = self
            .professional
            .slime_overlay_runs_generation
            .saturating_add(1);
        self.professional.slime_overlay_runs_loading = false;
        self.professional.slime_overlay_runs_request_bounds = None;
        self.map_query_budget
            .next_generation(MapQueryKind::SlimeRuns);
    }

    pub(super) fn refresh_slime_window_candidate_cache(&mut self, cx: &mut Context<Self>) {
        let Some(bounds) = self.professional_query_bounds() else {
            self.cancel_slime_window_candidate_query();
            self.professional.slime_window_candidates = None;
            return;
        };
        if bounds.dimension != Dimension::Overworld || bounds.chunk_count() > 20_000 {
            self.cancel_slime_window_candidate_query();
            self.professional.slime_window_candidates = None;
            return;
        }
        let requested_size = self.slime_query_window_size;
        if self
            .professional
            .slime_window_candidates
            .as_ref()
            .is_some_and(|cache| cache.bounds == bounds && cache.size == requested_size)
        {
            return;
        }
        if self.professional.slime_window_candidates_loading {
            if self.professional.slime_window_candidates_request_bounds == Some(bounds)
                && self.professional.slime_window_candidates_request_size == Some(requested_size)
            {
                return;
            }
            self.cancel_slime_window_candidate_query();
        }
        let Some(size) = SlimeWindowSize::new(requested_size.value()).ok() else {
            self.cancel_slime_window_candidate_query();
            self.professional.slime_window_candidates = None;
            return;
        };
        let cache_key = MapQueryCacheKey::new(
            MapQueryKind::SlimeCandidates,
            &self.world_path,
            bounds.dimension.id(),
            (
                bounds.min_chunk_x,
                bounds.max_chunk_x,
                bounds.min_chunk_z,
                bounds.max_chunk_z,
            ),
            requested_size.value() as u64,
        );
        if let Some(cached) = self
            .map_query_budget
            .cached::<SlimeWindowCandidateCache>(cache_key)
        {
            if self.professional.slime_window_candidates_loading {
                self.cancel_slime_window_candidate_query();
            }
            let changed = self.professional.slime_window_candidates.as_ref() != Some(&*cached);
            self.professional.slime_window_candidates = Some((*cached).clone());
            self.professional.slime_window_candidates_loading = false;
            self.professional.slime_window_candidates_cancel = None;
            self.professional.slime_window_candidates_request_bounds = None;
            self.professional.slime_window_candidates_request_size = None;
            if changed {
                self.sync_professional_render_snapshot(cx);
            }
            return;
        }
        self.cancel_slime_window_candidate_query();
        self.professional.slime_window_candidates = None;
        let generation = self
            .professional
            .slime_window_candidates_generation
            .saturating_add(1);
        self.professional.slime_window_candidates_generation = generation;
        let query_generation = self
            .map_query_budget
            .next_generation(MapQueryKind::SlimeCandidates);
        let cancel = CancelFlag::new();
        self.professional.slime_window_candidates_cancel = Some(cancel.clone());
        self.professional.slime_window_candidates_loading = true;
        self.professional.slime_window_candidates_request_bounds = Some(bounds);
        self.professional.slime_window_candidates_request_size = Some(requested_size);
        let metadata_generation = self.metadata_generation;
        let cancel_for_task = cancel.clone();
        let query_budget = self.map_query_budget.clone();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    if cancel_for_task.is_cancelled() {
                        return Err("slime window query cancelled".to_string());
                    }
                    query_slime_chunk_windows(bounds, size, 3).map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if !this
                    .map_query_budget
                    .is_current(MapQueryKind::SlimeCandidates, query_generation)
                {
                    return;
                }
                if !accept_slime_window_candidate_result(
                    this.metadata_generation,
                    this.professional.slime_window_candidates_generation,
                    this.professional_query_bounds(),
                    this.slime_query_window_size,
                    metadata_generation,
                    generation,
                    bounds,
                    requested_size,
                ) {
                    return;
                }
                this.professional.slime_window_candidates_loading = false;
                this.professional.slime_window_candidates_cancel = None;
                this.professional.slime_window_candidates_request_bounds = None;
                this.professional.slime_window_candidates_request_size = None;
                match result {
                    Ok(windows) => {
                        let cache = Arc::new(SlimeWindowCandidateCache {
                            bounds,
                            size: requested_size,
                            windows,
                        });
                        this.map_query_budget.cache(cache_key, Arc::clone(&cache));
                        this.professional.slime_window_candidates = Some((*cache).clone());
                    }
                    Err(error) => {
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn visible_slime_bounds(&self) -> Option<SlimeChunkBounds> {
        let range = region_render_range_for_viewport(self.viewport, self.active_layout)?;
        Some(SlimeChunkBounds {
            dimension: self.dimension,
            min_chunk_x: range.min_chunk_x,
            max_chunk_x: range.max_chunk_x,
            min_chunk_z: range.min_chunk_z,
            max_chunk_z: range.max_chunk_z,
        })
    }

    pub(super) fn professional_query_bounds(&self) -> Option<SlimeChunkBounds> {
        self.professional
            .selection
            .map(ChunkSelection::bounds)
            .or_else(|| self.visible_slime_bounds())
    }
}

fn tile_bound_map_info_requested(options: RegionOverlayQueryOptions) -> bool {
    options.include_entities
        || options.include_block_entities
        || options.include_pending_ticks
        || options.include_hardcoded_spawn_areas
}

pub(super) const fn should_defer_overlay_query_for_visible_tiles(
    manifest_probe_in_flight: bool,
    render_batch_active: bool,
    has_visible_work: bool,
) -> bool {
    manifest_probe_in_flight || render_batch_active || has_visible_work
}

fn overlay_query_cache_key(
    view: &MapViewerWindowView,
    bounds: SlimeChunkBounds,
) -> MapQueryCacheKey {
    MapQueryCacheKey::new(
        MapQueryKind::Overlay,
        &view.world_path,
        bounds.dimension.id(),
        (
            bounds.min_chunk_x,
            bounds.max_chunk_x,
            bounds.min_chunk_z,
            bounds.max_chunk_z,
        ),
        view.professional
            .map_info_invalidation_generation
            .wrapping_add(view.professional.village_index_generation.rotate_left(32)),
    )
}

fn village_query_cache_key(view: &MapViewerWindowView) -> MapQueryCacheKey {
    MapQueryCacheKey::new(
        MapQueryKind::VillageIndex,
        &view.world_path,
        view.dimension.id(),
        (0, 0, 0, 0),
        view.professional.map_info_invalidation_generation,
    )
}

fn map_info_tile_coordinates(bounds: SlimeChunkBounds, chunks_per_tile: u16) -> Vec<(i32, i32)> {
    let edge = i32::from(chunks_per_tile).max(1);
    let min_tile_x = bounds.min_chunk_x.div_euclid(edge);
    let max_tile_x = bounds.max_chunk_x.div_euclid(edge);
    let min_tile_z = bounds.min_chunk_z.div_euclid(edge);
    let max_tile_z = bounds.max_chunk_z.div_euclid(edge);
    let mut tiles = Vec::new();
    for tile_z in min_tile_z..=max_tile_z {
        for tile_x in min_tile_x..=max_tile_x {
            tiles.push((tile_x, tile_z));
        }
    }
    tiles
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct MapInfoQueryScope {
    pub(super) bounds: SlimeChunkBounds,
    pub(super) tile_coordinates: Vec<(i32, i32)>,
    pub(super) indexed_world: bool,
}

pub(super) fn map_info_query_scope(
    metadata_index_ready: bool,
    dimension: Dimension,
    chunk_bounds: Option<ChunkBounds>,
    available_tiles: &BTreeSet<(i32, i32)>,
    visible_bounds: Option<SlimeChunkBounds>,
    chunks_per_tile: u16,
) -> Option<MapInfoQueryScope> {
    if metadata_index_ready
        && !available_tiles.is_empty()
        && let Some(chunk_bounds) = chunk_bounds
        && chunk_bounds.dimension == dimension
    {
        return Some(MapInfoQueryScope {
            bounds: SlimeChunkBounds {
                dimension,
                min_chunk_x: chunk_bounds.min_chunk_x,
                max_chunk_x: chunk_bounds.max_chunk_x,
                min_chunk_z: chunk_bounds.min_chunk_z,
                max_chunk_z: chunk_bounds.max_chunk_z,
            },
            tile_coordinates: available_tiles.iter().copied().collect(),
            indexed_world: true,
        });
    }

    let bounds = visible_bounds?;
    Some(MapInfoQueryScope {
        bounds,
        tile_coordinates: map_info_tile_coordinates(bounds, chunks_per_tile),
        indexed_world: false,
    })
}

pub(super) fn accept_overlay_result(
    current_metadata_generation: u64,
    current_overlay_generation: u64,
    current_bounds: Option<SlimeChunkBounds>,
    current_options: Option<RegionOverlayQueryOptions>,
    result_metadata_generation: u64,
    result_overlay_generation: u64,
    result_bounds: SlimeChunkBounds,
    result_options: RegionOverlayQueryOptions,
) -> bool {
    current_metadata_generation == result_metadata_generation
        && current_overlay_generation == result_overlay_generation
        && current_bounds == Some(result_bounds)
        && current_options == Some(result_options)
}

pub(super) fn accept_slime_window_candidate_result(
    current_metadata_generation: u64,
    current_generation: u64,
    current_bounds: Option<SlimeChunkBounds>,
    current_size: SlimeQueryWindowSize,
    result_metadata_generation: u64,
    result_generation: u64,
    result_bounds: SlimeChunkBounds,
    result_size: SlimeQueryWindowSize,
) -> bool {
    current_metadata_generation == result_metadata_generation
        && current_generation == result_generation
        && current_bounds == Some(result_bounds)
        && current_size == result_size
}

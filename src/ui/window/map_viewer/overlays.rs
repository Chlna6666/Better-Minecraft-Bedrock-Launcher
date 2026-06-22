use super::model::*;
use super::prelude::*;
use super::viewport::*;

impl MapViewerWindowView {
    pub(super) fn refresh_professional_overlays(&mut self, cx: &mut Context<Self>) {
        if !self.has_database_professional_overlay() {
            self.cancel_professional_overlay_query();
            self.professional.overlay_bounds = None;
            self.professional.overlays = None;
            self.professional.overlay_paint = None;
            return;
        }
        let Some(bounds) = self.visible_slime_bounds() else {
            return;
        };
        let options = self.professional_overlay_query_options();
        if options.include_villages {
            self.refresh_village_index_if_needed(cx);
        }
        if self.drag.is_some() {
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
            && self.professional.overlays.is_some()
        {
            self.professional.pending_overlay_refresh = false;
            return;
        }
        let cancel = CancelFlag::new();
        let overlay_generation = self.professional.overlay_generation.saturating_add(1);
        self.professional.overlay_generation = overlay_generation;
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
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let mut query_options = options;
                    query_options.include_villages = false;
                    let mut overlays = query_region_overlays_blocking_with_control(
                        &world,
                        bounds,
                        query_options,
                        &cancel,
                    )
                    .map_err(|error| error.to_string())?;
                    if options.include_villages {
                        if let Some(index) = village_index {
                            overlays.villages = index.query(bounds, options.max_items_per_kind);
                        }
                    }
                    Ok::<_, String>(overlays)
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
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
                match result {
                    Ok(overlays) => {
                        this.professional.overlay_bounds = Some(overlays.bounds);
                        this.professional.overlay_paint = Some(Arc::new(
                            ProfessionalOverlayPaintCache::from_query(&overlays),
                        ));
                        this.professional.overlays = Some(overlays);
                        this.status = SharedString::from("专业地图叠加层已更新");
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
        if !self.overlay_options.villages
            || self.professional.village_index.is_some()
            || self.professional.village_index_loading
        {
            return;
        }
        let cancel = CancelFlag::new();
        let generation = self.professional.village_index_generation.saturating_add(1);
        self.professional.village_index_generation = generation;
        self.professional.village_index_loading = true;
        self.professional.village_index_cancel = Some(cancel.clone());
        let metadata_generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        self.status = SharedString::from("正在建立村庄索引...");
        cx.notify();
        cx.spawn(async move |handle, cx| {
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
                if this.metadata_generation != metadata_generation
                    || this.professional.village_index_generation != generation
                {
                    return;
                }
                this.professional.village_index_loading = false;
                this.professional.village_index_cancel = None;
                match result {
                    Ok(index) => {
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
    }

    pub(super) fn has_database_professional_overlay(&self) -> bool {
        self.overlay_options.entities
            || self.overlay_options.block_entities
            || self.overlay_options.villages
            || self.overlay_options.hardcoded_spawn_areas
    }

    pub(super) fn professional_overlay_query_options(&self) -> RegionOverlayQueryOptions {
        RegionOverlayQueryOptions {
            include_slime: self.overlay_options.slime_chunks,
            include_entities: self.overlay_options.entities,
            include_block_entities: self.overlay_options.block_entities,
            include_villages: self.overlay_options.villages,
            include_hardcoded_spawn_areas: self.overlay_options.hardcoded_spawn_areas,
            max_chunks: 4_096,
            max_items_per_kind: 2_000,
        }
    }

    pub(super) fn refresh_professional_render_caches(&mut self) {
        self.refresh_slime_overlay_run_cache();
        self.refresh_slime_window_candidate_cache();
    }

    pub(super) fn refresh_slime_overlay_run_cache(&mut self) {
        self.professional.slime_overlay_runs = self
            .visible_slime_bounds()
            .and_then(SlimeOverlayRunCache::build)
            .map(Arc::new);
    }

    pub(super) fn refresh_slime_window_candidate_cache(&mut self) {
        let Some(bounds) = self.professional_query_bounds() else {
            self.professional.slime_window_candidates = None;
            return;
        };
        if bounds.dimension != Dimension::Overworld || bounds.chunk_count() > 20_000 {
            self.professional.slime_window_candidates = None;
            return;
        }
        let Ok(size) = SlimeWindowSize::new(self.slime_query_window_size.value()) else {
            self.professional.slime_window_candidates = None;
            return;
        };
        let windows = query_slime_chunk_windows(bounds, size, 3).unwrap_or_default();
        self.professional.slime_window_candidates = Some(SlimeWindowCandidateCache {
            bounds,
            size: self.slime_query_window_size,
            windows,
        });
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

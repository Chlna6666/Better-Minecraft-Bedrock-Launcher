use super::helpers::*;
use super::model::*;
use super::prelude::*;
use super::tile_cache::decoded_tile_byte_len;
use super::tile_manifest::*;
use super::tile_render::*;
use super::tile_state::*;
use super::viewport::*;

impl MapViewerWindowView {
    fn delay_render_image_drop(&mut self, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        let was_empty = self.pending_render_image_evictions.is_empty();
        self.pending_render_image_evictions
            .push((Instant::now() + DRAG_RENDER_IMAGE_EVICTION_DELAY, image));
        if was_empty {
            self.schedule_pending_render_image_evictions(cx);
        }
    }

    fn schedule_pending_render_image_evictions(&mut self, cx: &mut Context<Self>) {
        let Some(next_ready_at) = self
            .pending_render_image_evictions
            .iter()
            .map(|(ready_at, _)| *ready_at)
            .min()
        else {
            return;
        };
        self.pending_render_image_eviction_generation = self
            .pending_render_image_eviction_generation
            .saturating_add(1);
        let generation = self.pending_render_image_eviction_generation;
        let delay = next_ready_at.saturating_duration_since(Instant::now());
        cx.spawn(async move |handle, cx| {
            if !delay.is_zero() {
                Timer::after(delay).await;
            }
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.pending_render_image_eviction_generation != generation {
                    return;
                }
                this.flush_pending_render_image_evictions(cx);
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn flush_pending_render_image_evictions(&mut self, cx: &mut Context<Self>) {
        if self.pending_render_image_evictions.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut remaining = Vec::new();
        let mut dropped_count = 0usize;
        for (ready_at, image) in std::mem::take(&mut self.pending_render_image_evictions) {
            if ready_at <= now && dropped_count < DRAG_RENDER_IMAGE_EVICTION_FLUSH_LIMIT {
                cx.drop_image(image, None);
                dropped_count = dropped_count.saturating_add(1);
            } else {
                remaining.push((ready_at, image));
            }
        }
        self.pending_render_image_evictions = remaining;
        if !self.pending_render_image_evictions.is_empty() {
            self.schedule_pending_render_image_evictions(cx);
        }
    }

    pub(super) fn clear_pending_render_image_evictions(&mut self, cx: &mut Context<Self>) {
        for (_, image) in self.pending_render_image_evictions.drain(..) {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn drop_render_images(
        images: impl IntoIterator<Item = Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) {
        for image in images {
            cx.drop_image(image, None);
        }
    }

    pub(super) fn drop_render_image(image: Option<Arc<RenderImage>>, cx: &mut Context<Self>) {
        if let Some(image) = image {
            cx.drop_image(image, None);
        }
    }

    fn drop_render_image_unless_current_tile(
        &self,
        coord: (i32, i32),
        image: Arc<RenderImage>,
        cx: &mut Context<Self>,
    ) {
        let is_current_tile_image = self
            .tile_manager
            .entries
            .get(&coord)
            .and_then(|entry| entry.image.as_ref())
            .is_some_and(|current| Arc::ptr_eq(&current.image, &image));
        if !is_current_tile_image {
            cx.drop_image(image, None);
        }
    }

    pub fn new(init: MapViewerWindowInit, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let world_path = PathBuf::from(init.world_path.as_ref());
        let window_size = window.viewport_size();
        let mut viewport = MapViewport::new(window_size);
        if let Some((spawn_x, spawn_z)) = spawn_block_center(&world_path) {
            viewport.center_on_block(spawn_x, spawn_z, web_relief_render_layout());
        }
        let input_fields = MapInputFields::new(window, cx);
        let editor_state = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor
        });
        let mut subscriptions = vec![cx.observe_window_bounds(window, |this, window, cx| {
            if this.update_viewport_size(window) {
                this.ensure_visible_tiles(cx);
                this.refresh_professional_render_caches();
                this.refresh_professional_overlays(cx);
                cx.notify();
            }
        })];
        subscriptions.extend(map_input_subscriptions(&input_fields, cx));
        let top_bar_view = cx.new(|_cx| MapTopBarView::default());
        let tool_stripe_view = cx.new(|_cx| MapToolStripeView::default());
        let menu_overlay_view = cx.new(|_cx| MapMenuOverlayView::default());
        let map_focus_handle = cx.focus_handle().tab_stop(true);
        map_focus_handle.focus(window);
        let canvas_view = cx.new({
            let map_focus_handle = map_focus_handle.clone();
            |cx| MapCanvasView::new(map_focus_handle, cx)
        });
        subscriptions.push(cx.subscribe(
            &editor_state,
            |this, editor, event: &CodeEditorEvent, cx| {
                this.handle_editor_event(editor, event, cx);
            },
        ));
        subscriptions.push(cx.subscribe(
            &top_bar_view,
            |this, _top_bar, action: &MapViewerAction, cx| {
                this.handle_action(*action, cx);
            },
        ));
        subscriptions.push(cx.subscribe(
            &tool_stripe_view,
            |this, _tool_stripe, action: &MapViewerAction, cx| {
                this.handle_action(*action, cx);
            },
        ));
        subscriptions.push(cx.subscribe(
            &menu_overlay_view,
            |this, _menu_overlay, action: &MapViewerAction, cx| {
                this.handle_action(*action, cx);
            },
        ));
        subscriptions.push(cx.subscribe(
            &canvas_view,
            |this, _canvas, action: &MapCanvasAction, cx| {
                this.handle_canvas_action(*action, cx);
            },
        ));
        let mut this = Self {
            version: init.version,
            asset: init.asset,
            world_path,
            mode: viewer_mode_from_render_mode(init.initial_mode),
            dimension: Dimension::Overworld,
            custom_dimension_id: 0,
            y_layer: 64,
            active_layout: web_relief_render_layout(),
            viewport,
            window_width: window_size.width / px(1.0),
            window_height: window_size.height / px(1.0),
            cpu_budget: RenderCpuBudget::default(),
            render_backend: default_interactive_render_backend(),
            render_gpu_backend: default_interactive_render_gpu_backend(),
            overlay_options: OverlayOptions::default(),
            slime_query_window_size: SlimeQueryWindowSize::default(),
            professional: ProfessionalQueryState::default(),
            history: MapHistoryState::default(),
            players: PlayerPanelState::default(),
            preview_3d: Preview3dState::default(),
            map_focus_handle,
            preview_3d_focus_handle: cx.focus_handle().tab_stop(true),
            edit_toast_id: None,
            toolbar_state: ToolbarState::default(),
            input_fields,
            ui_state: MapViewerUiState::default(),
            top_bar_view,
            tool_stripe_view,
            menu_overlay_view,
            canvas_view,
            editor_document: EditorDocument::default(),
            editor_state,
            db_tree: DbTreeState::default(),
            task_snapshots: task_manager::snapshot_arcs_map(),
            task_updates_task: None,
            frame_stats: FrameStats::default(),
            tile_reveal_state: TileRevealState::default(),
            available_tiles: BTreeSet::new(),
            tile_chunk_index: BTreeMap::new(),
            chunk_bounds: None,
            tile_manager: RegionManager::default(),
            canvas_tile_snapshot: Arc::new(TilePaintSnapshot::default()),
            canvas_tile_generation: 0,
            paste_preview_images: Arc::new(Vec::new()),
            paste_preview_images_generation: 0,
            last_synced_canvas_snapshot_key: None,
            last_synced_tile_layer_snapshot_key: None,
            render_session: None,
            markers: BTreeMap::new(),
            markers_generation: 0,
            context_menu: None,
            drag: None,
            right_selection_drag: None,
            hover_block_x: 0,
            hover_block_z: 0,
            recenter_on_next_metadata: true,
            pending_center_block: None,
            bypass_cache_active: false,
            metadata_loading: false,
            metadata_index_ready: false,
            manifest_probe_in_flight: false,
            manifest_probe_diagnostics: ManifestProbeDiagnostics::default(),
            manifest_scanned_tiles: BTreeSet::new(),
            session_loading: false,
            render_batch_active: false,
            request_id: 0,
            metadata_generation: 0,
            session_generation: 0,
            render_generation: 0,
            metadata_cancel: None,
            manifest_probe_cancel: None,
            render_cancels: BTreeMap::new(),
            active_render_tiles: BTreeSet::new(),
            active_render_center_tile: None,
            pending_viewport_refresh: false,
            viewport_idle_generation: 0,
            last_viewport_tile_sync: None,
            last_drag_canvas_snapshot_sync: None,
            last_visible_tile_log: None,
            last_tile_memory_trim: None,
            pending_render_image_evictions: Vec::new(),
            pending_render_image_eviction_generation: 0,
            last_visible_tile_signature: None,
            last_ready_status_update: None,
            status: SharedString::from("正在扫描地图瓦片..."),
            diagnostics: RenderDiagnostics::default(),
            render_stats: RenderPipelineStats::default(),
            refresh_rendered_tiles: 0,
            partial_refreshed_chunks: 0,
            cold_rendered_tiles: 0,
            last_queue_distance_squared: 0,
            last_visible_error: None,
            _subscriptions: subscriptions,
        };
        this.spawn_task_updates(cx);
        this.update_viewport_size(window);
        this.refresh_render_session(cx);
        this
    }

    pub(super) fn theme_colors(&self, cx: &App) -> ThemeColors {
        let theme = cx.global::<ThemeState>();
        lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(Instant::now()),
            theme.accent,
        )
    }

    pub(super) fn update_viewport_size(&mut self, window: &Window) -> bool {
        let window_size = window.viewport_size();
        self.window_width = window_size.width / px(1.0);
        self.window_height = window_size.height / px(1.0);
        self.ui_state
            .clamp_sizes(self.window_width, self.window_height);
        let changed = self.viewport.set_size(self.center_stage_size(window_size));
        changed
    }

    pub(super) fn center_stage_size(&self, window_size: Size<Pixels>) -> Size<Pixels> {
        center_stage_rect_for_layout(
            window_size.width / px(1.0),
            window_size.height / px(1.0),
            self.ui_state.left_panel_open,
            self.ui_state.right_panel_open,
            self.ui_state.right_panel_width,
            self.ui_state.bottom_panel_open,
            self.ui_state.bottom_panel_height,
            MIN_CENTER_WIDTH,
            MIN_CENTER_HEIGHT,
        )
        .size
    }

    pub(super) fn center_stage_origin(&self) -> Point<Pixels> {
        center_stage_rect_for_layout(
            self.window_width,
            self.window_height,
            self.ui_state.left_panel_open,
            self.ui_state.right_panel_open,
            self.ui_state.right_panel_width,
            self.ui_state.bottom_panel_open,
            self.ui_state.bottom_panel_height,
            MIN_CENTER_WIDTH,
            MIN_CENTER_HEIGHT,
        )
        .origin
    }

    pub(super) fn stage_local_position(&self, window_position: Point<Pixels>) -> Point<Pixels> {
        let origin = self.center_stage_origin();
        clamp_stage_position(
            point(window_position.x - origin.x, window_position.y - origin.y),
            self.viewport.width,
            self.viewport.height,
        )
    }

    pub(super) fn sync_canvas_snapshot(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        self.refresh_canvas_tiles_for_current_viewport_if_needed(cx);
        let snapshot_key = self.canvas_snapshot_key(colors);
        if self.last_synced_canvas_snapshot_key.as_ref() == Some(&snapshot_key) {
            return;
        }
        self.last_synced_canvas_snapshot_key = Some(snapshot_key);
        self.last_synced_tile_layer_snapshot_key = Some(self.tile_layer_snapshot_key(colors));
        let snapshot = self.canvas_snapshot(colors);
        let canvas_view = self.canvas_view.clone();
        canvas_view.update(cx, |view, cx| view.set_snapshot(snapshot, cx));
        self.record_memory_snapshot();
    }

    pub(super) fn sync_tile_layer_snapshot(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        self.refresh_canvas_tiles_for_current_viewport_if_needed(cx);
        let snapshot_key = self.tile_layer_snapshot_key(colors);
        if self.last_synced_tile_layer_snapshot_key.as_ref() == Some(&snapshot_key) {
            return;
        }
        self.last_synced_tile_layer_snapshot_key = Some(snapshot_key);
        let canvas_view = self.canvas_view.clone();
        let viewport = self.viewport;
        let layout = self.active_layout;
        let tiles = self.canvas_tile_snapshot.clone();
        canvas_view.update(cx, |view, cx| {
            view.set_tile_snapshot(viewport, layout, colors, tiles, cx)
        });
        self.record_memory_snapshot();
    }

    fn current_canvas_paint_bounds(&self) -> Option<TileBounds> {
        visible_tile_bounds_for_viewport(
            self.viewport,
            self.active_layout,
            self.viewport.center_tile(self.active_layout),
        )
    }

    fn refresh_canvas_tiles_for_current_viewport_if_needed(&mut self, cx: &mut Context<Self>) {
        let paint_bounds = self.current_canvas_paint_bounds();
        if self.canvas_tile_snapshot.paint_bounds == paint_bounds {
            return;
        }
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.canvas_tile_snapshot = Arc::new(build_tile_paint_snapshot(
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            self.toolbar_state.diagnostics_open,
            self.canvas_tile_generation,
        ));
        self.rebuild_paste_preview_images(cx);
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
    }

    pub(super) fn clear_canvas_tile_snapshot(&mut self) {
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.canvas_tile_snapshot = Arc::new(TilePaintSnapshot {
            generation: self.canvas_tile_generation,
            ..TilePaintSnapshot::default()
        });
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
    }

    pub(super) fn record_memory_snapshot(&self) {
        let canvas_snapshot_bytes = self.canvas_tile_snapshot.estimated_bytes;
        let paste_preview_bytes = self
            .paste_preview_images
            .iter()
            .map(|image| decoded_tile_byte_len(image.width, image.height).unwrap_or(0))
            .sum::<usize>();
        let copied_import_preview_bytes = self
            .professional
            .copied_chunk_preview_images
            .values()
            .map(|image| decoded_tile_byte_len(image.width, image.height).unwrap_or(0))
            .sum::<usize>();
        crate::utils::memory_diagnostics::record_map_viewer_memory(
            crate::utils::memory_diagnostics::MapViewerMemorySnapshot {
                tile_bytes: self.tile_manager.loaded_estimated_bytes(),
                tile_count: self.tile_manager.loaded_count(),
                canvas_snapshot_bytes,
                canvas_snapshot_tile_count: self.canvas_tile_snapshot.tiles.len(),
                paste_preview_bytes,
                paste_preview_count: self.paste_preview_images.len(),
                copied_import_preview_bytes,
                copied_import_preview_count: self.professional.copied_chunk_preview_images.len(),
                preview_3d_mesh_bytes: self
                    .preview_3d
                    .mesh
                    .as_ref()
                    .map_or(0, |mesh| mesh.estimated_cpu_bytes()),
                preview_3d_surface_bytes: self.preview_3d.estimated_surface_bytes(),
                preview_3d_chunk_mesh_count: self
                    .preview_3d
                    .mesh
                    .as_ref()
                    .map_or(0, |mesh| mesh.chunk_mesh_count()),
                preview_3d_vertex_count: self
                    .preview_3d
                    .mesh
                    .as_ref()
                    .map_or(0, |mesh| mesh.vertex_count()),
                preview_3d_render_in_flight: self.preview_3d.render_in_flight,
            },
        );
    }

    pub(super) fn refresh_canvas_tiles(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.canvas_tile_snapshot = Arc::new(build_tile_paint_snapshot(
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            self.toolbar_state.diagnostics_open,
            self.canvas_tile_generation,
        ));
        self.rebuild_paste_preview_images(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn remove_canvas_tiles(
        &mut self,
        coords: &[(i32, i32)],
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        if coords.is_empty() {
            return;
        }
        let removed_coords = coords.iter().copied().collect::<BTreeSet<_>>();
        let current = self.canvas_tile_snapshot.clone();
        let tiles = current
            .tiles
            .iter()
            .filter(|tile| !removed_coords.contains(&tile.coord))
            .cloned()
            .collect::<Vec<_>>();
        let debug_overlays = current
            .debug_overlays
            .iter()
            .filter(|overlay| !removed_coords.contains(&overlay.coord))
            .cloned()
            .collect::<Vec<_>>();
        if tiles.len() == current.tiles.len()
            && debug_overlays.len() == current.debug_overlays.len()
        {
            return;
        }
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        let estimated_bytes = tiles.iter().map(|tile| tile.estimated_bytes).sum::<usize>();
        self.canvas_tile_snapshot = Arc::new(TilePaintSnapshot {
            tiles: Arc::new(tiles),
            debug_overlays: Arc::new(debug_overlays),
            generation: self.canvas_tile_generation,
            estimated_bytes,
            paint_bounds: current.paint_bounds,
        });
        self.rebuild_paste_preview_images(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn refresh_canvas_tiles_if_changed(
        &mut self,
        changed_tiles: &[(i32, i32)],
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        let visible_bounds = visible_tile_bounds_for_viewport(
            self.viewport,
            self.active_layout,
            self.viewport.center_tile(self.active_layout),
        );
        let affects_visible = visible_bounds.is_none_or(|bounds| {
            changed_tiles
                .iter()
                .any(|coord| tile_bounds_contains(bounds, *coord))
        });
        if !affects_visible {
            return;
        }

        let generation = self.canvas_tile_generation.saturating_add(1);
        match patch_tile_paint_snapshot(
            &self.canvas_tile_snapshot,
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            self.toolbar_state.diagnostics_open,
            changed_tiles,
            generation,
        ) {
            TilePaintSnapshotPatch::Unchanged => {}
            TilePaintSnapshotPatch::Patched(snapshot) => {
                self.canvas_tile_generation = generation;
                self.canvas_tile_snapshot = Arc::new(snapshot);
                self.sync_tile_layer_snapshot(colors, cx);
            }
            TilePaintSnapshotPatch::Rebuild => self.refresh_canvas_tiles(colors, cx),
        }
    }

    pub(super) fn canvas_snapshot(&self, colors: ThemeColors) -> MapCanvasSnapshot {
        MapCanvasSnapshot {
            viewport: self.viewport,
            layout: self.active_layout,
            colors,
            overlays: self.overlay_options,
            tiles: self.canvas_tile_snapshot.clone(),
            overlay_paint: self.professional.overlay_paint.clone(),
            slime_runs: self.professional.slime_overlay_runs.clone(),
            selection: self.professional.selection,
            paste_preview: self.professional.paste_preview.clone(),
            paste_preview_images: self.paste_preview_images.clone(),
            paste_preview_images_generation: self.paste_preview_images_generation,
            highlighted_window: self.professional.highlighted_window.clone(),
            markers: Arc::new(
                self.markers
                    .get(&self.dimension)
                    .cloned()
                    .unwrap_or_default(),
            ),
            markers_generation: self.markers_generation,
            hover_label: SharedString::from(coordinate_text(
                self.hover_block_x,
                self.hover_block_z,
            )),
        }
    }

    pub(super) fn canvas_snapshot_key(&self, colors: ThemeColors) -> MapCanvasSnapshotKey {
        MapCanvasSnapshotKey {
            viewport: self.viewport,
            layout: self.active_layout,
            colors,
            dragging: self.drag.is_some() || self.ui_state.dock_drag.is_some(),
            overlays: self.overlay_options,
            tile_generation: self.canvas_tile_snapshot.generation,
            overlay_generation: self.professional.overlay_generation,
            overlay_paint_ptr: self
                .professional
                .overlay_paint
                .as_ref()
                .map(|cache| Arc::as_ptr(cache) as usize),
            slime_runs_ptr: self
                .professional
                .slime_overlay_runs
                .as_ref()
                .map(|cache| Arc::as_ptr(cache) as usize),
            selection: self.professional.selection,
            paste_preview: self.professional.paste_preview.clone(),
            paste_preview_images_generation: self.paste_preview_images_generation,
            highlighted_window: self.professional.highlighted_window.clone(),
            markers_generation: self.markers_generation,
            hover_block_x: self.hover_block_x,
            hover_block_z: self.hover_block_z,
        }
    }

    pub(super) fn tile_layer_snapshot_key(&self, colors: ThemeColors) -> TileLayerSnapshotKey {
        TileLayerSnapshotKey {
            viewport: self.viewport,
            layout: self.active_layout,
            colors,
            dragging: self.drag.is_some() || self.ui_state.dock_drag.is_some(),
            tile_generation: self.canvas_tile_snapshot.generation,
        }
    }

    pub(super) fn current_render_mode(&self) -> RenderMode {
        match self.mode {
            ViewerMode::Surface => RenderMode::SurfaceBlocks,
            ViewerMode::Biome => RenderMode::Biome { y: self.y_layer },
            ViewerMode::Height => RenderMode::HeightMap,
            ViewerMode::Layer => RenderMode::LayerBlocks { y: self.y_layer },
            ViewerMode::Cave => RenderMode::CaveSlice { y: self.y_layer },
        }
    }

    pub(super) fn cancel_active_render(&mut self) {
        for cancel in self.render_cancels.values() {
            cancel.cancel();
        }
        self.render_cancels.clear();
        self.render_batch_active = false;
        self.active_render_tiles.clear();
        self.active_render_center_tile = None;
        self.pending_viewport_refresh = false;
    }

    fn has_render_batch_capacity(&self) -> bool {
        self.render_cancels.len() < MAX_CONCURRENT_RENDER_BATCHES
    }

    fn track_render_request(
        &mut self,
        request_id: u64,
        render_cancel: RenderCancelFlag,
        requested_tiles: &[(i32, i32)],
        center_tile: (i32, i32),
    ) {
        self.render_cancels.insert(request_id, render_cancel);
        self.render_batch_active = true;
        self.active_render_tiles
            .extend(requested_tiles.iter().copied());
        self.active_render_center_tile = Some(center_tile);
    }

    fn finish_render_request(&mut self, request_id: u64, requested_tiles: &[(i32, i32)]) {
        self.render_cancels.remove(&request_id);
        for coord in requested_tiles {
            self.active_render_tiles.remove(coord);
        }
        self.render_batch_active = !self.render_cancels.is_empty();
        if self.active_render_tiles.is_empty() {
            self.active_render_center_tile = None;
        }
    }

    pub(super) fn cancel_metadata_scan(&mut self) {
        cancel_metadata_flag(&mut self.metadata_cancel);
        cancel_metadata_flag(&mut self.manifest_probe_cancel);
        self.manifest_probe_in_flight = false;
        self.metadata_loading = false;
    }

    pub(super) fn show_map_error(&mut self, message: impl Into<SharedString>, cx: &mut App) {
        let message = message.into();
        self.status = message.clone();
        if self
            .last_visible_error
            .as_ref()
            .is_none_or(|last_message| last_message != &message)
        {
            toast::error(cx, message.clone());
            self.last_visible_error = Some(message);
        }
    }

    pub(super) fn clear_visible_error(&mut self) {
        self.last_visible_error = None;
    }

    pub(super) fn refresh_render_session(&mut self, cx: &mut Context<Self>) {
        self.session_generation = self.session_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_metadata_scan();
        self.cancel_active_render();
        self.render_session = None;
        self.session_loading = true;
        self.clear_pending_render_image_evictions(cx);
        self.metadata_index_ready = false;
        self.status = SharedString::from("正在打开地图渲染会话...");
        tracing::debug!(
            generation = self.session_generation,
            backend = ?self.render_backend,
            world = %self.world_path.display(),
            "map_viewer session_open_start"
        );
        cx.notify();

        let generation = self.session_generation;
        let world_path = self.world_path.clone();
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    open_map_render_session(world_path, render_backend, render_gpu_backend)
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            if let Err(error) = view.update(cx, move |this, cx| {
                if this.session_generation != generation {
                    return;
                }
                this.session_loading = false;
                match result {
                    Ok(session) => {
                        this.render_session = Some(Arc::new(session));
                        this.status = SharedString::from("地图渲染会话就绪 · 正在扫描地图索引");
                        this.clear_visible_error();
                        tracing::debug!(generation, "map_viewer session_open_ok");
                        this.refresh_metadata(cx);
                    }
                    Err(error) => {
                        tracing::warn!(generation, %error, "map_viewer session_open_failed");
                        this.show_map_error(SharedString::from(error), cx);
                    }
                }
                cx.notify();
            }) {
                tracing::warn!(?error, "failed to update map render session state");
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn refresh_render_session_after_edit(
        &mut self,
        affected_tiles: Vec<(i32, i32)>,
        affected_chunks: BTreeSet<ChunkPos>,
        tile_priority: TilePriority,
        reuse_known_tile_index: bool,
        cx: &mut Context<Self>,
    ) {
        self.session_generation = self.session_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_metadata_scan();
        self.cancel_active_render();
        self.render_session = None;
        self.session_loading = true;
        self.clear_pending_render_image_evictions(cx);
        self.status = SharedString::from("正在刷新地图渲染会话...");
        tracing::debug!(
            generation = self.session_generation,
            tiles = affected_tiles.len(),
            backend = ?self.render_backend,
            world = %self.world_path.display(),
            "map_viewer edit_session_refresh_start"
        );
        cx.notify();

        let generation = self.session_generation;
        let world_path = self.world_path.clone();
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    open_map_render_session(world_path, render_backend, render_gpu_backend)
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            if let Err(error) = view.update(cx, move |this, cx| {
                if this.session_generation != generation {
                    return;
                }
                this.session_loading = false;
                match result {
                    Ok(session) => {
                        this.render_session = Some(Arc::new(session));
                        this.clear_visible_error();
                        if affected_tiles.is_empty() {
                            this.invalidate_tiles(cx);
                            this.status =
                                SharedString::from("渲染会话已刷新 · 正在重建可见瓦片");
                        } else {
                            this.queue_edit_refresh_tiles_after_session_refresh(
                                &affected_tiles,
                                &affected_chunks,
                                tile_priority,
                                reuse_known_tile_index,
                                cx,
                            );
                            let chunk_count = affected_chunks.len();
                            this.status = if reuse_known_tile_index && chunk_count > 0 {
                                SharedString::from(format!(
                                    "渲染会话已刷新 · 正在局部刷新 {chunk_count} 个 chunk"
                                ))
                            } else {
                                SharedString::from(format!(
                                    "渲染会话已刷新 · 正在重渲染 {} 个受影响瓦片",
                                    affected_tiles.len()
                                ))
                            };
                    }
                        tracing::debug!(
                            generation,
                            tiles = affected_tiles.len(),
                            "map_viewer edit_session_refresh_ok"
                        );
                        if !this.render_batch_active {
                            this.ensure_visible_tiles(cx);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(generation, %error, "map_viewer edit_session_refresh_failed");
                        this.show_map_error(SharedString::from(error), cx);
                    }
                }
                cx.notify();
            }) {
                tracing::warn!(?error, "failed to update refreshed map render session");
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn queue_edit_refresh_tiles_after_session_refresh(
        &mut self,
        affected_tiles: &[(i32, i32)],
        affected_chunks: &BTreeSet<ChunkPos>,
        tile_priority: TilePriority,
        reuse_known_tile_index: bool,
        cx: &mut Context<Self>,
    ) {
        if affected_tiles.is_empty() {
            return;
        }
        let mut direct_refresh_tiles = Vec::new();
        let mut manifest_refresh_tiles = Vec::new();
        let mut partial_refresh_requests = Vec::new();
        for coord in affected_tiles {
            self.available_tiles.remove(coord);
            if self
                .tile_chunk_index
                .get(coord)
                .is_some_and(|positions| !positions.is_empty())
            {
                if reuse_known_tile_index
                    && let Some(base_tile) = self.tile_manager.loaded_tile(*coord)
                {
                    let chunks = chunks_for_tile(affected_chunks, *coord, self.active_layout);
                    if !chunks.is_empty() {
                        partial_refresh_requests.push(ChunkPatchRefreshPlan {
                            coord: *coord,
                            chunks,
                            base_tile,
                        });
                        continue;
                    }
                }
                direct_refresh_tiles.push(*coord);
            } else {
                self.tile_chunk_index.remove(coord);
                self.manifest_scanned_tiles.remove(coord);
                Self::drop_render_image(self.tile_manager.remove_tile(*coord), cx);
                manifest_refresh_tiles.push(*coord);
            }
        }
        self.tile_manager
            .force_refresh_tiles(&direct_refresh_tiles, tile_priority);
        self.tile_manager
            .ensure_pending_manifest(&manifest_refresh_tiles, tile_priority);
        let partial_tile_count = partial_refresh_requests.len();
        if !partial_refresh_requests.is_empty() {
            self.schedule_chunk_patch_refresh(partial_refresh_requests, tile_priority, cx);
        }
        self.last_visible_tile_signature = None;
        self.pending_viewport_refresh = true;
        tracing::debug!(
            direct_tiles = direct_refresh_tiles.len(),
            manifest_tiles = manifest_refresh_tiles.len(),
            partial_tiles = partial_tile_count,
            "map_viewer edit_refresh_tiles_queued"
        );
    }

    pub(super) fn schedule_chunk_patch_refresh(
        &mut self,
        requests: Vec<ChunkPatchRefreshPlan>,
        tile_priority: TilePriority,
        cx: &mut Context<Self>,
    ) {
        let Some(render_session) = self.render_session.clone() else {
            let fallback_tiles = requests
                .iter()
                .map(|request| request.coord)
                .collect::<Vec<_>>();
            self.tile_manager
                .force_refresh_tiles(&fallback_tiles, tile_priority);
            return;
        };
        if requests.is_empty() {
            return;
        }
        let request_id = self.request_id.saturating_add(1);
        self.request_id = request_id;
        let render_generation = self.render_generation;
        let mode = self.current_render_mode();
        let layout = self.active_layout;
        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let requested_tiles = requests
            .iter()
            .map(|request| request.coord)
            .collect::<Vec<_>>();
        let render_cancel = RenderCancelFlag::new();
        let render_cancel_for_task = render_cancel.clone();
        self.track_render_request(
            request_id,
            render_cancel,
            &requested_tiles,
            self.viewport.center_tile(self.active_layout),
        );
        self.pending_viewport_refresh = false;
        self.status = SharedString::from(format!(
            "局部刷新 {} 个瓦片 / {} 个 chunk",
            requests.len(),
            requests
                .iter()
                .map(|request| request.chunks.len())
                .sum::<usize>()
        ));
        cx.notify();
        let requested_tiles_for_finish = requested_tiles.clone();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut results = Vec::new();
                    let mut failed_tiles = BTreeSet::new();
                    for plan in requests {
                        let coord = plan.coord;
                        let request = ChunkPatchRenderRequest {
                            render_session: render_session.clone(),
                            mode,
                            layout,
                            tile_coord: coord,
                            chunks: plan.chunks,
                            base_tile: plan.base_tile,
                            cpu_budget,
                            render_backend,
                            render_gpu_backend,
                            render_cancel: render_cancel_for_task.clone(),
                        };
                        match render_chunk_patches_blocking(request) {
                            Ok(result) => results.push(result),
                            Err(error) => {
                                tracing::debug!(
                                    tile = ?coord,
                                    %error,
                                    "map_viewer chunk_patch_refresh_failed"
                                );
                                failed_tiles.insert(coord);
                            }
                        }
                    }
                    (results, failed_tiles)
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.render_generation != render_generation {
                    return;
                }
                this.finish_render_request(request_id, &requested_tiles_for_finish);
                let colors = this.theme_colors(cx);
                let (results, failed_tiles) = result;
                let fallback_count = failed_tiles.len();
                let mut changed_tiles = Vec::with_capacity(results.len());
                let mut refreshed_chunks = 0usize;
                for result in results {
                    refreshed_chunks =
                        refreshed_chunks.saturating_add(result.refreshed_chunks.len());
                    this.diagnostics.add(result.diagnostics);
                    this.render_stats = result.stats;
                    Self::drop_render_image(
                        this.tile_manager.mark_loaded(result.coord, result.tile),
                        cx,
                    );
                    this.available_tiles.insert(result.coord);
                    changed_tiles.push(result.coord);
                }
                if !failed_tiles.is_empty() {
                    let fallback_tiles = failed_tiles.into_iter().collect::<Vec<_>>();
                    Self::drop_render_images(
                        this.tile_manager
                            .force_refresh_tiles(&fallback_tiles, tile_priority),
                        cx,
                    );
                    this.last_visible_tile_signature = None;
                    this.pending_viewport_refresh = true;
                }
                this.partial_refreshed_chunks = this
                    .partial_refreshed_chunks
                    .saturating_add(refreshed_chunks);
                if !changed_tiles.is_empty() {
                    this.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
                }
                this.status = SharedString::from(format!(
                    "局部刷新完成 · {} 个 chunk · fallback {} 个 tile",
                    refreshed_chunks, fallback_count
                ));
                this.schedule_next_tile_batch(cx);
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn refresh_metadata(&mut self, cx: &mut Context<Self>) {
        self.cancel_metadata_scan();
        self.metadata_generation = self.metadata_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_active_render();
        self.metadata_loading = true;
        Self::drop_render_images(self.tile_manager.clear(), cx);
        self.clear_canvas_tile_snapshot();
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        self.tile_reveal_state = TileRevealState::default();
        self.metadata_index_ready = false;
        self.manifest_probe_in_flight = false;
        self.manifest_scanned_tiles.clear();
        self.available_tiles.clear();
        self.tile_chunk_index.clear();
        self.chunk_bounds = None;
        self.diagnostics = RenderDiagnostics::default();
        self.render_stats = RenderPipelineStats::default();
        self.status = SharedString::from("正在读取地图瓦片索引...");
        tracing::debug!(
            generation = self.metadata_generation,
            dimension = ?self.dimension,
            cpu_percent = self.cpu_budget.percent,
            world = %self.world_path.display(),
            "map_viewer manifest_load_start"
        );
        cx.notify();

        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let dimension = self.dimension;
        let mode = self.current_render_mode();
        let layout = self.active_layout;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let recenter = self.recenter_on_next_metadata || !self.viewport.initialized;
        self.recenter_on_next_metadata = false;
        let pending_center_block = self.pending_center_block.take();
        let metadata_cancel = RenderTaskControl::new();
        let metadata_cancel_for_task = metadata_cancel.clone();
        let metadata_cancel_for_owner = metadata_cancel.clone();
        self.metadata_cancel = Some(metadata_cancel);

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    load_tile_manifest_from_disk(
                        world_path,
                        render_backend,
                        render_gpu_backend,
                        mode,
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
            if let Err(error) = view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    metadata_cancel_for_owner.cancel();
                    return;
                }
                if metadata_cancel_for_owner.is_cancelled() {
                    this.metadata_loading = false;
                    this.metadata_cancel = None;
                    tracing::debug!(generation, "map_viewer manifest_load_cancelled");
                    return;
                }
                this.metadata_cancel = None;
                this.metadata_loading = false;
                match result {
                    Ok(Some(result)) => {
                        this.render_generation = this.render_generation.saturating_add(1);
                        this.cancel_active_render();
                        Self::drop_render_images(this.tile_manager.clear(), cx);
                        this.clear_canvas_tile_snapshot();
                        let colors = this.theme_colors(cx);
                        this.sync_canvas_snapshot(colors, cx);
                        this.tile_reveal_state = TileRevealState::default();
                        this.bypass_cache_active = false;
                        this.available_tiles = result
                            .tile_chunk_index
                            .iter()
                            .filter_map(|(coord, positions)| {
                                (!positions.is_empty()).then_some(*coord)
                            })
                            .collect();
                        this.manifest_scanned_tiles =
                            result.tile_chunk_index.keys().copied().collect();
                        this.tile_chunk_index = result.tile_chunk_index;
                        this.refresh_chunk_tree_if_selected();
                        this.chunk_bounds = result.bounds;
                        this.metadata_index_ready = true;
                        if let Some((block_x, block_z)) = pending_center_block {
                            this.viewport.center_on_block(block_x, block_z, layout);
                        } else if recenter
                            && let (Some(block_x), Some(block_z)) =
                                (result.center_block_x, result.center_block_z)
                        {
                            this.viewport.center_on_block(block_x, block_z, layout);
                        }
                        this.status = SharedString::from(format!(
                            "本地地图索引就绪 · {} 个瓦片 · CPU预算 {}%",
                            this.available_tiles.len(),
                            this.cpu_budget.percent
                        ));
                        this.clear_visible_error();
                        tracing::debug!(
                            generation,
                            tiles = this.available_tiles.len(),
                            indexed_tiles = this.tile_chunk_index.len(),
                            bounds = ?this.chunk_bounds,
                            render_generation = this.render_generation,
                            "map_viewer manifest_load_ok"
                        );
                        this.ensure_visible_tiles(cx);
                    }
                    Ok(None) => {
                        this.metadata_index_ready = false;
                        this.status = SharedString::from("地图索引为空 · 正在从中心瓦片开始加载");
                        tracing::debug!(generation, "map_viewer manifest_load_miss");
                        this.ensure_visible_tiles(cx);
                    }
                    Err(error) => {
                        this.metadata_index_ready = false;
                        tracing::warn!(generation, %error, "map_viewer manifest_load_failed");
                        this.show_map_error(SharedString::from(error), cx);
                        this.ensure_visible_tiles(cx);
                    }
                }
                cx.notify();
            }) {
                tracing::warn!(?error, "failed to update map metadata state");
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn invalidate_tiles(&mut self, cx: &mut Context<Self>) {
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_metadata_scan();
        self.cancel_active_render();
        Self::drop_render_images(self.tile_manager.clear(), cx);
        self.clear_pending_render_image_evictions(cx);
        self.clear_canvas_tile_snapshot();
        self.manifest_probe_in_flight = false;
        self.diagnostics = RenderDiagnostics::default();
        self.render_stats = RenderPipelineStats::default();
        self.refresh_rendered_tiles = 0;
        self.partial_refreshed_chunks = 0;
        self.cold_rendered_tiles = 0;
        self.last_queue_distance_squared = 0;
        self.tile_reveal_state = TileRevealState::default();
        self.last_tile_memory_trim = None;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn ensure_visible_tiles(&mut self, cx: &mut Context<Self>) {
        self.last_viewport_tile_sync = Some(Instant::now());
        if self.render_session.is_none() {
            if !self.session_loading {
                tracing::debug!("map_viewer visible_tiles_waiting_for_session");
                self.refresh_render_session(cx);
            }
            return;
        }
        if self.metadata_loading {
            self.status = SharedString::from("地图索引后台扫描中 · 正在优先渲染当前视口");
        } else if !self.metadata_index_ready {
            self.status = SharedString::from("地图索引暂无区块 · 正在尝试渲染当前视口");
        }

        let tile_plan = self.viewport_tile_plan();
        if tile_plan.visible.is_empty() {
            self.status = if self.metadata_loading {
                SharedString::from("正在等待视口尺寸或地图索引")
            } else {
                SharedString::from("视口内没有可渲染瓦片")
            };
            return;
        }
        let visible_signature = ViewportTileSignature {
            visible: tile_plan.visible.clone(),
            prefetch: tile_plan.prefetch.clone(),
            retain: tile_plan.retain.iter().copied().collect(),
            center: tile_plan.center,
            metadata_loading: self.metadata_loading,
            metadata_index_ready: self.metadata_index_ready,
        };
        let signature_changed = self
            .last_visible_tile_signature
            .as_ref()
            .is_none_or(|previous| previous != &visible_signature);
        let now = Instant::now();
        let should_log_visible = signature_changed
            || self.last_visible_tile_log.is_none_or(|last| {
                now.saturating_duration_since(last) >= VISIBLE_TILE_LOG_INTERVAL
            });
        if should_log_visible {
            self.last_visible_tile_log = Some(now);
            tracing::debug!(
                visible = tile_plan.visible.len(),
                prefetch = tile_plan.prefetch.len(),
                metadata_loading = self.metadata_loading,
                metadata_index_ready = self.metadata_index_ready,
                available_tiles = self.available_tiles.len(),
                chunk_bounds = ?self.chunk_bounds,
                center = ?tile_plan.center,
                "map_viewer visible_tiles"
            );
        }
        if !signature_changed && self.render_batch_active && !self.has_render_batch_capacity() {
            return;
        }
        self.last_visible_tile_signature = Some(visible_signature);

        for image in self.tile_manager.retain_tiles(&tile_plan.retain) {
            if tile_plan.is_dragging {
                self.delay_render_image_drop(image, cx);
            } else {
                cx.drop_image(image, None);
            }
        }
        let mut visible_renderable_tiles = Vec::new();
        let mut visible_pending_manifest_tiles = Vec::new();
        for coord in &tile_plan.visible {
            match self.tile_chunk_index.get(coord) {
                Some(positions) if positions.is_empty() => {
                    Self::drop_render_image(
                        self.tile_manager.mark_invalid(
                            *coord,
                            SharedString::from("索引确认该瓦片没有可渲染区块"),
                        ),
                        cx,
                    );
                }
                Some(positions) => {
                    if !positions.is_empty() {
                        self.available_tiles.insert(*coord);
                        visible_renderable_tiles.push(*coord);
                    }
                }
                None => visible_pending_manifest_tiles.push(*coord),
            }
        }
        self.tile_manager
            .ensure_tiles(&visible_renderable_tiles, TilePriority::Visible);
        self.tile_manager
            .ensure_pending_manifest(&visible_pending_manifest_tiles, TilePriority::Visible);
        self.trim_tiles_to_memory_budget_for_retained(&tile_plan.retain, true, cx);
        let mut prefetch_renderable_tiles = Vec::new();
        let mut prefetch_pending_manifest_tiles = Vec::new();
        if self.metadata_index_ready && tile_plan.prefetch_radius > 0 {
            let visible_set = tile_plan.visible.iter().copied().collect::<BTreeSet<_>>();
            for coord in &tile_plan.prefetch {
                if visible_set.contains(coord) {
                    continue;
                }
                match self.tile_chunk_index.get(coord) {
                    Some(positions) if positions.is_empty() => {
                        Self::drop_render_image(
                            self.tile_manager.mark_invalid(
                                *coord,
                                SharedString::from("索引确认该瓦片没有可渲染区块"),
                            ),
                            cx,
                        );
                    }
                    Some(positions) => {
                        if !positions.is_empty() {
                            self.available_tiles.insert(*coord);
                            prefetch_renderable_tiles.push(*coord);
                        }
                    }
                    None => prefetch_pending_manifest_tiles.push(*coord),
                }
            }
            self.tile_manager
                .ensure_tiles(&prefetch_renderable_tiles, TilePriority::Prefetch);
            self.tile_manager
                .ensure_pending_manifest(&prefetch_pending_manifest_tiles, TilePriority::Prefetch);
        }
        let edit_refresh_tiles = self
            .tile_manager
            .pending_manifest_coords_with_priority(TilePriority::EditRefresh);
        let has_edit_refresh_manifest = !edit_refresh_tiles.is_empty();
        let should_probe_manifest = !self.metadata_loading
            && !self.manifest_probe_in_flight
            && (has_edit_refresh_manifest
                || (!self.tile_manager.has_visible_work()
                    && (!visible_pending_manifest_tiles.is_empty()
                        || !prefetch_pending_manifest_tiles.is_empty())));
        if should_probe_manifest {
            let prefetch_probe_tiles = visible_pending_manifest_tiles
                .iter()
                .chain(prefetch_pending_manifest_tiles.iter())
                .chain(edit_refresh_tiles.iter())
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            self.schedule_tile_manifest_probe(
                &visible_pending_manifest_tiles,
                &prefetch_probe_tiles,
                tile_plan.center,
                cx,
            );
        }

        if self.render_batch_active
            && (self.tile_manager.has_visible_work() || !visible_pending_manifest_tiles.is_empty())
        {
            self.pending_viewport_refresh = true;
        }
        self.schedule_next_tile_batch(cx);
    }

    pub(super) fn schedule_tile_manifest_probe(
        &mut self,
        visible_tiles: &[(i32, i32)],
        prefetch_tiles: &[(i32, i32)],
        center_tile: (i32, i32),
        cx: &mut Context<Self>,
    ) {
        if self.manifest_probe_in_flight {
            return;
        }
        let requested_tiles = select_manifest_probe_tiles(
            visible_tiles,
            prefetch_tiles,
            center_tile,
            &self.manifest_scanned_tiles,
        );
        if requested_tiles.is_empty() {
            return;
        }

        cancel_metadata_flag(&mut self.manifest_probe_cancel);
        self.manifest_probe_in_flight = true;
        self.manifest_probe_diagnostics
            .record_probe_start(requested_tiles.len(), center_tile);
        self.status = SharedString::from(format!(
            "正在探测中心瓦片索引 · 瓦片 {} · 排队 {}",
            requested_tiles.len(),
            self.tile_manager.queued_count()
        ));
        tracing::debug!(
            tiles = requested_tiles.len(),
            center = ?center_tile,
            first = ?requested_tiles.first(),
            "map_viewer manifest_probe_start"
        );
        cx.notify();

        let generation = self.metadata_generation;
        let Some(render_session) = self.render_session.clone() else {
            self.refresh_render_session(cx);
            return;
        };
        let dimension = self.dimension;
        let layout = self.active_layout;
        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let mode = self.current_render_mode();
        let manifest_probe_cancel = RenderTaskControl::new();
        let manifest_probe_cancel_for_task = manifest_probe_cancel.clone();
        let manifest_probe_cancel_for_owner = manifest_probe_cancel.clone();
        self.manifest_probe_cancel = Some(manifest_probe_cancel);

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    load_tile_manifest_probe(
                        render_session,
                        render_backend,
                        render_gpu_backend,
                        mode,
                        dimension,
                        layout,
                        requested_tiles,
                        cpu_budget,
                        manifest_probe_cancel_for_task,
                    )
                })
                .await;

            let Some(view) = handle.upgrade() else {
                manifest_probe_cancel_for_owner.cancel();
                return Ok(());
            };
            if let Err(error) = view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    manifest_probe_cancel_for_owner.cancel();
                    return;
                }
                this.manifest_probe_in_flight = false;
                this.manifest_probe_cancel = None;
                if manifest_probe_cancel_for_owner.is_cancelled() {
                    tracing::debug!(generation, "map_viewer manifest_probe_cancelled");
                    this.schedule_next_tile_batch(cx);
                    cx.notify();
                    return;
                }
                match result {
                    Ok(result) => {
                        let mut empty_tiles = 0usize;
                        let mut non_empty_tiles = 0usize;
                        for coord in &result.requested_tiles {
                            this.manifest_scanned_tiles.insert(*coord);
                        }
                        for (coord, positions) in result.tile_chunk_index {
                            let priority = this
                                .tile_manager
                                .entries
                                .get(&coord)
                                .map_or(TilePriority::Visible, |entry| entry.priority);
                            if positions.is_empty() {
                                empty_tiles = empty_tiles.saturating_add(1);
                                Self::drop_render_image(
                                    this.tile_manager.mark_invalid(
                                        coord,
                                        SharedString::from("索引确认该瓦片没有可渲染区块"),
                                    ),
                                    cx,
                                );
                            } else {
                                non_empty_tiles = non_empty_tiles.saturating_add(1);
                                this.available_tiles.insert(coord);
                                this.tile_manager.mark_manifest_ready(coord, priority);
                            }
                            this.tile_chunk_index.insert(coord, positions);
                        }
                        this.refresh_chunk_tree_if_selected();
                        this.chunk_bounds = merge_chunk_bounds(this.chunk_bounds, result.bounds);
                        this.metadata_index_ready = !this.tile_chunk_index.is_empty();
                        tracing::debug!(
                            requested = result.requested_tiles.len(),
                            non_empty_tiles,
                            empty_tiles,
                            indexed_tiles = this.tile_chunk_index.len(),
                            "map_viewer manifest_probe_ok"
                        );
                        this.status = SharedString::from(format!(
                            "索引探测完成 · non-empty {non_empty_tiles} · empty {empty_tiles} · queued {}",
                            this.tile_manager.queued_count()
                        ));
                        this.ensure_visible_tiles(cx);
                    }
                    Err(error) => {
                        if error.to_ascii_lowercase().contains("cancel") || error.contains("取消")
                        {
                            tracing::debug!(%error, "map_viewer manifest_probe_cancelled");
                        } else {
                            this.status = SharedString::from(error.clone());
                            tracing::warn!(%error, "map_viewer manifest_probe_failed");
                        }
                        this.schedule_next_tile_batch(cx);
                    }
                }
                cx.notify();
            }) {
                tracing::warn!(?error, "failed to merge tile manifest probe");
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn ensure_visible_tiles_throttled(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let sync_interval = if self.drag.is_some() || self.ui_state.dock_drag.is_some() {
            DRAG_VIEWPORT_TILE_SYNC_INTERVAL
        } else {
            VIEWPORT_TILE_SYNC_INTERVAL
        };
        let should_sync = self
            .last_viewport_tile_sync
            .is_none_or(|last_sync| now.saturating_duration_since(last_sync) >= sync_interval);
        if should_sync {
            self.ensure_visible_tiles(cx);
        }
    }

    fn has_current_viewport_work_or_pending_manifest(&self) -> bool {
        let visible_tiles = self.tile_coords_for_viewport(0);
        self.tile_manager.has_visible_work()
            || self
                .tile_manager
                .has_pending_manifest_for_tiles(&visible_tiles)
    }

    pub(super) fn trim_tiles_to_memory_budget(&mut self, force: bool, cx: &mut Context<Self>) {
        let is_dragging = self.drag.is_some() || self.ui_state.dock_drag.is_some();
        let mut retained_tiles = self
            .tile_coords_for_viewport(if is_dragging {
                DRAG_RETAIN_RADIUS
            } else {
                RETAIN_RADIUS
            })
            .into_iter()
            .collect::<BTreeSet<_>>();
        if retained_tiles.is_empty() {
            retained_tiles = self.tile_coords_for_viewport(0).into_iter().collect();
        }
        self.trim_tiles_to_memory_budget_for_retained(&retained_tiles, force, cx);
    }

    pub(super) fn trim_tiles_to_memory_budget_for_retained(
        &mut self,
        retained_tiles: &BTreeSet<(i32, i32)>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let budget = ui_tile_memory_budget_bytes(self.viewport);
        if self.tile_manager.loaded_estimated_bytes() <= budget {
            return;
        }
        let now = Instant::now();
        if !force
            && self.last_tile_memory_trim.is_some_and(|last_trim| {
                now.saturating_duration_since(last_trim) < TILE_MEMORY_TRIM_INTERVAL
            })
        {
            return;
        }
        self.last_tile_memory_trim = Some(now);
        Self::drop_render_images(
            self.tile_manager
                .trim_loaded_tiles_to_budget(retained_tiles, budget),
            cx,
        );
        self.flush_pending_render_image_evictions(cx);
    }

    pub(super) fn viewport_tile_plan(&self) -> ViewportTilePlan {
        let center = self.viewport.center_tile(self.active_layout);
        let visible_bounds =
            visible_tile_bounds_for_viewport(self.viewport, self.active_layout, center);
        let visible = visible_bounds
            .map(|bounds| tile_coords_for_bounds(bounds, 0, center))
            .unwrap_or_default();
        let is_dragging = self.drag.is_some() || self.ui_state.dock_drag.is_some();
        let retain_radius = if is_dragging {
            DRAG_RETAIN_RADIUS
        } else {
            RETAIN_RADIUS
        };
        let retain = visible_bounds
            .map(|bounds| {
                tile_coords_for_bounds(bounds, retain_radius, center)
                    .into_iter()
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        let prefetch_radius = if is_dragging {
            DRAG_PREFETCH_RADIUS.max(map_viewer_prefetch_radius())
        } else {
            map_viewer_prefetch_radius()
        };
        let mut prefetch = if self.metadata_index_ready && prefetch_radius > 0 {
            visible_bounds
                .map(|bounds| tile_coords_for_bounds(bounds, prefetch_radius, center))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if self.metadata_index_ready
            && prefetch_radius > 0
            && is_dragging
            && let (Some(visible_bounds), Some(drag)) = (visible_bounds, self.drag)
        {
            let mut projected_viewport = self.viewport;
            let drag_bias = drag.last_movement_x.abs().max(drag.last_movement_y.abs());
            let direction_x = drag.last_movement_x.signum();
            let direction_y = drag.last_movement_y.signum();
            let projected_shift = drag_bias.max(32.0);
            projected_viewport.offset_x += direction_x * projected_shift;
            projected_viewport.offset_y += direction_y * projected_shift;
            if let Some(projected_bounds) =
                visible_tile_bounds_for_viewport(projected_viewport, self.active_layout, center)
            {
                prefetch.extend(collect_circular_tile_coords(
                    visible_bounds,
                    projected_bounds.expand(prefetch_radius),
                    prefetch_radius,
                    center,
                ));
                prefetch.sort_unstable();
                prefetch.dedup();
            }
        }
        ViewportTilePlan {
            visible,
            prefetch,
            retain,
            center,
            is_dragging,
            prefetch_radius,
        }
    }

    pub(super) fn tile_coords_for_viewport(&self, radius: i32) -> Vec<(i32, i32)> {
        let center = self.viewport.center_tile(self.active_layout);
        let Some(visible) =
            visible_tile_bounds_for_viewport(self.viewport, self.active_layout, center)
        else {
            return Vec::new();
        };
        tile_coords_for_bounds(visible, radius, center)
    }

    pub(super) fn schedule_next_tile_batch(&mut self, cx: &mut Context<Self>) {
        if !self.has_render_batch_capacity() {
            return;
        }
        let Some(render_session) = self.render_session.clone() else {
            if !self.session_loading {
                tracing::debug!("map_viewer schedule_waiting_for_session");
                self.refresh_render_session(cx);
            }
            return;
        };
        let visible_tiles = self.tile_coords_for_viewport(0);
        let batch_size = visible_render_batch_size(
            interactive_tile_batch_size(self.render_backend, self.cpu_budget),
            visible_tiles.len(),
            self.drag.is_some() || self.ui_state.dock_drag.is_some(),
            self.tile_manager.loaded_count() == 0 && self.tile_manager.loading_count() == 0,
        );
        let center_tile = self.viewport.center_tile(self.active_layout);
        let allow_prefetch = self.metadata_index_ready
            && map_viewer_prefetch_radius() > 0
            && !self.pending_viewport_refresh
            && !self.tile_manager.has_visible_work()
            && !self
                .tile_manager
                .has_pending_manifest_for_tiles(&visible_tiles);
        let visible_bounds = tile_bounds_from_coords(&visible_tiles);
        let prioritize_center = !allow_prefetch;
        let candidate_limit = batch_size.saturating_mul(4).max(batch_size).max(16);
        let candidate_tiles = self.tile_manager.queued_coords_limited(
            center_tile,
            visible_bounds,
            allow_prefetch,
            prioritize_center,
            candidate_limit,
        );
        let mode = self.current_render_mode();
        let dimension = self.dimension;
        let layout = self.active_layout;
        let mut render_plans = Vec::with_capacity(batch_size);
        for coord in candidate_tiles {
            if render_plans.len() >= batch_size {
                break;
            }
            let priority = self
                .tile_manager
                .entries
                .get(&coord)
                .map_or(TilePriority::Prefetch, |entry| entry.priority);
            let Some(chunk_positions) = self.tile_chunk_index.get(&coord).map(Arc::clone) else {
                self.tile_manager
                    .ensure_pending_manifest(&[coord], priority);
                continue;
            };
            if chunk_positions.is_empty() {
                Self::drop_render_image(
                    self.tile_manager
                        .mark_invalid(coord, SharedString::from("索引确认该瓦片没有可渲染区块")),
                    cx,
                );
                continue;
            }
            match RenderTilePlan::new(dimension, mode, layout, coord, chunk_positions) {
                Ok(plan) => render_plans.push(plan),
                Err(error) if error.contains("没有可渲染区块") => {
                    Self::drop_render_image(
                        self.tile_manager
                            .mark_invalid(coord, SharedString::from(error)),
                        cx,
                    );
                }
                Err(error) => {
                    Self::drop_render_image(
                        self.tile_manager
                            .mark_failed(coord, SharedString::from(error)),
                        cx,
                    );
                }
            }
        }
        let requested_tiles = render_plans
            .iter()
            .map(|plan| plan.coord)
            .collect::<Vec<_>>();
        let has_edit_refresh_tiles = requested_tiles.iter().any(|coord| {
            self.tile_manager
                .entries
                .get(coord)
                .is_some_and(|entry| entry.priority == TilePriority::EditRefresh)
        });
        let cache_policy = if self.bypass_cache_active {
            RenderCachePolicy::Bypass
        } else if has_edit_refresh_tiles {
            RenderCachePolicy::Refresh
        } else {
            RenderCachePolicy::Use
        };
        if requested_tiles.is_empty() {
            if self.tile_manager.queued_count() == 0 && self.tile_manager.loading_count() == 0 {
                self.bypass_cache_active = false;
            }
            let visible_loaded = visible_tiles
                .iter()
                .filter(|coord| {
                    self.tile_manager.entries.get(coord).is_some_and(|entry| {
                        matches!(entry.state, TileLoadState::Loaded | TileLoadState::Invalid)
                    })
                })
                .count();
            self.status = if visible_tiles.is_empty() {
                SharedString::from("视口内没有可渲染瓦片")
            } else if visible_loaded == visible_tiles.len() {
                SharedString::from(format!(
                    "可见瓦片已就绪 · 已加载 {} · 失败 {} · CPU {}%",
                    self.tile_manager.loaded_count(),
                    self.tile_manager.failed_count(),
                    self.cpu_budget.percent
                ))
            } else {
                SharedString::from(format!(
                    "暂无待渲染瓦片 · 可见 {visible_loaded}/{} · 排队 {} · 探测 {} · 加载中 {} · 失败 {}",
                    visible_tiles.len(),
                    self.tile_manager.queued_count(),
                    self.tile_manager.pending_manifest_count(),
                    self.tile_manager.loading_count(),
                    self.tile_manager.failed_count()
                ))
            };
            tracing::debug!(
                visible = visible_tiles.len(),
                visible_loaded,
                queued = self.tile_manager.queued_count(),
                pending_manifest = self.tile_manager.pending_manifest_count(),
                loading = self.tile_manager.loading_count(),
                failed = self.tile_manager.failed_count(),
                "map_viewer render_batch_idle"
            );
            return;
        }
        self.tile_manager.mark_loading(&requested_tiles);

        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        let render_generation = self.render_generation;
        let requested_tile_count = requested_tiles.len();
        self.pending_viewport_refresh = false;
        self.last_queue_distance_squared =
            max_tile_distance_squared(&requested_tiles, center_tile).unwrap_or(0);
        let render_cancel = RenderCancelFlag::new();
        let render_cancel_for_owner = render_cancel.clone();
        self.track_render_request(
            request_id,
            render_cancel.clone(),
            &requested_tiles,
            center_tile,
        );
        let render_label = match cache_policy {
            RenderCachePolicy::Use => "缓存/渲染",
            RenderCachePolicy::Refresh => "刷新渲染",
            RenderCachePolicy::Bypass => "CPU 补齐",
        };
        self.status = SharedString::from(format!(
            "{render_label} {} 个瓦片 · 队列 {} · 队列距离² {} · CPU 解码预算 {}%",
            requested_tile_count,
            self.tile_manager.queued_count(),
            self.last_queue_distance_squared,
            self.cpu_budget.percent
        ));
        cx.notify();

        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let tile_cache_validation_seed = bedrock_render::render_preset_cache_validation_seed(
            &self.world_path,
            render_backend,
            render_gpu_backend,
        );
        let metadata_indexed_tiles = requested_tiles
            .iter()
            .filter(|coord| self.tile_chunk_index.contains_key(coord))
            .count();
        let unindexed_tiles = requested_tile_count.saturating_sub(metadata_indexed_tiles);
        let batch_chunk_count =
            selected_tile_chunk_count(&requested_tiles, self.active_layout, &self.tile_chunk_index);
        let batch_region_count = selected_tile_region_count(
            &requested_tiles,
            self.active_layout,
            &self.tile_chunk_index,
        );
        let quick_reveal = self.tile_reveal_state.ready_batches == 0
            || self.drag.is_some()
            || self.ui_state.dock_drag.is_some();
        let tile_batch_request = TileBatchRequest {
            render_session,
            dimension,
            layout,
            center_tile,
            cache_policy,
            plans: render_plans,
            cpu_budget,
            render_backend,
            render_gpu_backend,
            tile_cache_validation_seed,
            quick_reveal,
            render_cancel,
        };
        let (event_sender, mut event_receiver) = unbounded::<TileRenderEvent>();
        let batch_started = Instant::now();
        tracing::debug!(
            request_id,
            tiles = requested_tile_count,
            ui_batch_tiles = requested_tile_count,
            ui_batch_chunks = batch_chunk_count,
            ui_batch_regions = batch_region_count,
            metadata_indexed_tiles,
            unindexed_tiles,
            exact_manifest_chunks = true,
            center = ?center_tile,
            backend = ?render_backend,
            gpu_backend = ?render_gpu_backend,
            cache_policy = ?cache_policy,
            "map_viewer render_batch_start"
        );

        cx.spawn(async move |handle, cx| {
            let render_task = cx.background_spawn(async move {
                render_tile_batch_stream(tile_batch_request, event_sender)
            });

            let mut saw_complete = false;
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, TileRenderEvent::Complete { .. });
                let Some(view) = handle.upgrade() else {
                    render_cancel_for_owner.cancel();
                    return Ok(());
                };
                if let Err(error) = view.update(cx, move |this, cx| {
                    if this.render_generation != render_generation {
                        tracing::debug!(
                            request_id,
                            current_generation = this.render_generation,
                            event_generation = render_generation,
                            "map_viewer render_event_discarded"
                        );
                        return;
                    }

                    let mut notify_parent = true;
                    let colors = this.theme_colors(cx);
                    match event {
                        TileRenderEvent::ReadyBatch { tiles } => {
                            notify_parent = false;
                            let ready_count = tiles.len();
                            let mut changed_tiles = Vec::with_capacity(ready_count);
                            let retained_tiles = this.viewport_tile_plan().retain;
                            for ReadyTile {
                                coord,
                                tile,
                                source,
                            } in tiles
                            {
                                if !retained_tiles.contains(&coord) {
                                    this.drop_render_image_unless_current_tile(
                                        coord,
                                        tile.image.clone(),
                                        cx,
                                    );
                                    continue;
                                }
                                let cache_freshness = match source {
                                    TileReadySource::MemoryCache
                                    | TileReadySource::DiskCacheFresh => {
                                        Some(TileSourceFreshness::Fresh)
                                    }
                                    TileReadySource::DiskCacheStale => {
                                        Some(TileSourceFreshness::Stale)
                                    }
                                    TileReadySource::Render | TileReadySource::Preview => None,
                                };
                                if let Some(cache_freshness) = cache_freshness {
                                    let tile_image = tile.image.clone();
                                    let (accepted, dropped_image) = this
                                        .tile_manager
                                        .mark_loaded_from_cache_with_eviction(
                                            coord,
                                            tile,
                                            cache_freshness,
                                        );
                                    if accepted {
                                        changed_tiles.push(coord);
                                    } else {
                                        this.drop_render_image_unless_current_tile(
                                            coord,
                                            tile_image,
                                            cx,
                                        );
                                    }
                                    Self::drop_render_image(dropped_image, cx);
                                } else {
                                    if source == TileReadySource::Render {
                                        this.cold_rendered_tiles =
                                            this.cold_rendered_tiles.saturating_add(1);
                                    }
                                    Self::drop_render_image(
                                        this.tile_manager.mark_loaded(coord, tile),
                                        cx,
                                    );
                                    changed_tiles.push(coord);
                                }
                            }
                            this.tile_reveal_state.ready_batches =
                                this.tile_reveal_state.ready_batches.saturating_add(1);
                            this.tile_reveal_state.last_batch_size = ready_count;
                            this.trim_tiles_to_memory_budget(false, cx);
                            this.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
                            tracing::debug!(
                                request_id,
                                ready_count,
                                loaded = this.tile_manager.loaded_count(),
                                queued = this.tile_manager.queued_count(),
                                first_tile_ms = batch_started.elapsed().as_millis(),
                                "map_viewer render_ready_batch"
                            );
                        }
                        TileRenderEvent::Failed { coord, message } => {
                            notify_parent = false;
                            tracing::warn!(
                                request_id,
                                tile = ?coord,
                                %message,
                                "map_viewer render_tile_failed"
                            );
                            if message.contains("no renderable chunks")
                                || message.contains("没有可渲染区块")
                            {
                                if this
                                    .tile_chunk_index
                                    .get(&coord)
                                    .is_some_and(|positions| !positions.is_empty())
                                {
                                    Self::drop_render_image(
                                        this.tile_manager
                                            .mark_failed(coord, SharedString::from(message)),
                                        cx,
                                    );
                                } else {
                                    Self::drop_render_image(
                                        this.tile_manager
                                            .mark_invalid(coord, SharedString::from(message)),
                                        cx,
                                    );
                                }
                            } else {
                                Self::drop_render_image(
                                    this.tile_manager
                                        .mark_failed(coord, SharedString::from(message)),
                                    cx,
                                );
                            }
                            this.refresh_canvas_tiles_if_changed(&[coord], colors, cx);
                        }
                        TileRenderEvent::Empty { coord, message } => {
                            notify_parent = false;
                            tracing::trace!(
                                request_id,
                                tile = ?coord,
                                %message,
                                "map_viewer render_tile_empty"
                            );
                            Self::drop_render_image(
                                this.tile_manager
                                    .mark_invalid(coord, SharedString::from(message)),
                                cx,
                            );
                            this.refresh_canvas_tiles_if_changed(&[coord], colors, cx);
                        }
                        TileRenderEvent::Complete {
                            requested_tiles,
                            diagnostics,
                            stats,
                        } => {
                            this.finish_render_request(request_id, &requested_tiles);
                            this.last_ready_status_update = None;
                            let requested = requested_tiles.into_iter().collect::<BTreeSet<_>>();
                            let mut completion_changed_tiles = Vec::new();
                            for coord in requested {
                                if !matches!(
                                    this.tile_manager
                                        .entries
                                        .get(&coord)
                                        .map(|entry| entry.state),
                                    Some(
                                        TileLoadState::Loaded
                                            | TileLoadState::Queued
                                            | TileLoadState::Failed
                                            | TileLoadState::Invalid,
                                    )
                                ) {
                                    Self::drop_render_image(
                                        this.tile_manager
                                            .mark_failed(coord, SharedString::from("渲染未返回瓦片")),
                                        cx,
                                    );
                                    completion_changed_tiles.push(coord);
                                }
                            }
                            this.diagnostics.add(diagnostics);
                            this.render_stats = stats;
                            if !completion_changed_tiles.is_empty() {
                                this.refresh_canvas_tiles_if_changed(
                                    &completion_changed_tiles,
                                    colors,
                                    cx,
                                );
                            }
                            this.status = SharedString::from(format!(
                                "瓦片批次 {request_id} 完成 · 已加载 {} · 排队 {} · 冷渲染 {} · CPU {} · GPU {}（{}） · {} · 渲染缓存 命中 {} / 未命中 {} / 负缓存 {} / 读取 {}ms / 解压 {}ms / blob {}ms · 瓦片索引 T/V/M/E {}/{}/{}/{} · 依赖校验 {}ms · 写入丢弃 {} · 损坏 miss {} · 刷新 {} · 局部 chunk {} · 距离² {} · 区域缓存 {}/{} · 数据库 {}ms · 解码 {}ms · 合成 {}ms · GPU {}ms{}",
                                this.tile_manager.loaded_count(),
                                this.tile_manager.queued_count(),
                                this.cold_rendered_tiles,
                                this.render_stats.cpu_tiles,
                                this.render_stats.gpu_tiles,
                                this.render_stats.resolved_backend.label(),
                                gpu_status_text(&this.render_stats),
                                this.render_stats.cache_disk_fresh_hits,
                                this.render_stats.cache_misses,
                                this.render_stats.cache_empty_negative_hits,
                                this.render_stats.cache_read_ms,
                                this.render_stats.cache_decode_ms,
                                this.render_stats.tile_blob_decode_ms,
                                this.render_stats.tile_index_trusted_hits,
                                this.render_stats.tile_index_validated_hits,
                                this.render_stats.tile_index_misses,
                                this.render_stats.tile_index_empty_hits,
                                this.render_stats.tile_dep_validation_ms,
                                this.render_stats.tile_cache_writer_dropped,
                                this.render_stats.index_corrupt_misses,
                                this.refresh_rendered_tiles,
                                this.partial_refreshed_chunks,
                                this.last_queue_distance_squared,
                                this.render_stats.region_cache_hits,
                                this.render_stats.region_cache_misses,
                                this.render_stats.db_read_ms,
                                this.render_stats.cpu_decode_ms.max(this.render_stats.decode_ms),
                                this.render_stats.tile_compose_ms,
                                this.render_stats.gpu_dispatch_ms
                                    .saturating_add(this.render_stats.gpu_readback_ms),
                                this.render_stats
                                    .gpu_fallback_reason
                                    .as_ref()
                                    .map(|reason| format!(" · 回退 {reason}"))
                                    .unwrap_or_default()
                            ));
                            tracing::debug!(
                                request_id,
                                ui_batch_tiles = requested_tile_count,
                                ui_batch_chunks = batch_chunk_count,
                                ui_batch_regions = batch_region_count,
                                metadata_indexed_tiles,
                                unindexed_tiles,
                                loaded = this.tile_manager.loaded_count(),
                                queued = this.tile_manager.queued_count(),
                                failed = this.tile_manager.failed_count(),
                                cache_hits = this.render_stats.cache_hits,
                                cache_misses = this.render_stats.cache_misses,
                                region_cache_hits = this.render_stats.region_cache_hits,
                                region_cache_misses = this.render_stats.region_cache_misses,
                                tile_index_trusted_hits = this.render_stats.tile_index_trusted_hits,
                                tile_index_validated_hits = this.render_stats.tile_index_validated_hits,
                                tile_index_misses = this.render_stats.tile_index_misses,
                                tile_index_empty_hits = this.render_stats.tile_index_empty_hits,
                                tile_index_read_ms = this.render_stats.tile_index_read_ms,
                                tile_dep_validation_ms = this.render_stats.tile_dep_validation_ms,
                                tile_blob_decode_ms = this.render_stats.tile_blob_decode_ms,
                                tile_cache_writer_dropped = this.render_stats.tile_cache_writer_dropped,
                                world_signature_trusted = this.render_stats.world_signature_trusted,
                                world_signature_changed = this.render_stats.world_signature_changed,
                                index_corrupt_misses = this.render_stats.index_corrupt_misses,
                                cpu_tiles = this.render_stats.cpu_tiles,
                                gpu_tiles = this.render_stats.gpu_tiles,
                                gpu_backend = this.render_stats.resolved_backend.label(),
                                gpu_requested = ?this.render_stats.gpu_requested_backend,
                                gpu_actual = ?this.render_stats.gpu_actual_backend,
                                gpu_adapter = ?this.render_stats.gpu_adapter_name,
                                gpu_device = ?this.render_stats.gpu_device_name,
                                gpu_dispatch_ms = this.render_stats.gpu_dispatch_ms,
                                gpu_readback_ms = this.render_stats.gpu_readback_ms,
                                gpu_fallback = ?this.render_stats.gpu_fallback_reason,
                                exact_get_batches = this.render_stats.exact_get_batches,
                                exact_keys_requested = this.render_stats.exact_keys_requested,
                                exact_keys_found = this.render_stats.exact_keys_found,
                                render_prefix_scans = this.render_stats.render_prefix_scans,
                                db_read_ms = this.render_stats.db_read_ms,
                                decode_ms = this.render_stats.decode_ms,
                                cpu_decode_ms = this.render_stats.cpu_decode_ms,
                                cpu_frame_pack_ms = this.render_stats.cpu_frame_pack_ms,
                                tile_compose_ms = this.render_stats.tile_compose_ms,
                                "map_viewer render_batch_complete"
                            );
                            let pending_viewport_refresh = this.pending_viewport_refresh;
                            this.pending_viewport_refresh = false;
                            if pending_viewport_refresh
                                || this.has_current_viewport_work_or_pending_manifest()
                            {
                                this.ensure_visible_tiles(cx);
                            }
                        }
                    }
                    if notify_parent {
                        cx.notify();
                    }
                }) {
                    tracing::warn!(?error, "failed to merge map tile event");
                }
                if is_complete {
                    saw_complete = true;
                    break;
                }
            }

            let result = render_task.await;
            if let Err(error) = result {
                let requested_tiles = requested_tiles.clone();
                let Some(view) = handle.upgrade() else {
                    return Ok(());
                };
                if let Err(update_error) = view.update(cx, move |this, cx| {
                    if this.render_generation != render_generation {
                        tracing::debug!(
                            request_id,
                            current_generation = this.render_generation,
                            event_generation = render_generation,
                            "map_viewer render_error_discarded"
                        );
                        return;
                    }
                    this.finish_render_request(request_id, &requested_tiles);
                    let pending_viewport_refresh = this.pending_viewport_refresh;
                    this.pending_viewport_refresh = false;
                    let message = SharedString::from(error);
                    let mut changed_tiles = Vec::new();
                    for coord in requested_tiles {
                        if !matches!(
                            this.tile_manager
                                .entries
                                .get(&coord)
                                .map(|entry| entry.state),
                            Some(
                                TileLoadState::Loaded
                                    | TileLoadState::Queued
                                    | TileLoadState::Failed
                                    | TileLoadState::Invalid,
                            )
                        ) {
                            Self::drop_render_image(
                                this.tile_manager.mark_failed(coord, message.clone()),
                                cx,
                            );
                            changed_tiles.push(coord);
                        }
                    }
                    let colors = this.theme_colors(cx);
                    this.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
                    this.status = message;
                    tracing::warn!(request_id, status = %this.status, "map_viewer render_batch_error");
                    if pending_viewport_refresh || this.has_current_viewport_work_or_pending_manifest()
                    {
                        this.ensure_visible_tiles(cx);
                    }
                    cx.notify();
                }) {
                    tracing::warn!(?update_error, "failed to merge map tile render error");
                }
            } else if !saw_complete {
                let Some(view) = handle.upgrade() else {
                    return Ok(());
                };
                if let Err(error) = view.update(cx, move |this, cx| {
                    if this.render_generation != render_generation {
                        return;
                    }
                    this.finish_render_request(request_id, &requested_tiles);
                    let pending_viewport_refresh = this.pending_viewport_refresh;
                    this.pending_viewport_refresh = false;
                    this.status = SharedString::from(format!(
                        "瓦片批次 {request_id} 已结束 · 已加载 {} · 排队 {} · CPU 瓦片 {}",
                        this.tile_manager.loaded_count(),
                        this.tile_manager.queued_count(),
                        this.render_stats.cpu_tiles
                    ));
                    if pending_viewport_refresh || this.has_current_viewport_work_or_pending_manifest()
                    {
                        this.ensure_visible_tiles(cx);
                    }
                    cx.notify();
                }) {
                    tracing::warn!(?error, "failed to finalize map tile batch");
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
        if self.has_render_batch_capacity() && self.tile_manager.queued_count() > 0 {
            self.schedule_next_tile_batch(cx);
        }
    }
}

fn tile_coords_for_bounds(visible: TileBounds, radius: i32, center: (i32, i32)) -> Vec<(i32, i32)> {
    let mut expanded = visible.expand(radius);
    clamp_tile_span(&mut expanded.min_x, &mut expanded.max_x, center.0);
    clamp_tile_span(&mut expanded.min_z, &mut expanded.max_z, center.1);
    collect_circular_tile_coords(visible, expanded, radius, center)
}

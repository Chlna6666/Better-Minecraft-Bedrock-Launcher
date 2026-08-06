use super::helpers::*;
use super::model::*;
use super::prelude::*;
use super::tile_cache::decoded_tile_byte_len;
use super::tile_occupancy::*;
use super::tile_plan::*;
use super::tile_render::*;
use super::tile_state::*;
use super::viewport::*;

const MAP_VIEWER_TASK_STAGE_LABELS: [(&str, &str); 7] = [
    ("map_export", "地图导出"),
    ("map_import", "地图导入"),
    ("map_copy", "复制区块"),
    ("map_paste", "粘贴区块"),
    ("map_delete", "删除区块"),
    ("map_write", "写入地图"),
    ("map_refresh", "刷新地图瓦片"),
];
// Submit original world-tile images directly to GPUI. The retained tile layer already
// publishes ready batches incrementally and budgets new GPU uploads per frame, while keeping
// the one-block-per-pixel source images reusable across zoom levels without viewport resampling.
pub(super) const VIEWPORT_COMPOSITE_ENABLED: bool = false;

pub(super) const fn viewport_composite_owns_viewport(
    viewport_dragging: bool,
    composite_request_active: bool,
    composite_frame_available: bool,
) -> bool {
    VIEWPORT_COMPOSITE_ENABLED
        && !viewport_dragging
        && (composite_request_active || composite_frame_available)
}

pub(super) fn visible_tile_foreground_work_limit(is_interacting: bool) -> usize {
    if is_interacting {
        INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT
    } else {
        VISIBLE_TILE_FOREGROUND_WORK_LIMIT
    }
    .max(1)
}

pub(super) fn visible_tile_needs_foreground_work(
    tile_chunk_index: &TileChunkIndex,
    tile_manager: &RegionManager,
    coord: (i32, i32),
) -> bool {
    match tile_chunk_index.get(&coord) {
        Some(positions) if positions.is_empty() => !tile_manager
            .entries
            .get(&coord)
            .is_some_and(|entry| {
                matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)
            }),
        Some(_) => !tile_manager.entries.get(&coord).is_some_and(|entry| {
            matches!(entry.state, TileLoadState::Queued | TileLoadState::Loading)
                || (entry.state == TileLoadState::Loaded && entry.image.is_some())
                || matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)
        }),
        None => !tile_manager.entries.get(&coord).is_some_and(|entry| {
            matches!(
                entry.state,
                TileLoadState::Empty
                    | TileLoadState::Queued
                    | TileLoadState::Loading
                    | TileLoadState::Loaded
                    | TileLoadState::Invalid
            )
        }),
    }
}

pub(super) fn has_unregistered_visible_tile_work(
    visible_tiles: &[(i32, i32)],
    tile_chunk_index: &TileChunkIndex,
    tile_manager: &RegionManager,
) -> bool {
    visible_tiles
        .iter()
        .copied()
        .any(|coord| visible_tile_needs_foreground_work(tile_chunk_index, tile_manager, coord))
}

pub(super) const fn should_defer_render_image_evictions(viewport_interacting: bool) -> bool {
    viewport_interacting
}

pub(super) const fn should_notify_parent_after_interaction_layer_sync() -> bool {
    true
}

pub(super) fn should_yield_after_ready_batch(
    quick_reveal: bool,
    viewport_interacting: bool,
    event: &TileRenderEvent,
) -> bool {
    let TileRenderEvent::ReadyBatch { tiles } = event else {
        return false;
    };
    !tiles.is_empty()
        && (viewport_interacting
            || quick_reveal
            || tiles.iter().any(|tile| {
                matches!(
                    tile.source,
                    TileReadySource::MemoryCache
                        | TileReadySource::DiskCacheFresh
                        | TileReadySource::DiskCacheStale
                )
            }))
}

pub(super) fn tile_event_needs_window_refresh(event: &TileRenderEvent) -> bool {
    matches!(
        event,
        TileRenderEvent::ReadyBatch { .. }
            | TileRenderEvent::Failed { .. }
            | TileRenderEvent::Empty { .. }
    )
}

pub(super) fn render_batch_matches_current_viewport(
    request_center: (i32, i32),
    current_center: (i32, i32),
    visible_bounds: TileBounds,
) -> bool {
    request_center == current_center && tile_bounds_contains(visible_bounds, request_center)
}

pub(super) fn tile_request_intersects_plan(
    requested_tiles: &[(i32, i32)],
    visible_bounds: Option<TileBounds>,
    retain_filter: Option<RetainedTileFilter>,
) -> bool {
    requested_tiles.iter().copied().any(|coord| {
        retain_filter.is_some_and(|filter| filter.contains(coord))
            || visible_bounds.is_some_and(|bounds| bounds.contains(coord))
    })
}

impl MapViewerWindowView {
    pub(super) fn viewport_drag_active(&self) -> bool {
        self.drag.is_some() || self.ui_state.dock_drag.is_some()
    }

    pub(super) fn viewport_interaction_active(&self) -> bool {
        self.viewport_drag_active()
            || self.right_selection_drag.is_some()
            || self.viewport_recently_interacted()
    }

    pub(super) fn mark_viewport_interaction(&mut self) {
        self.last_viewport_interaction = Some(Instant::now());
    }

    pub(super) fn mark_viewport_render_dirty(&mut self) {
        // Keep in-flight tile work alive while the camera moves. The next viewport
        // plan cancels only requests outside its retain filter; cancelling every
        // request here creates holes and repeatedly re-renders the same tiles.
        if self.viewport_composite_request_id.is_some() {
            self.viewport_composite_signature = None;
        }
        self.pending_viewport_refresh = true;
    }

    pub(super) fn prepare_viewport_images_for_zoom(&mut self, _cx: &mut Context<Self>) -> bool {
        if !VIEWPORT_COMPOSITE_ENABLED {
            return false;
        }
        self.viewport_zoom_transition = true;
        // Invalidate the old request before the debounce window can deliver a
        // stale tile. Its event is dropped instead of being painted at the new
        // zoom level; the previous frame remains visible until replacement.
        self.viewport_composite_signature = None;
        self.pending_viewport_refresh = true;
        true
    }

    pub(super) fn viewport_recently_interacted(&self) -> bool {
        self.last_viewport_interaction
            .is_some_and(|last_interaction| {
                last_interaction.elapsed() < VIEWPORT_INTERACTION_IDLE_DELAY
            })
    }

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
        let mut delay = next_ready_at.saturating_duration_since(Instant::now());
        if should_defer_render_image_evictions(self.viewport_interaction_active()) {
            delay = delay.max(VIEWPORT_INTERACTION_IDLE_DELAY);
        }
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
        if should_defer_render_image_evictions(self.viewport_interaction_active()) {
            self.schedule_pending_render_image_evictions(cx);
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
        task_manager::register_task_stage_labels(MAP_VIEWER_TASK_STAGE_LABELS);
        let world_path = PathBuf::from(init.world_path.as_ref());
        let window_size = window.viewport_size();
        let mut viewport = MapViewport::new(window_size);
        if let Some((spawn_x, spawn_z)) = spawn_block_center(&world_path) {
            viewport.center_on_block(spawn_x, spawn_z, web_relief_render_layout());
        }
        let input_fields = MapInputFields::new(window, cx);
        let initial_layout = web_relief_render_layout();
        let (initial_center_x, initial_center_z) = viewport.center_block(initial_layout);
        initialize_map_input_values(
            &input_fields,
            [
                (MapInputField::CenterX, initial_center_x.to_string()),
                (MapInputField::CenterZ, initial_center_z.to_string()),
                (
                    MapInputField::ZoomPercent,
                    format!("{:.0}", viewport.scale * 100.0),
                ),
                (
                    MapInputField::DimensionId,
                    Dimension::Overworld.id().to_string(),
                ),
            ],
            window,
            cx,
        );
        let editor_state = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor
        });
        let mut subscriptions = vec![cx.observe_window_bounds(window, |this, window, cx| {
            if this.update_viewport_size(window) {
                this.invalidate_professional_overlay_for_viewport_change();
                this.ensure_visible_tiles(cx);
                this.refresh_professional_render_caches(cx);
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
            active_layout: initial_layout,
            // Keep one stable texture layout for the lifetime of the view. Camera
            // zoom changes only origin/scale, so already decoded images stay valid
            // and the interaction path never swaps in a blurry intermediate LOD.
            render_texture_layout: initial_layout,
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
            task_snapshots: task_manager::snapshot_arcs_map().into_iter().collect(),
            pending_paste_task_completion: None,
            task_updates_task: None,
            frame_stats: FrameStats::default(),
            tile_reveal_state: TileRevealState::default(),
            available_tiles: BTreeSet::new(),
            tile_chunk_index: BTreeMap::new(),
            tile_occupancy_index: None,
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
            session_loading: false,
            render_batch_active: false,
            request_id: 0,
            metadata_generation: 0,
            session_generation: 0,
            render_generation: 0,
            metadata_cancel: None,
            render_cancels: BTreeMap::new(),
            active_render_tiles: ActiveRenderTiles::default(),
            active_render_center_tiles: BTreeMap::new(),
            active_render_request_tiles: BTreeMap::new(),
            pending_viewport_refresh: false,
            viewport_work_refresh_scheduled: false,
            viewport_idle_generation: 0,
            viewport_idle_task: None,
            physical_render_batches: PhysicalRenderBatchBudget::default(),
            map_query_budget: MapQueryBudget::default(),
            last_viewport_interaction: None,
            last_viewport_tile_sync: None,
            last_drag_canvas_snapshot_sync: None,
            pending_interaction_ready_tiles: Vec::new(),
            last_interaction_ready_flush: None,
            last_visible_tile_log: None,
            last_tile_memory_trim: None,
            last_memory_snapshot_record: None,
            pending_render_image_evictions: Vec::new(),
            pending_render_image_eviction_generation: 0,
            last_visible_tile_signature: None,
            viewport_plan_generation: 0,
            viewport_composite_signature: None,
            viewport_composite_request_id: None,
            viewport_zoom_transition: false,
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
        this.preload_entity_avatar_pool(cx);
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
        self.viewport.set_size(self.center_stage_size(window_size))
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
        if !self.viewport_interaction_active() {
            self.flush_pending_interaction_ready_tiles_if_due(colors, cx);
            self.refresh_canvas_tiles_for_current_viewport_if_needed(cx);
        }
        let snapshot_key = self.canvas_snapshot_key(colors);
        if self.last_synced_canvas_snapshot_key.as_ref() == Some(&snapshot_key) {
            return;
        }
        self.last_synced_canvas_snapshot_key = Some(snapshot_key);
        self.last_synced_tile_layer_snapshot_key = Some(self.tile_layer_snapshot_key(colors));
        let snapshot = self.canvas_snapshot(colors);
        let canvas_view = self.canvas_view.clone();
        canvas_view.update(cx, |view, cx| view.set_snapshot(snapshot, cx));
        self.record_memory_snapshot_if_due();
    }

    pub(super) fn sync_tile_layer_snapshot(&mut self, colors: ThemeColors, cx: &mut Context<Self>) {
        if !self.viewport_interaction_active() {
            self.refresh_canvas_tiles_for_current_viewport_if_needed(cx);
        }
        self.commit_tile_layer_snapshot(colors, cx);
    }

    pub(super) fn sync_interaction_tile_layer_snapshot(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) -> bool {
        // Interaction changes the camera before the idle refresh runs. Rebuild the
        // retained paint snapshot when its tile bounds no longer cover the camera;
        // otherwise the canvas keeps drawing the pre-zoom snapshot and exposes
        // transparent gaps until the next completed render batch.
        self.refresh_canvas_tiles_for_current_viewport_if_needed(cx);
        let canvas_view = self.canvas_view.clone();
        let snapshot = self.canvas_snapshot(colors);
        canvas_view.update(cx, |view, cx| view.set_interaction_snapshot(snapshot, cx));
        self.last_synced_tile_layer_snapshot_key = Some(self.tile_layer_snapshot_key(colors));
        self.record_memory_snapshot_if_due();
        true
    }

    fn commit_tile_layer_snapshot(&mut self, colors: ThemeColors, cx: &mut Context<Self>) -> bool {
        let snapshot_key = self.tile_layer_snapshot_key(colors);
        if self.last_synced_tile_layer_snapshot_key.as_ref() == Some(&snapshot_key) {
            return false;
        }
        self.last_synced_tile_layer_snapshot_key = Some(snapshot_key);
        let canvas_view = self.canvas_view.clone();
        let viewport = self.viewport;
        let layout = self.active_layout;
        let overlays = self.overlay_options;
        let dragging = self.viewport_interaction_active();
        let tiles = self.canvas_tile_snapshot.clone();
        canvas_view.update(cx, |view, cx| {
            view.set_tile_snapshot(viewport, layout, colors, overlays, dragging, tiles, cx)
        });
        self.record_memory_snapshot_if_due();
        true
    }

    fn canvas_paint_radius(&self) -> i32 {
        if self.viewport_drag_active() {
            DRAG_RETAIN_RADIUS
        } else {
            RETAIN_RADIUS
        }
    }

    fn current_canvas_paint_bounds(&self) -> Option<TileBounds> {
        paint_tile_bounds_for_viewport(
            self.viewport,
            self.active_layout,
            self.canvas_paint_radius(),
        )
    }

    fn refresh_canvas_tiles_for_current_viewport_if_needed(&mut self, cx: &mut Context<Self>) {
        self.refresh_canvas_tiles_for_current_viewport_if_needed_impl(
            self.toolbar_state.diagnostics_open,
            true,
            false,
            cx,
        );
    }

    fn refresh_canvas_tiles_for_current_viewport_if_needed_impl(
        &mut self,
        include_debug_overlays: bool,
        rebuild_paste_preview_images: bool,
        force_rebuild: bool,
        cx: &mut Context<Self>,
    ) {
        if VIEWPORT_COMPOSITE_ENABLED {
            if rebuild_paste_preview_images {
                self.rebuild_paste_preview_images(cx);
            }
            return;
        }
        let paint_bounds = self.current_canvas_paint_bounds();
        if !force_rebuild && self.canvas_tile_snapshot.paint_bounds == paint_bounds {
            return;
        }
        if !self.canvas_tile_snapshot.screen_images.is_empty() && self.uses_viewport_composite() {
            return;
        }
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        let snapshot = build_tile_paint_snapshot(
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            include_debug_overlays,
            self.canvas_paint_radius(),
            self.canvas_tile_generation,
        );
        self.replace_canvas_tile_snapshot(snapshot, cx);
        if rebuild_paste_preview_images {
            self.rebuild_paste_preview_images(cx);
        }
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
    }

    pub(super) fn clear_canvas_tile_snapshot(&mut self, cx: &mut Context<Self>) {
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.viewport_zoom_transition = false;
        self.replace_canvas_tile_snapshot(
            TilePaintSnapshot {
                generation: self.canvas_tile_generation,
                ..TilePaintSnapshot::default()
            },
            cx,
        );
        self.viewport_composite_signature = None;
        self.viewport_composite_request_id = None;
        self.pending_interaction_ready_tiles.clear();
        self.last_interaction_ready_flush = None;
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
    }

    fn replace_canvas_tile_snapshot(
        &mut self,
        snapshot: TilePaintSnapshot,
        cx: &mut Context<Self>,
    ) {
        let dropped_screen_images = self
            .canvas_tile_snapshot
            .screen_images
            .iter()
            .map(|image| image.image.clone())
            .collect::<Vec<_>>();
        self.canvas_tile_snapshot = Arc::new(snapshot);
        Self::drop_render_images(dropped_screen_images, cx);
    }

    pub(super) fn record_memory_snapshot_if_due(&mut self) {
        let now = Instant::now();
        if !memory_snapshot_due(self.last_memory_snapshot_record, now) {
            return;
        }
        self.last_memory_snapshot_record = Some(now);
        self.record_memory_snapshot();
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
                canvas_snapshot_tile_count: self
                    .canvas_tile_snapshot
                    .tiles
                    .len()
                    .saturating_add(self.canvas_tile_snapshot.screen_images.len()),
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
        if VIEWPORT_COMPOSITE_ENABLED {
            self.rebuild_paste_preview_images(cx);
            self.viewport_composite_signature = None;
            self.pending_viewport_refresh = true;
            self.sync_canvas_snapshot(colors, cx);
            self.schedule_viewport_work_refresh(cx);
            return;
        }
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        let snapshot = build_tile_paint_snapshot(
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            self.toolbar_state.diagnostics_open,
            self.canvas_paint_radius(),
            self.canvas_tile_generation,
        );
        self.replace_canvas_tile_snapshot(snapshot, cx);
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
        self.replace_canvas_tile_snapshot(
            TilePaintSnapshot {
                tiles: Arc::new(tiles),
                screen_images: Arc::new(Vec::new()),
                debug_overlays: Arc::new(debug_overlays),
                generation: self.canvas_tile_generation,
                estimated_bytes,
                paint_bounds: current.paint_bounds,
            },
            cx,
        );
        self.rebuild_paste_preview_images(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    fn prune_canvas_tile_snapshot_to_current_tiles(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        let current = self.canvas_tile_snapshot.clone();
        if current.tiles.is_empty() && current.debug_overlays.is_empty() {
            return;
        }

        let tiles = current
            .tiles
            .iter()
            .filter(|tile| {
                self.tile_manager
                    .entries
                    .get(&tile.coord)
                    .and_then(|entry| entry.image.as_ref())
                    .is_some_and(|current| Arc::ptr_eq(&current.image, &tile.image))
            })
            .cloned()
            .collect::<Vec<_>>();
        let debug_overlays = current
            .debug_overlays
            .iter()
            .filter(|overlay| {
                self.tile_manager
                    .entries
                    .get(&overlay.coord)
                    .is_some_and(|entry| {
                        matches!(entry.state, TileLoadState::Failed | TileLoadState::Invalid)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if tiles.len() == current.tiles.len()
            && debug_overlays.len() == current.debug_overlays.len()
        {
            return;
        }

        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        let estimated_bytes = tiles
            .iter()
            .map(|tile| tile.estimated_bytes)
            .sum::<usize>()
            .saturating_add(
                current
                    .screen_images
                    .iter()
                    .map(|image| image.estimated_bytes)
                    .sum::<usize>(),
            );
        self.replace_canvas_tile_snapshot(
            TilePaintSnapshot {
                tiles: Arc::new(tiles),
                screen_images: current.screen_images.clone(),
                debug_overlays: Arc::new(debug_overlays),
                generation: self.canvas_tile_generation,
                estimated_bytes,
                paint_bounds: current.paint_bounds,
            },
            cx,
        );
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
        if self.viewport_interaction_active() {
            self.sync_interaction_tile_layer_snapshot(colors, cx);
        } else {
            self.sync_tile_layer_snapshot(colors, cx);
        }
    }

    pub(super) fn refresh_canvas_tiles_if_changed(
        &mut self,
        changed_tiles: &[(i32, i32)],
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        if VIEWPORT_COMPOSITE_ENABLED {
            if changed_tiles.is_empty() {
                return;
            }
            self.viewport_composite_signature = None;
            self.pending_viewport_refresh = true;
            self.sync_canvas_snapshot(colors, cx);
            self.schedule_viewport_work_refresh(cx);
            return;
        }
        let paint_bounds = self.current_canvas_paint_bounds();
        let affects_visible = paint_bounds.is_none_or(|bounds| {
            changed_tiles
                .iter()
                .any(|coord| tile_bounds_contains(bounds, *coord))
        });
        if !affects_visible {
            return;
        }

        let snapshot_was_empty = self.canvas_tile_snapshot.tiles.is_empty()
            && self.canvas_tile_snapshot.screen_images.is_empty();
        let generation = self.canvas_tile_generation.saturating_add(1);
        match patch_tile_paint_snapshot(
            &self.canvas_tile_snapshot,
            &self.tile_manager,
            self.viewport,
            self.active_layout,
            self.toolbar_state.diagnostics_open,
            self.canvas_paint_radius(),
            changed_tiles,
            generation,
        ) {
            TilePaintSnapshotPatch::Unchanged => {}
            TilePaintSnapshotPatch::Patched(snapshot) => {
                self.canvas_tile_generation = generation;
                self.replace_canvas_tile_snapshot(snapshot, cx);
                if snapshot_was_empty {
                    // A cache hit can arrive before the initial canvas snapshot is published.
                    // Rebuild from the current viewport once instead of retaining a partial
                    // incremental patch from the first ready batch.
                    self.refresh_canvas_tiles_for_current_viewport_if_needed_impl(
                        self.toolbar_state.diagnostics_open,
                        false,
                        true,
                        cx,
                    );
                    self.sync_canvas_snapshot(colors, cx);
                } else {
                    self.sync_tile_layer_snapshot(colors, cx);
                }
            }
            TilePaintSnapshotPatch::Rebuild => self.refresh_canvas_tiles(colors, cx),
        }
    }

    fn refresh_interaction_canvas_tiles_after_ready_batch(&mut self, changed_tiles: &[(i32, i32)]) {
        if changed_tiles.is_empty() {
            return;
        }
        self.pending_viewport_refresh = true;
        self.pending_interaction_ready_tiles
            .extend(changed_tiles.iter().copied());
    }

    fn flush_pending_interaction_ready_tiles_if_due(
        &mut self,
        colors: ThemeColors,
        cx: &mut Context<Self>,
    ) {
        if self.pending_interaction_ready_tiles.is_empty() {
            return;
        }
        if self.viewport_interaction_active() {
            return;
        }
        self.last_interaction_ready_flush = Some(Instant::now());
        let mut changed_tiles = std::mem::take(&mut self.pending_interaction_ready_tiles);
        changed_tiles.sort_unstable();
        changed_tiles.dedup();
        self.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
    }

    pub(super) fn canvas_snapshot(&self, colors: ThemeColors) -> MapCanvasSnapshot {
        MapCanvasSnapshot {
            stage_origin: self.center_stage_origin(),
            viewport: self.viewport,
            layout: self.active_layout,
            dimension: self.dimension,
            colors,
            overlays: self.overlay_options,
            dragging: self.viewport_interaction_active(),
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
            overlays: self.overlay_options,
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
        self.active_render_center_tiles.clear();
        self.active_render_request_tiles.clear();
        self.viewport_composite_request_id = None;
        self.viewport_composite_signature = None;
        self.pending_viewport_refresh = false;
    }

    fn has_render_batch_capacity(&self) -> bool {
        self.active_render_batch_count() < map_concurrent_render_batches()
    }

    fn has_quick_reveal_render_batch_capacity(&self) -> bool {
        self.has_render_batch_capacity()
    }

    fn active_render_batch_count(&self) -> usize {
        self.physical_render_batches.active()
    }

    fn acquire_render_batch(&self) -> Option<PhysicalRenderBatchPermit> {
        self.physical_render_batches
            .try_acquire(map_concurrent_render_batches())
    }

    fn resume_render_work_after_physical_batch(&mut self, cx: &mut Context<Self>) {
        if !self.has_render_batch_capacity() {
            return;
        }
        if self.pending_viewport_refresh {
            self.schedule_viewport_work_refresh(cx);
        } else if self.tile_manager.queued_count() > 0 {
            self.schedule_next_tile_batch(cx);
        }
    }

    fn render_request_is_current(
        &self,
        request_id: u64,
        render_generation: u64,
        event: &'static str,
    ) -> bool {
        if self.render_generation != render_generation {
            tracing::debug!(
                request_id,
                current_generation = self.render_generation,
                event_generation = render_generation,
                event,
                "map_viewer render_event_discarded"
            );
            return false;
        }
        if !self.render_cancels.contains_key(&request_id) {
            tracing::debug!(request_id, event, "map_viewer render_event_stale_request");
            return false;
        }
        true
    }

    fn cancel_render_batches_outside_visible_bounds(
        &mut self,
        visible_bounds: TileBounds,
        current_center: (i32, i32),
    ) -> usize {
        self.cancel_render_batches_outside_tile_plan(Some(visible_bounds), None, current_center)
    }

    fn cancel_render_batches_outside_tile_plan(
        &mut self,
        visible_bounds: Option<TileBounds>,
        retain_filter: Option<RetainedTileFilter>,
        current_center: (i32, i32),
    ) -> usize {
        let stale_request_ids = self
            .active_render_center_tiles
            .iter()
            .filter_map(|(request_id, request_center)| {
                let intersects_plan = self
                    .active_render_request_tiles
                    .get(request_id)
                    .map(|requested_tiles| {
                        tile_request_intersects_plan(requested_tiles, visible_bounds, retain_filter)
                    })
                    .unwrap_or_else(|| {
                        visible_bounds.is_some_and(|bounds| {
                            render_batch_matches_current_viewport(
                                *request_center,
                                current_center,
                                bounds,
                            )
                        })
                    });
                (!intersects_plan).then_some(*request_id)
            })
            .collect::<Vec<_>>();

        let mut cancelled_count = 0usize;
        for request_id in stale_request_ids {
            if let Some(cancel) = self.render_cancels.get(&request_id)
                && !cancel.is_cancelled()
            {
                cancel.cancel();
                cancelled_count = cancelled_count.saturating_add(1);
            }
        }
        cancelled_count
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
        track_active_render_tiles(&mut self.active_render_tiles, requested_tiles);
        self.active_render_center_tiles
            .insert(request_id, center_tile);
        self.active_render_request_tiles
            .insert(request_id, requested_tiles.to_vec());
    }

    fn finish_render_request(&mut self, request_id: u64, requested_tiles: &[(i32, i32)]) {
        self.render_cancels.remove(&request_id);
        finish_active_render_tiles(&mut self.active_render_tiles, requested_tiles);
        self.active_render_center_tiles.remove(&request_id);
        self.active_render_request_tiles.remove(&request_id);
        self.render_batch_active = !self.render_cancels.is_empty();
    }
    pub(super) fn cancel_metadata_scan(&mut self) {
        cancel_metadata_flag(&mut self.metadata_cancel);
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

    fn retire_render_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.render_session.take() else {
            return;
        };
        cx.background_spawn(async move {
            drop(session);
        })
        .detach();
    }

    pub(super) fn refresh_render_session(&mut self, cx: &mut Context<Self>) {
        self.session_generation = self.session_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_metadata_scan();
        self.cancel_active_render();
        self.retire_render_session(cx);
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

        // The cache identity check does not need an open LevelDB handle. Run it beside session
        // creation so a warm start pays the slower of the two costs instead of their sum.
        self.refresh_metadata(cx);

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
                        this.status = if this.metadata_loading {
                            SharedString::from("地图渲染会话就绪 · 正在校验本地瓦片缓存")
                        } else {
                            SharedString::from("地图渲染会话就绪 · 正在加载当前视口")
                        };
                        this.clear_visible_error();
                        tracing::debug!(generation, "map_viewer session_open_ok");
                        this.ensure_visible_tiles(cx);
                        this.refresh_metadata_consumers_if_ready(cx);
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
        self.retire_render_session(cx);
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
                        this.finish_pending_paste_task(Some(&error));
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
        if !self.has_render_batch_capacity() {
            let fallback_tiles = requests
                .iter()
                .map(|request| request.coord)
                .collect::<Vec<_>>();
            Self::drop_render_images(
                self.tile_manager
                    .force_refresh_tiles(&fallback_tiles, tile_priority),
                cx,
            );
            self.last_visible_tile_signature = None;
            self.pending_viewport_refresh = true;
            self.schedule_next_tile_batch(cx);
            return;
        }
        let request_id = self.request_id.saturating_add(1);
        self.request_id = request_id;
        let render_generation = self.render_generation;
        let mode = self.current_render_mode();
        let layout = self.render_texture_layout;
        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let requested_tiles = requests
            .iter()
            .map(|request| request.coord)
            .collect::<Vec<_>>();
        let total_chunks = requests
            .iter()
            .map(|request| request.chunks.len())
            .sum::<usize>();
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
            total_chunks
        ));
        cx.notify();
        let requested_tiles_for_finish = requested_tiles.clone();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut results = Vec::new();
                    let mut failed_tiles = BTreeSet::new();
                    let mut cancelled = false;
                    for plan in requests {
                        if render_cancel_for_task.is_cancelled() {
                            render_cancel_for_task.cancel();
                            cancelled = true;
                            break;
                        }
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
                            Ok(result) => {
                                results.push(result);
                            }
                            Err(error) => {
                                if render_cancel_for_task.is_cancelled() {
                                    cancelled = true;
                                    break;
                                }
                                tracing::debug!(
                                    tile = ?coord,
                                    %error,
                                    "map_viewer chunk_patch_refresh_failed"
                                );
                                failed_tiles.insert(coord);
                            }
                        }
                    }
                    (results, failed_tiles, cancelled)
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if !this.render_request_is_current(request_id, render_generation, "chunk_patch") {
                    return;
                }
                this.finish_render_request(request_id, &requested_tiles_for_finish);
                let colors = this.theme_colors(cx);
                let (results, failed_tiles, cancelled) = result;
                if cancelled {
                    this.status = SharedString::from("地图局部刷新已取消");
                    this.schedule_next_tile_batch(cx);
                    cx.notify();
                    return;
                }
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
                this.finish_paste_preview_tile_refresh(cx);
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
    }

fn refresh_metadata_consumers_if_ready(&mut self, cx: &mut Context<Self>) {
        if self.render_session.is_none() || self.metadata_loading {
            return;
        }
        self.refresh_professional_render_caches(cx);
        self.refresh_professional_overlays(cx);
    }

    pub(super) fn invalidate_tiles(&mut self, cx: &mut Context<Self>) {
        self.render_generation = self.render_generation.saturating_add(1);
        self.cancel_metadata_scan();
        self.cancel_active_render();
        Self::drop_render_images(self.tile_manager.clear(), cx);
        self.clear_pending_render_image_evictions(cx);
        self.clear_canvas_tile_snapshot(cx);
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
        if VIEWPORT_COMPOSITE_ENABLED || self.uses_viewport_composite() {
            self.last_visible_tile_signature = None;
            self.pending_viewport_refresh = false;
            self.schedule_next_tile_batch(cx);
            return;
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
            retain_filter: tile_plan.retain_filter,
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
        if signature_changed {
            self.viewport_plan_generation = self.viewport_plan_generation.saturating_add(1);
            let cancelled = self.cancel_render_batches_outside_tile_plan(
                tile_plan.visible_bounds,
                tile_plan.retain_filter,
                tile_plan.center,
            );
            if cancelled > 0 {
                tracing::debug!(
                    generation = self.viewport_plan_generation,
                    cancelled,
                    "map_viewer cancelled_outdated_tile_batches"
                );
            }
        }
        if !signature_changed && self.render_batch_active && !self.has_render_batch_capacity() {
            self.pending_viewport_refresh = true;
            return;
        }
        self.last_visible_tile_signature = Some(visible_signature);
        let visible_tile_set = tile_plan.visible.iter().copied().collect::<BTreeSet<_>>();
        self.tile_manager
            .reconcile_viewport_priorities(&visible_tile_set);
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
        if deferred_visible_work {
            self.pending_viewport_refresh = true;
            self.schedule_viewport_work_refresh(cx);
        }

        if self.render_batch_active && self.tile_manager.has_visible_work() {
            self.pending_viewport_refresh = true;
        } else if !deferred_visible_work {
            self.pending_viewport_refresh = false;
        }
        if !tile_plan.is_interacting {
            // The retained radius is a warm-cache hint, not an eviction boundary. A hard
            // viewport prune makes a short drag away and back behave like a cold load.
            // Protect only the current viewport here and let the LRU choose older warm tiles
            // when the actual memory/capacity limit is reached.
            // Keep the camera overscan in the cache. Zooming back or reversing a drag
            // must reuse the images that were just rendered instead of exposing a
            // cleared viewport while those tiles are rendered again.
            let retain_filter = tile_plan.retain_filter;
            self.trim_tiles_to_memory_budget_for_filter(retain_filter, true, cx);
            self.trim_tile_entries_to_capacity_for_filter(retain_filter, cx);
        }
        self.schedule_next_tile_batch(cx);
        if !tile_plan.is_interacting {
            self.refresh_professional_overlays(cx);
        }
    }

    fn resolve_occupancy_tiles(
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

    pub(super) fn schedule_viewport_work_refresh(&mut self, cx: &mut Context<Self>) {
        if self.viewport_work_refresh_scheduled {
            return;
        }
        self.viewport_work_refresh_scheduled = true;
        cx.spawn(async move |handle, cx| {
            Timer::after(VIEWPORT_WORK_REFRESH_INTERVAL).await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, |this, cx| {
                this.viewport_work_refresh_scheduled = false;
                if !this.pending_viewport_refresh || this.right_selection_drag.is_some() {
                    return;
                }
                if this.viewport_drag_active() {
                    this.ensure_visible_tiles_for_viewport_drag(cx);
                } else {
                    this.ensure_visible_tiles(cx);
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }


pub(super) fn ensure_visible_tiles_throttled(&mut self, cx: &mut Context<Self>) {
        if self.right_selection_drag.is_some() {
            self.pending_viewport_refresh = true;
            return;
        }
        let now = Instant::now();
        let sync_interval = VIEWPORT_TILE_SYNC_INTERVAL;
        let should_sync = self
            .last_viewport_tile_sync
            .is_none_or(|last_sync| now.saturating_duration_since(last_sync) >= sync_interval);
        if should_sync {
            if self.viewport_drag_active() {
                self.ensure_visible_tiles_for_viewport_drag(cx);
            } else {
                self.ensure_visible_tiles(cx);
            }
        } else {
            self.pending_viewport_refresh = true;
            self.schedule_viewport_work_refresh(cx);
        }
    }

    fn ensure_visible_tiles_for_viewport_drag(&mut self, cx: &mut Context<Self>) {
        self.last_viewport_tile_sync = Some(Instant::now());
        if self.render_session.is_none() {
            if !self.session_loading {
                self.refresh_render_session(cx);
            }
            return;
        }
        if VIEWPORT_COMPOSITE_ENABLED {
            self.pending_viewport_refresh = true;
            self.schedule_next_tile_batch(cx);
            return;
        }

        let center = self.viewport.center_tile(self.active_layout);
        let Some(visible_bounds) =
            visible_tile_bounds_for_viewport(self.viewport, self.active_layout, center)
        else {
            return;
        };
        let visible_tiles = tile_coords_for_visible_bounds(visible_bounds, center);
        if visible_tiles.is_empty() {
            return;
        }

        let visible_tile_set = visible_tiles.iter().copied().collect::<BTreeSet<_>>();
        self.tile_manager
            .reconcile_viewport_priorities(&visible_tile_set);
        let visible_renderable_tiles = self.resolve_occupancy_tiles(&visible_tiles, cx);
        self.tile_manager.ensure_tiles_for_layout(
            &visible_renderable_tiles,
            TilePriority::Visible,
            self.render_texture_layout,
        );
        if !self.has_render_batch_capacity() && self.tile_manager.has_visible_work() {
            let active_bounds = tile_bounds_from_coords(&visible_tiles).unwrap_or(visible_bounds);
            let cancelled_batches =
                self.cancel_render_batches_outside_visible_bounds(active_bounds, center);
            if cancelled_batches > 0 {
                self.pending_viewport_refresh = true;
            }
        }
        self.schedule_next_tile_batch(cx);
    }

    fn has_current_viewport_work(&self) -> bool {
        let visible_tiles = self.tile_coords_for_viewport(0);
        self.tile_manager.has_queued_work_for_tiles(&visible_tiles)
            || has_unregistered_visible_tile_work(
                &visible_tiles,
                &self.tile_chunk_index,
                &self.tile_manager,
            )
    }

    pub(super) fn trim_tiles_to_memory_budget(&mut self, force: bool, cx: &mut Context<Self>) {
        let is_dragging = self.viewport_interaction_active();
        let retain_filter =
            retained_tile_filter_for_viewport(self.viewport, self.active_layout, is_dragging);
        self.trim_tiles_to_memory_budget_for_filter(retain_filter, force, cx);
    }

    pub(super) fn trim_tiles_to_memory_budget_for_filter(
        &mut self,
        retain_filter: Option<RetainedTileFilter>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let budget = ui_tile_memory_budget_bytes(self.viewport, self.render_texture_layout);
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
        let dropped_images = if let Some(retain_filter) = retain_filter {
            self.tile_manager
                .trim_loaded_tiles_to_budget_by(|coord| retain_filter.contains(coord), budget)
        } else {
            self.tile_manager
                .trim_loaded_tiles_to_budget_by(|_| false, budget)
        };
        if !dropped_images.is_empty() {
            let colors = self.theme_colors(cx);
            self.prune_canvas_tile_snapshot_to_current_tiles(colors, cx);
        }
        Self::drop_render_images(dropped_images, cx);
        self.flush_pending_render_image_evictions(cx);
    }

    pub(super) fn trim_tile_entries_to_capacity_for_filter(
        &mut self,
        retain_filter: Option<RetainedTileFilter>,
        cx: &mut Context<Self>,
    ) {
        let capacity = tile_cache_memory_limit(self.cpu_budget).max(
            retain_filter
                .map(RetainedTileFilter::maximum_tile_count)
                .unwrap_or(0),
        );
        self.trim_tile_entries_to_capacity(retain_filter, capacity, cx);
    }

    fn trim_tile_entries_to_capacity(
        &mut self,
        retain_filter: Option<RetainedTileFilter>,
        capacity: usize,
        cx: &mut Context<Self>,
    ) {
        let dropped_images = if let Some(retain_filter) = retain_filter {
            self.tile_manager
                .trim_entries_to_capacity_by(|coord| retain_filter.contains(coord), capacity)
        } else {
            self.tile_manager
                .trim_entries_to_capacity_by(|_| false, capacity)
        };
        if !dropped_images.is_empty() {
            let colors = self.theme_colors(cx);
            self.prune_canvas_tile_snapshot_to_current_tiles(colors, cx);
        }
        Self::drop_render_images(dropped_images, cx);
        self.flush_pending_render_image_evictions(cx);
    }

    pub(super) fn viewport_tile_plan(&self) -> ViewportTilePlan {
        build_viewport_tile_plan(ViewportTilePlanOptions {
            viewport: self.viewport,
            layout: self.active_layout,
            is_interacting: self.viewport_interaction_active(),
            drag: self.drag,
        })
    }

    pub(super) fn tile_coords_for_viewport(&self, radius: i32) -> Vec<(i32, i32)> {
        let center = self.viewport.center_tile(self.active_layout);
        let Some(visible) =
            visible_tile_bounds_for_viewport(self.viewport, self.active_layout, center)
        else {
            return Vec::new();
        };
        if radius <= 0 {
            return tile_coords_for_visible_bounds(visible, center);
        }
        tile_coords_for_bounds(
            visible,
            radius,
            center,
            canvas_tile_image_budget(self.viewport, self.active_layout),
        )
    }

    fn uses_viewport_composite(&self) -> bool {
        if self.viewport_drag_active() {
            return false;
        }
        viewport_composite_owns_viewport(
            false,
            self.viewport_composite_request_id.is_some(),
            !self.canvas_tile_snapshot.screen_images.is_empty(),
        )
    }

    fn schedule_viewport_composite_batch(
        &mut self,
        render_session: Arc<MapRenderSession>,
        viewport_interacting: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        if (viewport_interacting || self.right_selection_drag.is_some()) && !force {
            self.pending_viewport_refresh = true;
            self.schedule_viewport_work_refresh(cx);
            return;
        }
        let composite_viewport = viewport_with_composite_overscan(self.viewport);
        let center_tile = composite_viewport.center_tile(self.active_layout);
        let visible_tiles =
            visible_tile_bounds_for_viewport(composite_viewport, self.active_layout, center_tile)
                .map(|bounds| tile_coords_for_visible_bounds(bounds, center_tile))
                .unwrap_or_default();
        let signature = ViewportCompositeSignature {
            viewport: composite_viewport,
            layout: self.active_layout,
            render_generation: self.render_generation,
            visible: visible_tiles.clone(),
            center: center_tile,
            chunk_bounds_key: self.chunk_bounds.map(|bounds| {
                (
                    bounds.min_chunk_x,
                    bounds.min_chunk_z,
                    bounds.max_chunk_x,
                    bounds.max_chunk_z,
                )
            }),
        };
        if self.viewport_composite_signature.as_ref() == Some(&signature)
            && (self.viewport_composite_request_id.is_some()
                || !self.canvas_tile_snapshot.screen_images.is_empty())
        {
            return;
        }
        if let Some(request_id) = self.viewport_composite_request_id.take() {
            if let Some(cancel) = self.render_cancels.get(&request_id) {
                cancel.cancel();
            }
        }
        if !self.has_render_batch_capacity() {
            self.pending_viewport_refresh = true;
            return;
        }

        let mut render_plans = Vec::new();
        let mode = self.current_render_mode();
        let dimension = self.dimension;
        let layout = self.active_layout;
        for coord in &visible_tiles {
            let chunk_positions = match self.tile_chunk_index.get(coord) {
                Some(positions) => Some(Arc::clone(positions)),
                None => tile_chunk_positions_for_known_bounds(
                    dimension,
                    *coord,
                    layout,
                    self.chunk_bounds,
                )
                .ok(),
            };
            if chunk_positions
                .as_deref()
                .is_some_and(|positions| positions.is_empty())
            {
                continue;
            }
            match RenderTilePlan::from_optional_chunk_positions(
                dimension,
                mode,
                layout,
                *coord,
                chunk_positions,
            ) {
                Ok(plan) => render_plans.push(plan),
                Err(error) => {
                    tracing::debug!(tile = ?coord, %error, "map_viewer viewport_composite_plan_skipped");
                }
            }
        }
        if render_plans.is_empty() {
            self.status = SharedString::from("大视口合成暂无待渲染瓦片");
            return;
        }
        let Some(render_batch_permit) = self.acquire_render_batch() else {
            self.pending_viewport_refresh = true;
            return;
        };

        let requested_tiles = render_plans
            .iter()
            .map(|plan| plan.coord)
            .collect::<Vec<_>>();
        let request_id = self.request_id.saturating_add(1);
        self.request_id = request_id;
        let render_generation = self.render_generation;
        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let tile_cache_validation_seed = bedrock_render::render_preset_cache_validation_seed(
            &self.world_path,
            render_backend,
            render_gpu_backend,
        );
        let render_cancel = RenderCancelFlag::new();
        let render_cancel_for_owner = render_cancel.clone();
        self.track_render_request(
            request_id,
            render_cancel.clone(),
            &requested_tiles,
            center_tile,
        );
        self.viewport_composite_signature = Some(signature.clone());
        self.viewport_composite_request_id = Some(request_id);
        self.pending_viewport_refresh = false;

        if force && self.canvas_tile_snapshot.screen_images.is_empty() {
            match empty_viewport_composite_frame(self.viewport) {
                Ok(frame) => {
                    self.set_viewport_composite_frame(frame, cx);
                }
                Err(error) => {
                    self.clear_canvas_tile_snapshot(cx);
                    self.viewport_composite_signature = None;
                    self.viewport_composite_request_id = None;
                    self.status = SharedString::from(error.clone());
                    self.show_map_error(SharedString::from(error), cx);
                    return;
                }
            }
        }
        self.viewport_composite_signature = Some(signature.clone());
        self.viewport_composite_request_id = Some(request_id);

        let work_estimate = selected_tile_work_estimate(&requested_tiles, &self.tile_chunk_index);
        self.status = SharedString::from(format!(
            "大视口合成 {} 个瓦片 · chunk {} · 区域 {} · CPU {}%",
            requested_tiles.len(),
            work_estimate.chunk_count,
            work_estimate.region_count,
            self.cpu_budget.percent()
        ));
        tracing::debug!(
            request_id,
            visible = visible_tiles.len(),
            requested = requested_tiles.len(),
            chunks = work_estimate.chunk_count,
            regions = work_estimate.region_count,
            viewport_width = self.viewport.width,
            viewport_height = self.viewport.height,
            "map_viewer viewport_composite_start"
        );
        cx.notify();

        let viewport = composite_viewport;
        let requested_tiles_for_finish = requested_tiles.clone();
        let signature_for_finish = signature.clone();
        let composite_request = ViewportCompositeRequest {
            render_session,
            dimension,
            layout,
            viewport,
            center_tile,
            cache_policy: RenderCachePolicy::Use,
            plans: render_plans,
            cpu_budget,
            render_backend,
            render_gpu_backend,
            tile_cache_validation_seed,
            render_cancel,
        };
        let (event_sender, mut event_receiver) = unbounded::<ViewportCompositeEvent>();
        cx.spawn(async move |handle, cx| {
            let render_task = cx.background_spawn(async move {
                let _render_batch_permit = render_batch_permit;
                render_viewport_composite_stream(composite_request, event_sender)
            });
            let mut saw_complete = false;
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, ViewportCompositeEvent::Complete { .. });
                let Some(view) = handle.upgrade() else {
                    render_cancel_for_owner.cancel();
                    return Ok(());
                };
                let signature_for_event = signature_for_finish.clone();
                view.update(cx, move |this, cx| {
                    if this.viewport_composite_request_id == Some(request_id)
                        && this.viewport_composite_signature.as_ref() != Some(&signature_for_event)
                    {
                        match event {
                            ViewportCompositeEvent::Tile { tile } => {
                                Self::drop_render_image(Some(tile.image), cx);
                            }
                            ViewportCompositeEvent::Complete {
                                frame,
                                requested_tiles,
                                ..
                            } => {
                                if let Some(frame) = frame {
                                    Self::drop_render_image(Some(frame.image), cx);
                                }
                                this.finish_render_request(request_id, &requested_tiles);
                                this.viewport_composite_request_id = None;
                                this.schedule_viewport_work_refresh(cx);
                            }
                        }
                        return;
                    }
                    if this.viewport_composite_request_id != Some(request_id)
                        || !this.render_request_is_current(
                            request_id,
                            render_generation,
                            "viewport_composite",
                        )
                    {
                        match event {
                            ViewportCompositeEvent::Tile { tile } => {
                                Self::drop_render_image(Some(tile.image), cx);
                            }
                            ViewportCompositeEvent::Complete { frame, .. } if frame.is_some() => {
                                if let Some(frame) = frame {
                                    Self::drop_render_image(Some(frame.image), cx);
                                }
                            }
                            ViewportCompositeEvent::Complete { .. } => {}
                        }
                        return;
                    }

                    match event {
                        ViewportCompositeEvent::Tile { tile } => {
                            if this.append_viewport_composite_tile(tile, cx) {
                                let colors = this.theme_colors(cx);
                                this.sync_canvas_snapshot(colors, cx);
                            }
                        }
                        ViewportCompositeEvent::Complete {
                            frame,
                            requested_tiles,
                            rendered_tiles,
                            failed_tiles,
                            diagnostics,
                            stats,
                        } => {
                            if failed_tiles == 0 {
                                if let Some(frame) = frame {
                                    // An empty render result can happen while the camera is
                                    // crossing an index boundary. Keep the last visible frame
                                    // instead of replacing it with a blank image.
                                    if rendered_tiles > 0
                                        || this.canvas_tile_snapshot.screen_images.is_empty()
                                    {
                                        if this.set_viewport_composite_frame(frame, cx) {
                                            let colors = this.theme_colors(cx);
                                            this.sync_canvas_snapshot(colors, cx);
                                        }
                                    } else {
                                        Self::drop_render_image(Some(frame.image), cx);
                                    }
                                }
                            } else if let Some(frame) = frame {
                                Self::drop_render_image(Some(frame.image), cx);
                            }
                            this.finish_render_request(request_id, &requested_tiles);
                            this.viewport_composite_request_id = None;
                            this.diagnostics.add(diagnostics);
                            this.render_stats = stats;
                            this.status = if failed_tiles == 0 {
                                SharedString::from(format!(
                                    "地图合成完成 · 显示 {} 个瓦片 · GPU 图像 1",
                                    rendered_tiles
                                ))
                            } else {
                                SharedString::from(format!(
                                    "地图数据读取失败 {} 个瓦片，已保留上一张完整地图",
                                    failed_tiles
                                ))
                            };
                            tracing::debug!(
                                request_id,
                                rendered = rendered_tiles,
                                requested = requested_tiles.len(),
                                failed_tiles,
                                "map_viewer viewport_composite_complete"
                            );
                            if this.pending_viewport_refresh {
                                this.schedule_viewport_work_refresh(cx);
                            }
                        }
                    }
                    cx.notify();
                })?;
                if is_complete {
                    saw_complete = true;
                    break;
                }
            }
            let result = render_task.await;
            if !saw_complete || result.is_err() {
                let Some(view) = handle.upgrade() else {
                    render_cancel_for_owner.cancel();
                    return Ok(());
                };
                let error = result
                    .err()
                    .unwrap_or_else(|| "视口合成事件流提前结束".to_string());
                view.update(cx, move |this, cx| {
                    if this.viewport_composite_request_id != Some(request_id)
                        || !this.render_request_is_current(
                            request_id,
                            render_generation,
                            "viewport_composite_failed",
                        )
                    {
                        return;
                    }
                    this.finish_render_request(request_id, &requested_tiles_for_finish);
                    this.viewport_composite_signature = None;
                    this.viewport_composite_request_id = None;
                    if viewport_composite_error_is_cancelled(&error) {
                        this.status = SharedString::from("视口变化，正在重新合成地图");
                        tracing::debug!(request_id, "map_viewer viewport_composite_cancelled");
                    } else {
                        this.status = SharedString::from(error.clone());
                        tracing::warn!(request_id, %error, "map_viewer viewport_composite_failed");
                        this.show_map_error(SharedString::from(error), cx);
                    }
                    this.pending_viewport_refresh = true;
                    this.schedule_viewport_work_refresh(cx);
                    cx.notify();
                })?;
            }
            if let Some(view) = handle.upgrade() {
                view.update(cx, |this, cx| {
                    this.resume_render_work_after_physical_batch(cx);
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn set_viewport_composite_frame(
        &mut self,
        frame: ViewportCompositeFrame,
        cx: &mut Context<Self>,
    ) -> bool {
        if !screen_image_viewports_transformable(self.viewport, frame.source_viewport) {
            Self::drop_render_image(Some(frame.image), cx);
            self.pending_viewport_refresh = true;
            self.status = SharedString::from("视口变化，正在重新合成地图");
            self.schedule_viewport_work_refresh(cx);
            return false;
        }
        self.viewport_zoom_transition = false;
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.replace_canvas_tile_snapshot(
            TilePaintSnapshot {
                tiles: Arc::new(Vec::new()),
                screen_images: Arc::new(vec![ScreenPaintImage {
                    image: frame.image,
                    source_viewport: frame.source_viewport,
                    left: 0.0,
                    top: 0.0,
                    width: frame.source_viewport.width,
                    height: frame.source_viewport.height,
                    estimated_bytes: frame.estimated_bytes,
                }]),
                debug_overlays: Arc::new(Vec::new()),
                generation: self.canvas_tile_generation,
                estimated_bytes: frame.estimated_bytes,
                paint_bounds: self.current_canvas_paint_bounds(),
            },
            cx,
        );
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
        true
    }

    fn append_viewport_composite_tile(
        &mut self,
        tile: ViewportCompositeTile,
        cx: &mut Context<Self>,
    ) -> bool {
        if !screen_image_viewports_transformable(self.viewport, tile.source_viewport) {
            Self::drop_render_image(Some(tile.image), cx);
            return false;
        }
        let candidate = ScreenPaintImage {
            image: tile.image,
            source_viewport: tile.source_viewport,
            left: tile.left,
            top: tile.top,
            width: tile.width,
            height: tile.height,
            estimated_bytes: tile.estimated_bytes,
        };
        let canvas_bounds = Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(self.viewport.width), px(self.viewport.height)),
        );
        let candidate_visible = screen_image_bounds(canvas_bounds, self.viewport, &candidate)
            .is_some_and(|bounds| {
                bounds.right() > canvas_bounds.left()
                    && bounds.left() < canvas_bounds.right()
                    && bounds.bottom() > canvas_bounds.top()
                    && bounds.top() < canvas_bounds.bottom()
            });
        let mut screen_images = if self.viewport_zoom_transition && candidate_visible {
            self.viewport_zoom_transition = false;
            Vec::new()
        } else {
            self.canvas_tile_snapshot.screen_images.as_ref().clone()
        };
        screen_images.push(candidate);
        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        let estimated_bytes = screen_images
            .iter()
            .map(|image| image.estimated_bytes)
            .sum::<usize>();
        self.replace_canvas_tile_snapshot(
            TilePaintSnapshot {
                tiles: Arc::new(Vec::new()),
                screen_images: Arc::new(screen_images),
                debug_overlays: Arc::new(Vec::new()),
                generation: self.canvas_tile_generation,
                estimated_bytes,
                paint_bounds: self.current_canvas_paint_bounds(),
            },
            cx,
        );
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;
        true
    }

    pub(super) fn schedule_next_tile_batch(&mut self, cx: &mut Context<Self>) {
        let viewport_dragging = self.viewport_drag_active();
        let viewport_interacting = self.viewport_interaction_active();
        if self.right_selection_drag.is_some() {
            self.pending_viewport_refresh = true;
            return;
        }
        let Some(render_session) = self.render_session.clone() else {
            if !self.session_loading {
                tracing::debug!("map_viewer schedule_waiting_for_session");
                self.refresh_render_session(cx);
            }
            return;
        };
        if VIEWPORT_COMPOSITE_ENABLED || self.uses_viewport_composite() {
            if viewport_interacting {
                self.pending_viewport_refresh = true;
                self.schedule_viewport_idle_refresh(cx);
            } else {
                self.schedule_viewport_composite_batch(render_session, false, false, cx);
            }
            return;
        }
        let tile_plan = self.viewport_tile_plan();
        let visible_tiles = tile_plan.visible.clone();
        let center_tile = tile_plan.center;
        let paint_resources_unavailable = take_map_tile_paint_resources_unavailable();
        if paint_resources_unavailable {
            tracing::warn!(
                visible = visible_tiles.len(),
                loaded = self.tile_manager.loaded_count(),
                "map_viewer releasing tile images after paint resource exhaustion"
            );
            let dropped_images = self
                .tile_manager
                .trim_loaded_tiles_to_budget_by(|_| false, 0);
            if !dropped_images.is_empty() {
                Self::drop_render_images(dropped_images, cx);
            }
            self.clear_canvas_tile_snapshot(cx);
            self.pending_viewport_refresh = true;
            self.status = SharedString::from("GPU 图片资源不足，已释放瓦片图片并重新排队");
            self.schedule_viewport_work_refresh(cx);
            cx.notify();
            return;
        }
        let visible_ready_count = visible_tiles
            .iter()
            .filter(|coord| {
                self.tile_manager.entries.get(coord).is_some_and(|entry| {
                    matches!(entry.state, TileLoadState::Loaded | TileLoadState::Invalid)
                })
            })
            .count();
        let visible_reveal_incomplete =
            !visible_tiles.is_empty() && visible_ready_count < visible_tiles.len();
        let quick_reveal_initial_load = visible_reveal_incomplete && !viewport_interacting;
        let has_batch_capacity = if quick_reveal_initial_load {
            self.has_quick_reveal_render_batch_capacity()
        } else {
            self.has_render_batch_capacity()
        };
        if !has_batch_capacity {
            return;
        }
        let batch_size = visible_render_batch_size(
            interactive_tile_batch_size(self.render_backend, self.cpu_budget),
            visible_tiles.len(),
            viewport_dragging,
            quick_reveal_initial_load,
        );
        let allow_prefetch = !viewport_dragging
            && map_viewer_prefetch_radius() > 0
            && !self.pending_viewport_refresh
            && !visible_reveal_incomplete;
        let visible_bounds = tile_bounds_from_coords(&visible_tiles);
        let visible_candidate_tiles = self.tile_manager.queued_visible_coords_limited_ordered(
            &visible_tiles,
            &self.active_render_tiles,
            batch_size,
        );
        let candidate_tiles = if viewport_dragging || !visible_candidate_tiles.is_empty() {
            visible_candidate_tiles
        } else {
            let edit_refresh_tiles = self
                .tile_manager
                .queued_coords_limited(center_tile, visible_bounds, true, true, batch_size)
                .into_iter()
                .filter(|coord| {
                    self.tile_manager
                        .entries
                        .get(coord)
                        .is_some_and(|entry| entry.priority == TilePriority::EditRefresh)
                })
                .collect::<Vec<_>>();
            if edit_refresh_tiles.is_empty() && allow_prefetch {
                self.tile_manager
                    .queued_prefetch_coords_limited_ordered(&tile_plan.prefetch, batch_size)
            } else {
                edit_refresh_tiles
            }
        };
        let mode = self.current_render_mode();
        let dimension = self.dimension;
        let layout = self.render_texture_layout;
        let mut render_plans = Vec::with_capacity(batch_size);
        for coord in candidate_tiles {
            if render_plans.len() >= batch_size {
                break;
            }
            if self.active_render_tiles.contains(&coord) {
                continue;
            }
            let chunk_positions = match self.tile_chunk_index.get(&coord) {
                Some(positions) => Some(Arc::clone(positions)),
                None => {
                    match tile_chunk_positions_for_known_bounds(
                        dimension,
                        coord,
                        layout,
                        self.chunk_bounds,
                    ) {
                        Ok(positions) => Some(positions),
                        Err(error) => {
                            Self::drop_render_image(
                                self.tile_manager
                                    .mark_failed(coord, SharedString::from(error)),
                                cx,
                            );
                            continue;
                        }
                    }
                }
            };
            if chunk_positions
                .as_deref()
                .is_some_and(|positions| positions.is_empty())
            {
                Self::drop_render_image(
                    self.tile_manager
                        .mark_invalid(coord, SharedString::from("索引确认该瓦片没有可渲染区块")),
                    cx,
                );
                continue;
            }
            match RenderTilePlan::from_optional_chunk_positions(
                dimension,
                mode,
                layout,
                coord,
                chunk_positions,
            ) {
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
        let has_edit_refresh_tiles = render_plans.iter().any(|plan| {
            self.tile_manager
                .entries
                .get(&plan.coord)
                .is_some_and(|entry| entry.priority == TilePriority::EditRefresh)
        });
        let has_stale_cache_refresh_tiles = render_plans.iter().any(|plan| {
            self.tile_manager
                .entries
                .get(&plan.coord)
                .is_some_and(|entry| entry.source_status == TileSourceStatus::DiskStale)
        });
        if has_edit_refresh_tiles && !self.bypass_cache_active {
            render_plans.retain(|plan| {
                self.tile_manager
                    .entries
                    .get(&plan.coord)
                    .is_some_and(|entry| entry.priority == TilePriority::EditRefresh)
            });
        }
        let requested_tiles = render_plans
            .iter()
            .map(|plan| plan.coord)
            .collect::<Vec<_>>();
        let cache_policy = if self.bypass_cache_active {
            RenderCachePolicy::Bypass
        } else if has_edit_refresh_tiles || has_stale_cache_refresh_tiles {
            RenderCachePolicy::Refresh
        } else {
            RenderCachePolicy::Use
        };
        if requested_tiles.is_empty() {
            if self.tile_manager.queued_count() == 0
                && self.tile_manager.loading_count() == 0
                && self.tile_manager.empty_count() == 0
            {
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
                    self.cpu_budget.percent()
                ))
            } else {
                SharedString::from(format!(
                    "暂无待渲染瓦片 · 可见 {visible_loaded}/{} · 排队 {} · 探测 {} · 加载中 {} · 失败 {}",
                    visible_tiles.len(),
                    self.tile_manager.queued_count(),
                    self.tile_manager.empty_count(),
                    self.tile_manager.loading_count(),
                    self.tile_manager.failed_count()
                ))
            };
            tracing::debug!(
                visible = visible_tiles.len(),
                visible_loaded,
                queued = self.tile_manager.queued_count(),
                empty = self.tile_manager.empty_count(),
                loading = self.tile_manager.loading_count(),
                failed = self.tile_manager.failed_count(),
                "map_viewer render_batch_idle"
            );
            return;
        }
        let Some(render_batch_permit) = self.acquire_render_batch() else {
            self.pending_viewport_refresh = true;
            return;
        };
        self.tile_manager.mark_loading(&requested_tiles);

        self.request_id = self.request_id.saturating_add(1);
        let request_id = self.request_id;
        let render_generation = self.render_generation;
        let requested_tile_count = requested_tiles.len();
        // A low-zoom camera can expose more tiles than one foreground registration
        // slice. Starting a physical batch must not consume that deferred camera work;
        // completion will keep requesting slices until every visible tile is registered.
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
            self.cpu_budget.percent()
        ));
        cx.notify();

        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let metadata_indexed_tiles = requested_tiles
            .iter()
            .filter(|coord| self.tile_chunk_index.contains_key(coord))
            .count();
        let unindexed_tiles = requested_tile_count.saturating_sub(metadata_indexed_tiles);
        let tile_cache_validation_seed = if unindexed_tiles > 0 && metadata_indexed_tiles == 0 {
            0
        } else {
            bedrock_render::render_preset_cache_validation_seed(
                &self.world_path,
                render_backend,
                render_gpu_backend,
            )
        };
        let work_estimate = selected_tile_work_estimate(&requested_tiles, &self.tile_chunk_index);
        let quick_reveal = !viewport_interacting
            && (visible_reveal_incomplete || self.tile_reveal_state.ready_batches == 0);
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
            ui_batch_chunks = work_estimate.chunk_count,
            ui_batch_regions = work_estimate.region_count,
            metadata_indexed_tiles,
            unindexed_tiles,
            exact_occupancy_chunks = unindexed_tiles == 0,
            center = ?center_tile,
            backend = ?render_backend,
            gpu_backend = ?render_gpu_backend,
            cache_policy = ?cache_policy,
            "map_viewer render_batch_start"
        );

        cx.spawn(async move |handle, cx| {
            let render_task = cx.background_spawn(async move {
                let _render_batch_permit = render_batch_permit;
                render_tile_batch_stream(tile_batch_request, event_sender)
            });

            let mut saw_complete = false;
            loop {
                let Some(event) = event_receiver.next().await else {
                    break;
                };
                let is_complete = matches!(&event, TileRenderEvent::Complete { .. });
                let should_refresh_window = tile_event_needs_window_refresh(&event);
                let Some(view) = handle.upgrade() else {
                    render_cancel_for_owner.cancel();
                    return Ok(());
                };
                let viewport_interacting = match view
                    .read_with(cx, |this, _| this.viewport_interaction_active())
                {
                    Ok(viewport_interacting) => viewport_interacting,
                    Err(error) => {
                        tracing::debug!(?error, "failed to read map viewport interaction state");
                        false
                    }
                };
                let should_yield_after_ready = should_yield_after_ready_batch(
                    quick_reveal,
                    viewport_interacting,
                    &event,
                );
                if let Err(error) = view.update(cx, move |this, cx| {
                    if !this.render_request_is_current(request_id, render_generation, "ready_event") {
                        return;
                    }

                    let mut notify_parent = true;
                    match event {
                        TileRenderEvent::ReadyBatch { tiles } => {
                            notify_parent = false;
                            let defer_canvas_refresh = this.viewport_interaction_active();
                            let ready_count = tiles.len();
                            let mut changed_tiles = Vec::with_capacity(ready_count);
                            for ReadyTile {
                                coord,
                                tile,
                                source,
                                chunk_positions,
                            } in tiles
                            {
                                if let Some(chunk_positions) = chunk_positions {
                                    if chunk_positions.is_empty() {
                                        Self::drop_render_image(
                                            this.tile_manager.mark_empty(coord),
                                            cx,
                                        );
                                        this.available_tiles.remove(&coord);
                                        this.tile_chunk_index.remove(&coord);
                                        changed_tiles.push(coord);
                                        continue;
                                    } else {
                                        this.available_tiles.insert(coord);
                                        this.tile_chunk_index.insert(coord, chunk_positions);
                                    }
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
                                    let is_stale_cache =
                                        cache_freshness == TileSourceFreshness::Stale;
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
                                        if is_stale_cache {
                                            this.tile_manager.requeue_stale_cache_for_refresh(coord);
                                            this.pending_viewport_refresh = true;
                                        }
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
                            if defer_canvas_refresh {
                                this.pending_viewport_refresh = true;
                                this.refresh_interaction_canvas_tiles_after_ready_batch(
                                    &changed_tiles,
                                );
                            } else {
                                this.trim_tiles_to_memory_budget(false, cx);
                                let retain_filter = retained_tile_filter_for_viewport(
                                    this.viewport,
                                    this.active_layout,
                                    false,
                                );
                                this.trim_tile_entries_to_capacity_for_filter(retain_filter, cx);
                                let colors = this.theme_colors(cx);
                                this.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
                            }
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
                            let is_cancelled = message.contains("取消")
                                || message.to_ascii_lowercase().contains("cancel");
                            tracing::warn!(
                                request_id,
                                tile = ?coord,
                                %message,
                                "map_viewer render_tile_failed"
                            );
                            if is_cancelled {
                                this.tile_manager.requeue_cancelled_loading(&[coord]);
                            } else if message.contains("no renderable chunks")
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
                                        this.tile_manager.mark_empty(coord),
                                        cx,
                                    );
                                    this.available_tiles.remove(&coord);
                                    this.tile_chunk_index.remove(&coord);
                                }
                            } else {
                                Self::drop_render_image(
                                    this.tile_manager
                                        .mark_failed(coord, SharedString::from(message)),
                                    cx,
                                );
                            }
                            if this.viewport_interaction_active() {
                                this.pending_viewport_refresh = true;
                            } else {
                                let colors = this.theme_colors(cx);
                                this.refresh_canvas_tiles_if_changed(&[coord], colors, cx);
                            }
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
                                this.tile_manager.mark_empty(coord),
                                cx,
                            );
                            this.available_tiles.remove(&coord);
                            this.tile_chunk_index.remove(&coord);
                            if this.viewport_interaction_active() {
                                this.pending_viewport_refresh = true;
                            } else {
                                let colors = this.theme_colors(cx);
                                this.refresh_canvas_tiles_if_changed(&[coord], colors, cx);
                            }
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
                                            | TileLoadState::Empty
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
                                if this.viewport_interaction_active() {
                                    this.pending_viewport_refresh = true;
                                } else {
                                    let colors = this.theme_colors(cx);
                                    this.refresh_canvas_tiles_if_changed(
                                        &completion_changed_tiles,
                                        colors,
                                        cx,
                                    );
                                }
                            }
                            this.status = SharedString::from(format!(
                                "瓦片批次 {request_id} 完成 · 已加载 {} · 排队 {} · 冷渲染 {} · CPU {} · GPU {}（{}） · {} · 渲染缓存 命中 {}（内存 {} / 磁盘新 {} / 磁盘旧 {}）/ 未命中 {} / 负缓存 {} / 读取 {}ms / 解压 {}ms / blob {}ms · 瓦片索引 T/V/M/E {}/{}/{}/{} · 依赖校验 {}ms · 写入丢弃 {} · 损坏 miss {} · 刷新 {} · 局部 chunk {} · 距离² {} · 区域缓存 {}/{} · 数据库 {}ms · 解码 {}ms · 合成 {}ms · GPU {}ms{}",
                                this.tile_manager.loaded_count(),
                                this.tile_manager.queued_count(),
                                this.cold_rendered_tiles,
                                this.render_stats.cpu_tiles,
                                this.render_stats.gpu_tiles,
                                this.render_stats.resolved_backend.label(),
                                gpu_status_text(&this.render_stats),
                                this.render_stats.cache_hits,
                                this.render_stats.cache_memory_hits,
                                this.render_stats.cache_disk_fresh_hits,
                                this.render_stats.cache_disk_stale_hits,
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
                                ui_batch_chunks = work_estimate.chunk_count,
                                ui_batch_regions = work_estimate.region_count,
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
                            let needs_more_viewport_work = pending_viewport_refresh
                                || this.has_current_viewport_work();
                            if this.viewport_interaction_active() {
                                this.pending_viewport_refresh = true;
                            } else if needs_more_viewport_work {
                                this.pending_viewport_refresh = true;
                                this.schedule_viewport_work_refresh(cx);
                            } else {
                                this.pending_viewport_refresh = false;
                                if this.professional.pending_overlay_refresh {
                                    this.refresh_professional_overlays(cx);
                                }
                                if VIEWPORT_COMPOSITE_ENABLED
                                    && this.canvas_tile_snapshot.screen_images.is_empty()
                                    && let Some(render_session) = this.render_session.clone()
                                {
                                    this.schedule_viewport_composite_batch(
                                        render_session,
                                        false,
                                        false,
                                        cx,
                                    );
                                }
                            }
                            if this.has_render_batch_capacity() && this.tile_manager.queued_count() > 0
                            {
                                this.schedule_next_tile_batch(cx);
                            }
                            this.finish_paste_preview_tile_refresh(cx);
                        }
                    }
                    if notify_parent {
                        cx.notify();
                    }
                }) {
                    tracing::warn!(?error, "failed to merge map tile event");
                }
                if should_refresh_window {
                    if let Err(error) = cx.refresh() {
                        tracing::debug!(?error, "failed to refresh map tile window");
                    }
                }
                if is_complete {
                    saw_complete = true;
                    break;
                }
                if should_yield_after_ready {
                    Timer::after(QUICK_REVEAL_TILE_FRAME_INTERVAL).await;
                }
            }

            let result = render_task.await;
            if let Err(error) = result {
                let requested_tiles = requested_tiles.clone();
                let Some(view) = handle.upgrade() else {
                    return Ok(());
                };
                if let Err(update_error) = view.update(cx, move |this, cx| {
                    if !this.render_request_is_current(request_id, render_generation, "error") {
                        return;
                    }
                    this.finish_render_request(request_id, &requested_tiles);
                    let pending_viewport_refresh = this.pending_viewport_refresh;
                    this.pending_viewport_refresh = false;
                    let is_cancelled =
                        error.contains("取消") || error.to_ascii_lowercase().contains("cancel");
                    let message = SharedString::from(error);
                    let mut changed_tiles = Vec::new();
                    if is_cancelled {
                        this.tile_manager
                            .requeue_cancelled_loading(&requested_tiles);
                    } else {
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
                    }
                    let colors = this.theme_colors(cx);
                    if this.viewport_interaction_active() {
                        this.pending_viewport_refresh = true;
                    } else {
                        this.refresh_canvas_tiles_if_changed(&changed_tiles, colors, cx);
                    }
                    this.status = message;
                    tracing::warn!(request_id, status = %this.status, "map_viewer render_batch_error");
                    if this.viewport_interaction_active() {
                        this.pending_viewport_refresh = true;
                    } else {
                        this.pending_viewport_refresh = false;
                        if pending_viewport_refresh
                            || this.has_current_viewport_work()
                        {
                            this.ensure_visible_tiles(cx);
                        }
                    }
                    if this.has_render_batch_capacity() && this.tile_manager.queued_count() > 0 {
                        this.schedule_next_tile_batch(cx);
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
                    if !this.render_request_is_current(request_id, render_generation, "finalize") {
                        return;
                    }
                    this.finish_render_request(request_id, &requested_tiles);
                    let pending_viewport_refresh = this.pending_viewport_refresh;
                    this.status = SharedString::from(format!(
                        "瓦片批次 {request_id} 已结束 · 已加载 {} · 排队 {} · CPU 瓦片 {}",
                        this.tile_manager.loaded_count(),
                        this.tile_manager.queued_count(),
                        this.render_stats.cpu_tiles
                    ));
                    if this.viewport_interaction_active() {
                        this.pending_viewport_refresh = true;
                    } else {
                        this.pending_viewport_refresh = false;
                        if pending_viewport_refresh
                            || this.has_current_viewport_work()
                        {
                            this.ensure_visible_tiles(cx);
                        }
                    }
                    if this.has_render_batch_capacity() && this.tile_manager.queued_count() > 0 {
                        this.schedule_next_tile_batch(cx);
                    }
                    cx.notify();
                }) {
                    tracing::warn!(?error, "failed to finalize map tile batch");
                }
            }

            if let Some(view) = handle.upgrade() {
                view.update(cx, |this, cx| {
                    this.resume_render_work_after_physical_batch(cx);
                })?;
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
        if self.has_render_batch_capacity() && self.tile_manager.queued_count() > 0 {
            self.schedule_next_tile_batch(cx);
        }
    }

    fn finish_paste_preview_tile_refresh(&mut self, cx: &mut Context<Self>) {
        let edit_refresh_active = self.tile_manager.entries.values().any(|entry| {
            entry.priority == TilePriority::EditRefresh
                && matches!(
                    entry.state,
                    TileLoadState::Empty | TileLoadState::Queued | TileLoadState::Loading
                )
        });
        let paste_is_waiting = self
            .professional
            .paste_preview
            .as_ref()
            .and_then(|preview| preview.write_progress)
            .is_some_and(|progress| progress.awaiting_tile_refresh);
        if !paste_tile_refresh_can_finish(
            self.session_loading,
            edit_refresh_active,
            paste_is_waiting,
        ) {
            return;
        }
        self.clear_paste_preview_state(cx);
        self.finish_pending_paste_task(None);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    fn finish_pending_paste_task(&mut self, refresh_error: Option<&str>) {
        let Some(completion) = self.pending_paste_task_completion.take() else {
            return;
        };
        let message = match refresh_error {
            Some(error) => format!("{}；瓦片刷新失败: {error}", completion.message),
            None => completion.message,
        };
        task_manager::finish_task(&completion.task_id, "completed", Some(message));
        probe_tokio_blocking_pool("map_paste_refresh_complete");
    }
}

pub(super) const fn paste_tile_refresh_can_finish(
    session_loading: bool,
    edit_refresh_active: bool,
    paste_is_waiting: bool,
) -> bool {
    !session_loading && !edit_refresh_active && paste_is_waiting
}

pub(super) fn memory_snapshot_due(last_recorded: Option<Instant>, now: Instant) -> bool {
    last_recorded.is_none_or(|last_recorded| {
        now.saturating_duration_since(last_recorded) >= MAP_MEMORY_SNAPSHOT_INTERVAL
    })
}

pub(super) const fn should_probe_edit_refresh_occupancy(
    occupancy_index_in_flight: bool,
    has_edit_refresh_occupancy: bool,
) -> bool {
    has_edit_refresh_occupancy && !occupancy_index_in_flight
}

use super::editor::{copy_chunks_blocking, pasted_chunk_targets};
use super::helpers::*;
use super::import_preview;
use super::mcstructure;
use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::region_package;
use super::tile_render::{
    RenderTilePlan, TileBatchRequest, open_map_render_session, render_tile_batch_stream,
};
use super::tile_state::ReadyTile;
use crate::ui::state::launcher::LauncherState;
use crate::ui::state::local_versions::LocalVersionsState;
use std::io::Cursor;

struct CopyChunkComplete {
    copied_chunk: CopiedChunkData,
    preview_images: BTreeMap<ChunkPos, CopiedChunkPreviewImage>,
    preview_error: Option<String>,
}

impl MapViewerWindowView {
    pub(super) fn begin_edit_toast(
        &mut self,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if let Some(toast_id) = self.edit_toast_id.take() {
            toast::dismiss(cx, toast_id);
        }
        self.edit_toast_id = Some(toast::pending(cx, message.into()));
    }

    pub(super) fn resolve_edit_toast(
        &mut self,
        kind: toast::ToastKind,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        if let Some(toast_id) = self.edit_toast_id.take() {
            toast::resolve(cx, toast_id, kind, message);
        } else {
            toast::push_kind(cx, kind, message);
        }
    }

    pub(super) fn map_shortcuts_allowed(&self, window: &Window, cx: &App) -> bool {
        !self.text_input_focused(window, cx) && self.map_focus_handle.is_focused(window)
    }

    pub(super) fn set_mode(&mut self, mode: ViewerMode, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.invalidate_tiles(cx);
        self.ensure_visible_tiles(cx);
        cx.notify();
    }

    pub(super) fn set_dimension(&mut self, dimension: Dimension, cx: &mut Context<Self>) {
        if self.dimension == dimension {
            return;
        }
        self.dimension = dimension;
        self.context_menu = None;
        self.cancel_professional_overlay_query();
        self.professional = ProfessionalQueryState::default();
        self.replace_paste_preview_images(Vec::new(), cx);
        self.set_professional_detail(None, cx);
        self.db_tree = DbTreeState::default();
        self.recenter_on_next_metadata = true;
        self.refresh_metadata(cx);
    }

    pub(super) fn step_y(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.y_layer = self.y_layer.saturating_add(delta).clamp(-64, 320);
        if matches!(
            self.mode,
            ViewerMode::Biome | ViewerMode::Layer | ViewerMode::Cave
        ) {
            self.invalidate_tiles(cx);
            self.ensure_visible_tiles(cx);
        }
        cx.notify();
    }

    pub(super) fn zoom_at(&mut self, position: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
        self.viewport.zoom_at(position, factor);
        self.context_menu = None;
        self.ensure_visible_tiles(cx);
        self.professional.pending_overlay_refresh = true;
        self.schedule_viewport_idle_refresh(cx);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn zoom_by_center(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom_at(
            point(
                px(self.viewport.width / 2.0),
                px(self.viewport.height / 2.0),
            ),
            factor,
            cx,
        );
    }

    pub(super) fn recenter_to_spawn(&mut self, cx: &mut Context<Self>) {
        self.recenter_on_next_metadata = true;
        self.refresh_metadata(cx);
    }

    pub(super) fn redraw_bypassing_cache(&mut self, cx: &mut Context<Self>) {
        self.bypass_cache_active = true;
        self.invalidate_tiles(cx);
        self.ensure_visible_tiles(cx);
        cx.notify();
    }

    pub(super) fn step_cpu_budget(&mut self, delta: i8, cx: &mut Context<Self>) {
        self.cpu_budget.step(delta);
        self.status = SharedString::from(format!(
            "CPU预算已设为 {}% · 下个批次生效",
            self.cpu_budget.percent
        ));
        self.schedule_next_tile_batch(cx);
        cx.notify();
    }

    pub(super) fn toggle_render_backend(&mut self, cx: &mut Context<Self>) {
        match (self.render_backend, self.render_gpu_backend) {
            (RenderBackend::Cpu, _) => {
                self.render_backend = RenderBackend::Auto;
                self.render_gpu_backend = RenderGpuBackend::Auto;
            }
            (RenderBackend::Auto, _) => {
                self.render_backend = RenderBackend::Wgpu;
                #[cfg(target_os = "windows")]
                {
                    self.render_gpu_backend = RenderGpuBackend::Dx11;
                }
                #[cfg(not(target_os = "windows"))]
                {
                    self.render_gpu_backend = RenderGpuBackend::Vulkan;
                }
            }
            (RenderBackend::Wgpu, RenderGpuBackend::Dx11) => {
                self.render_backend = RenderBackend::Cpu;
                self.render_gpu_backend = RenderGpuBackend::Auto;
            }
            _ => {
                self.render_backend = RenderBackend::Cpu;
                self.render_gpu_backend = RenderGpuBackend::Auto;
            }
        }
        self.status = SharedString::from(format!(
            "渲染后端固定为 {} · 缓存签名隔离并从下个批次生效",
            render_backend_label(self.render_backend, self.render_gpu_backend)
        ));
        self.invalidate_tiles(cx);
        self.refresh_render_session(cx);
        self.ensure_visible_tiles(cx);
        cx.notify();
    }

    pub(super) fn toggle_top_more(&mut self, cx: &mut Context<Self>) {
        self.ui_state.top_more_open = !self.ui_state.top_more_open;
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn close_top_more(&mut self) {
        self.ui_state.top_more_open = false;
    }

    pub(super) fn close_all_menus(&mut self, cx: &mut Context<Self>) {
        let changed = self.context_menu.take().is_some()
            || self.ui_state.top_more_open
            || self.ui_state.context_more_open
            || self.ui_state.context_paste_open;
        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;
        self.ui_state.context_paste_open = false;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn handle_action(&mut self, action: MapViewerAction, cx: &mut Context<Self>) {
        match action {
            MapViewerAction::SetMode(mode) => self.set_mode(mode, cx),
            MapViewerAction::StepY(delta) => self.step_y(delta, cx),
            MapViewerAction::ZoomBy(factor) => self.zoom_by_center(factor, cx),
            MapViewerAction::ImportStructureFile => self.open_import_structure_dialog(cx),
            MapViewerAction::ToggleTopMore => self.toggle_top_more(cx),
            MapViewerAction::ToggleLeftPanel => self.toggle_left_panel(cx),
            MapViewerAction::ToggleBottomPanel => self.toggle_bottom_panel(cx),
            MapViewerAction::SetBottomTab(tab) => self.set_bottom_tab(tab, cx),
            MapViewerAction::OpenRightNbt => self.open_right_nbt_panel(cx),
            MapViewerAction::OpenRightPreview3d => self.open_right_preview_3d_panel(cx),
            MapViewerAction::CloseMenus => self.close_all_menus(cx),
        }
    }

    pub(super) fn toggle_axis(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.axis = !self.overlay_options.axis;
        cx.notify();
    }

    pub(super) fn toggle_dense_grid(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.dense_grid = !self.overlay_options.dense_grid;
        cx.notify();
    }

    pub(super) fn toggle_ruler(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.ruler = !self.overlay_options.ruler;
        cx.notify();
    }

    pub(super) fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        let open = !self.ui_state.right_panel_open;
        if !open && self.ui_state.active_right_panel == MapViewerRightPanel::Preview3d {
            self.clear_preview_3d_resources(false);
        }
        self.ui_state.set_right_panel_open(open);
        let size = size(px(self.window_width), px(self.window_height));
        if self.viewport.set_size(self.center_stage_size(size)) {
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches();
            self.refresh_professional_overlays(cx);
        }
        cx.notify();
    }

    pub(super) fn open_right_nbt_panel(&mut self, cx: &mut Context<Self>) {
        if self.ui_state.active_right_panel == MapViewerRightPanel::Preview3d {
            self.clear_preview_3d_resources(false);
        }
        self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
        self.ui_state.set_right_panel_open(true);
        self.update_viewport_after_dock_change(cx);
        cx.notify();
    }

    pub(super) fn open_right_preview_3d_panel(&mut self, cx: &mut Context<Self>) {
        self.show_right_preview_3d_panel(cx);
        cx.notify();
    }

    pub(super) fn show_right_preview_3d_panel(&mut self, cx: &mut Context<Self>) {
        self.ui_state.active_right_panel = MapViewerRightPanel::Preview3d;
        self.ui_state.set_right_panel_open(true);
        self.update_viewport_after_dock_change(cx);
    }

    pub(super) fn update_viewport_after_dock_change(&mut self, cx: &mut Context<Self>) {
        let size = size(px(self.window_width), px(self.window_height));
        if self.viewport.set_size(self.center_stage_size(size)) {
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches();
            self.refresh_professional_overlays(cx);
        }
    }

    pub(super) fn toggle_left_panel(&mut self, cx: &mut Context<Self>) {
        self.ui_state.left_panel_open = !self.ui_state.left_panel_open;
        let size = size(px(self.window_width), px(self.window_height));
        if self.viewport.set_size(self.center_stage_size(size)) {
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches();
            self.refresh_professional_overlays(cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_bottom_panel(&mut self, cx: &mut Context<Self>) {
        self.ui_state.bottom_panel_open = !self.ui_state.bottom_panel_open;
        let size = size(px(self.window_width), px(self.window_height));
        if self.viewport.set_size(self.center_stage_size(size)) {
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches();
            self.refresh_professional_overlays(cx);
        }
        cx.notify();
    }

    pub(super) fn set_bottom_tab(&mut self, tab: MapViewerBottomTab, cx: &mut Context<Self>) {
        self.ui_state.active_bottom_tab = tab;
        self.ui_state.bottom_panel_open = true;
        if tab == MapViewerBottomTab::Players && self.players.players.is_empty() {
            self.refresh_players(cx);
        }
        if tab == MapViewerBottomTab::History {
            self.refresh_history(cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_context_more(&mut self, cx: &mut Context<Self>) {
        self.ui_state.context_more_open = !self.ui_state.context_more_open;
        cx.notify();
    }

    pub(super) fn toggle_context_paste(&mut self, cx: &mut Context<Self>) {
        self.ui_state.context_paste_open = !self.ui_state.context_paste_open;
        cx.notify();
    }

    pub(super) fn select_chunk_tree_tile_at(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let tile = self.viewport.screen_to_tile(position, self.active_layout);
        self.refresh_chunk_tree_for_tile(tile);
        self.ui_state.active_bottom_tab = MapViewerBottomTab::ChunkTree;
        self.ui_state.bottom_panel_open = true;
        self.status = SharedString::from(format!(
            "已选择瓦片 {}, {} · 区块树按需加载",
            tile.0, tile.1
        ));
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    pub(super) fn begin_right_panel_resize(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.begin_exclusive_pointer_interaction();
        self.ui_state.dock_drag = Some(DockDragState {
            drag: DockDrag::RightPanel,
            start_x: position.x / px(1.0),
            start_y: position.y / px(1.0),
            start_size: self.ui_state.right_panel_width,
        });
        cx.notify();
    }

    pub(super) fn begin_bottom_panel_resize(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.begin_exclusive_pointer_interaction();
        self.ui_state.dock_drag = Some(DockDragState {
            drag: DockDrag::BottomPanel,
            start_x: position.x / px(1.0),
            start_y: position.y / px(1.0),
            start_size: self.ui_state.bottom_panel_height,
        });
        cx.notify();
    }

    pub(super) fn update_dock_drag(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.ui_state.dock_drag else {
            return false;
        };
        match drag.drag {
            DockDrag::RightPanel => {
                let delta = drag.start_x - position.x / px(1.0);
                self.ui_state.right_panel_width =
                    clamp_right_panel_width(drag.start_size + delta, self.window_width);
                let size = size(px(self.window_width), px(self.window_height));
                if self.viewport.set_size(self.center_stage_size(size)) {
                    self.ensure_visible_tiles_throttled(cx);
                    self.professional.pending_overlay_refresh = true;
                    self.schedule_viewport_idle_refresh(cx);
                    let colors = self.theme_colors(cx);
                    self.sync_canvas_snapshot(colors, cx);
                }
            }
            DockDrag::BottomPanel => {
                let delta = drag.start_y - position.y / px(1.0);
                self.ui_state.bottom_panel_height =
                    clamp_bottom_panel_height(drag.start_size + delta, self.window_height);
                let size = size(px(self.window_width), px(self.window_height));
                if self.viewport.set_size(self.center_stage_size(size)) {
                    self.ensure_visible_tiles_throttled(cx);
                    self.professional.pending_overlay_refresh = true;
                    self.schedule_viewport_idle_refresh(cx);
                    let colors = self.theme_colors(cx);
                    self.sync_canvas_snapshot(colors, cx);
                }
            }
        }
        cx.notify();
        true
    }

    pub(super) fn end_dock_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.ui_state.dock_drag.take().is_some() {
            #[cfg(target_os = "windows")]
            self.preview_3d.clear_surface();
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn release_pointer_captures(
        &mut self,
        source: &'static str,
        cx: &mut Context<Self>,
    ) -> bool {
        let release = take_pointer_captures(
            &mut self.drag,
            &mut self.right_selection_drag,
            &mut self.preview_3d.drag_origin,
            &mut self.ui_state.dock_drag,
        );
        let changed = release.changed();
        if changed {
            log_pointer_capture_release(source, release);
            if release.dock_drag {
                #[cfg(target_os = "windows")]
                self.preview_3d.clear_surface();
            }
            cx.notify();
        }
        changed
    }

    pub(super) fn cancel_pointer_captures_for_panel_interaction(
        &mut self,
        source: &'static str,
        cx: &mut Context<Self>,
    ) -> bool {
        let release = take_pointer_captures(
            &mut self.drag,
            &mut self.right_selection_drag,
            &mut self.preview_3d.drag_origin,
            &mut self.ui_state.dock_drag,
        );
        let changed = release.changed();
        if changed {
            log_pointer_capture_release(source, release);
            if release.dock_drag {
                #[cfg(target_os = "windows")]
                self.preview_3d.clear_surface();
            }
            cx.notify();
        }
        changed
    }

    pub(super) fn begin_exclusive_pointer_interaction(&mut self) {
        self.drag = None;
        self.right_selection_drag = None;
        self.preview_3d.drag_origin = None;
        self.ui_state.dock_drag = None;
        self.context_menu = None;
        self.ui_state.top_more_open = false;
    }

    pub(super) fn begin_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.ui_state.dock_drag.is_some() {
            return;
        }
        self.begin_exclusive_pointer_interaction();
        self.drag = Some(DragState {
            start: position,
            offset_x: self.viewport.offset_x,
            offset_y: self.viewport.offset_y,
            moved: false,
        });
        cx.notify();
    }

    pub(super) fn update_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.update_dock_drag(position, cx) {
            return;
        }
        let hover_changed = self.update_hover_block(position);
        let Some(mut drag) = self.drag else {
            if hover_changed {
                let colors = self.theme_colors(cx);
                self.sync_canvas_snapshot(colors, cx);
            }
            return;
        };
        let dx = (position.x - drag.start.x) / px(1.0);
        let dy = (position.y - drag.start.y) / px(1.0);
        if !drag.moved && dx.hypot(dy) > MAP_CLICK_DRAG_THRESHOLD_PX {
            drag.moved = true;
            self.drag = Some(drag);
        }
        self.viewport.offset_x = drag.offset_x + (position.x - drag.start.x) / px(1.0);
        self.viewport.offset_y = drag.offset_y + (position.y - drag.start.y) / px(1.0);
        self.ensure_visible_tiles_throttled(cx);
        self.professional.pending_overlay_refresh = true;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn end_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.end_dock_drag(cx) {
            return;
        }
        if let Some(drag) = self.drag.take() {
            let dx = (position.x - drag.start.x) / px(1.0);
            let dy = (position.y - drag.start.y) / px(1.0);
            if !drag.moved && dx.hypot(dy) <= MAP_CLICK_DRAG_THRESHOLD_PX {
                self.select_chunk_tree_tile_at(position, cx);
                return;
            }
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches();
            self.refresh_professional_overlays(cx);
            cx.notify();
        }
    }

    pub(super) fn chunk_at_stage_position(&self, position: Point<Pixels>) -> ChunkPos {
        let (block_x, block_z) = self.viewport.screen_to_block(position, self.active_layout);
        chunk_from_block(block_x, block_z, self.dimension)
    }

    pub(super) fn set_selection_from_drag(
        &mut self,
        drag: RightSelectionDrag,
        cx: &mut Context<Self>,
    ) {
        self.professional.selection = Some(drag.selection());
        self.professional.selection_stats = None;
        self.clear_paste_preview_state(cx);
        self.invalidate_preview_3d_mesh();
        let bounds = drag.selection().bounds();
        self.status = SharedString::from(format!(
            "已选择 chunk {}..{}, {}..{}",
            bounds.min_chunk_x, bounds.max_chunk_x, bounds.min_chunk_z, bounds.max_chunk_z
        ));
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    pub(super) fn begin_right_selection(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.ui_state.dock_drag.is_some() {
            return;
        }
        self.begin_exclusive_pointer_interaction();
        let local_position = self.stage_local_position(position);
        let chunk = self.chunk_at_stage_position(local_position);
        self.right_selection_drag = Some(RightSelectionDrag::new(local_position, chunk));
        tracing::debug!(
            target: "bmcbl::ui::window::map_viewer::view",
            chunk_x = chunk.x,
            chunk_z = chunk.z,
            "map_viewer right_selection_begin"
        );
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    pub(super) fn update_right_selection(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.right_selection_drag else {
            return;
        };
        let local_position = self.stage_local_position(position);
        let chunk = self.chunk_at_stage_position(local_position);
        if right_selection_moved(
            drag.start_position,
            local_position,
            MAP_CLICK_DRAG_THRESHOLD_PX,
        ) {
            drag.moved = true;
        }
        if drag.current_chunk == chunk && self.professional.selection == Some(drag.selection()) {
            self.right_selection_drag = Some(drag);
            return;
        }
        drag.current_chunk = chunk;
        self.right_selection_drag = Some(drag);
        self.set_selection_from_drag(drag, cx);
        tracing::debug!(
            target: "bmcbl::ui::window::map_viewer::view",
            start_x = drag.start_chunk.x,
            start_z = drag.start_chunk.z,
            current_x = drag.current_chunk.x,
            current_z = drag.current_chunk.z,
            moved = drag.moved,
            "map_viewer right_selection_update"
        );
        cx.notify();
    }

    pub(super) fn end_right_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.right_selection_drag.take() else {
            return;
        };
        let local_position = self.stage_local_position(position);
        drag.current_chunk = self.chunk_at_stage_position(local_position);
        if right_selection_moved(
            drag.start_position,
            local_position,
            MAP_CLICK_DRAG_THRESHOLD_PX,
        ) {
            drag.moved = true;
        }
        self.set_selection_from_drag(drag, cx);
        tracing::debug!(
            target: "bmcbl::ui::window::map_viewer::view",
            start_x = drag.start_chunk.x,
            start_z = drag.start_chunk.z,
            end_x = drag.current_chunk.x,
            end_z = drag.current_chunk.z,
            moved = drag.moved,
            "map_viewer right_selection_end"
        );
        if !drag.moved {
            self.open_context_menu(position, cx);
        } else {
            self.set_selection_from_drag(drag, cx);
            self.open_context_menu(position, cx);
        }
    }

    pub(super) fn schedule_viewport_idle_refresh(&mut self, cx: &mut Context<Self>) {
        self.viewport_idle_generation = self.viewport_idle_generation.saturating_add(1);
        let generation = self.viewport_idle_generation;
        cx.spawn(async move |handle, cx| {
            Timer::after(Duration::from_millis(120)).await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.viewport_idle_generation != generation
                    || this.drag.is_some()
                    || this.right_selection_drag.is_some()
                    || this.ui_state.dock_drag.is_some()
                {
                    return;
                }
                this.refresh_professional_render_caches();
                this.refresh_professional_overlays(cx);
                let colors = this.theme_colors(cx);
                this.sync_canvas_snapshot(colors, cx);
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn sync_input_values(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (center_x, center_z) = self.viewport.center_block(self.active_layout);
        self.sync_input_value(MapInputField::CenterX, center_x.to_string(), window, cx);
        self.sync_input_value(MapInputField::CenterZ, center_z.to_string(), window, cx);
        self.sync_input_value(
            MapInputField::ZoomPercent,
            format!("{:.0}", self.viewport.scale * 100.0),
            window,
            cx,
        );
        self.sync_input_value(
            MapInputField::DimensionId,
            self.dimension.id().to_string(),
            window,
            cx,
        );
    }

    pub(super) fn sync_input_value(
        &mut self,
        field: MapInputField,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_fields.focused_field == Some(field)
            || self.input_fields.dirty_fields.contains(&field)
        {
            return;
        }
        let input = self.input_fields.entity(field).clone();
        if input.read(cx).value().as_ref() == value {
            return;
        }
        input.update(cx, |input, cx| {
            input.set_value(SharedString::from(value), window, cx);
        });
    }

    pub(super) fn handle_map_input_event(
        &mut self,
        field: MapInputField,
        input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Focus => {
                self.input_fields.focused_field = Some(field);
                cx.notify();
            }
            InputEvent::Change => {
                self.input_fields.dirty_fields.insert(field);
                if self.input_fields.validation.invalid_field == Some(field) {
                    self.input_fields.validation = InputValidationState::default();
                }
                cx.notify();
            }
            InputEvent::Blur => {
                let value = input.read(cx).value();
                self.apply_map_input_value(field, value, cx);
                if self.input_fields.focused_field == Some(field) {
                    self.input_fields.focused_field = None;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                let value = input.read(cx).value();
                self.apply_map_input_value(field, value, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn apply_map_input_value(
        &mut self,
        field: MapInputField,
        value: SharedString,
        cx: &mut Context<Self>,
    ) {
        let trimmed = value.as_ref().trim();
        let result = match field {
            MapInputField::CenterX => parse_i32_input(trimmed, "X").map(|block_x| {
                let (_, block_z) = self.viewport.center_block(self.active_layout);
                self.viewport
                    .center_on_block(block_x, block_z, self.active_layout);
                self.context_menu = None;
                self.ensure_visible_tiles(cx);
                format!("已跳转到 X {block_x}")
            }),
            MapInputField::CenterZ => parse_i32_input(trimmed, "Z").map(|block_z| {
                let (block_x, _) = self.viewport.center_block(self.active_layout);
                self.viewport
                    .center_on_block(block_x, block_z, self.active_layout);
                self.context_menu = None;
                self.ensure_visible_tiles(cx);
                format!("已跳转到 Z {block_z}")
            }),
            MapInputField::ZoomPercent => parse_zoom_scale(trimmed).map(|scale| {
                let factor = scale / self.viewport.scale.max(f32::EPSILON);
                self.zoom_by_center(factor, cx);
                format!("缩放已设为 {:.0}%", self.viewport.scale * 100.0)
            }),
            MapInputField::DimensionId => parse_i32_input(trimmed, "维度ID").map(|dimension_id| {
                self.custom_dimension_id = dimension_id;
                let dimension = Dimension::from_id(dimension_id);
                if self.dimension != dimension {
                    self.set_dimension(dimension, cx);
                }
                format!("维度已设为 {}", dimension_label(dimension))
            }),
        };

        match result {
            Ok(message) => {
                self.input_fields.dirty_fields.remove(&field);
                if self.input_fields.validation.invalid_field == Some(field) {
                    self.input_fields.validation = InputValidationState::default();
                }
                self.status = SharedString::from(message);
            }
            Err(message) => {
                self.input_fields.validation.invalid_field = Some(field);
                self.input_fields.validation.message = Some(message.clone());
                self.input_fields.dirty_fields.insert(field);
                self.status = message;
            }
        }
    }

    pub(super) fn update_hover_block(&mut self, position: Point<Pixels>) -> bool {
        let (block_x, block_z) = self.viewport.screen_to_block(position, self.active_layout);
        let changed = self.hover_block_x != block_x || self.hover_block_z != block_z;
        self.hover_block_x = block_x;
        self.hover_block_z = block_z;
        changed
    }

    pub(super) fn hover_chunk_pos(&self) -> ChunkPos {
        chunk_from_block(self.hover_block_x, self.hover_block_z, self.dimension)
    }

    pub(super) fn active_target_chunk_pos(&self) -> ChunkPos {
        self.context_chunk_pos()
            .unwrap_or_else(|| self.hover_chunk_pos())
    }

    pub(super) fn viewport_center_chunk_pos(&self) -> ChunkPos {
        let (block_x, block_z) = self.viewport.center_block(self.active_layout);
        chunk_from_block(block_x, block_z, self.dimension)
    }

    fn center_paste_preview_in_view(&mut self, cx: &mut Context<Self>) {
        let Some((center_block_x, center_block_z)) = self.paste_preview_center_block() else {
            return;
        };
        self.viewport
            .center_on_block(center_block_x, center_block_z, self.active_layout);
        self.ensure_visible_tiles(cx);
        self.professional.pending_overlay_refresh = true;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }

    fn paste_preview_center_block(&self) -> Option<(i32, i32)> {
        let preview = self.professional.paste_preview.as_ref()?;
        let min_x = preview.targets.iter().map(|chunk| chunk.x).min()?;
        let max_x = preview.targets.iter().map(|chunk| chunk.x).max()?;
        let min_z = preview.targets.iter().map(|chunk| chunk.z).min()?;
        let max_z = preview.targets.iter().map(|chunk| chunk.z).max()?;
        let center_x = min_x
            .saturating_add(max_x)
            .saturating_add(1)
            .saturating_mul(8);
        let center_z = min_z
            .saturating_add(max_z)
            .saturating_add(1)
            .saturating_mul(8);
        Some((center_x, center_z))
    }

    pub(super) fn text_input_focused(&self, window: &Window, cx: &App) -> bool {
        let map_input_focused = [
            MapInputField::CenterX,
            MapInputField::CenterZ,
            MapInputField::ZoomPercent,
            MapInputField::DimensionId,
        ]
        .into_iter()
        .any(|field| {
            self.input_fields
                .entity(field)
                .focus_handle(cx)
                .is_focused(window)
        });

        self.editor_state.focus_handle(cx).is_focused(window) || map_input_focused
    }

    pub(super) fn rebuild_paste_preview_images(&mut self, cx: &mut Context<Self>) {
        self.replace_paste_preview_images(self.build_paste_preview_images(), cx);
    }

    pub(super) fn replace_paste_preview_images(
        &mut self,
        images: Vec<PastePreviewImage>,
        cx: &mut Context<Self>,
    ) {
        replace_paste_preview_image_set(&mut self.paste_preview_images, images);
    }

    fn clear_paste_preview_state(&mut self, cx: &mut Context<Self>) {
        self.professional.paste_preview = None;
        self.replace_paste_preview_images(Vec::new(), cx);
    }

    fn build_paste_preview_images(&self) -> Vec<PastePreviewImage> {
        let Some(preview) = self.professional.paste_preview.as_ref() else {
            return Vec::new();
        };
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            return Vec::new();
        };
        if copied_chunk.chunk_count() > import_preview::PREVIEW_IMAGE_CHUNK_LIMIT {
            return Vec::new();
        }
        let source_bounds = copied_chunk_chunk_bounds(copied_chunk);
        let delta_bounds = copied_chunk
            .chunk_delta_bounds(preview.source_anchor)
            .unwrap_or((0, 0, 0, 0));
        copied_chunk
            .chunks
            .iter()
            .filter_map(|chunk| {
                let source_image = self
                    .professional
                    .copied_chunk_preview_images
                    .get(&chunk.chunk)
                    .cloned()
                    .or_else(|| {
                        source_bounds.map(|bounds| {
                            fallback_copied_chunk_preview_image(
                                chunk.chunk,
                                preview.source_anchor,
                                bounds,
                            )
                        })
                    })?;
                let delta_x = chunk.chunk.x.saturating_sub(preview.source_anchor.x);
                let delta_z = chunk.chunk.z.saturating_sub(preview.source_anchor.z);
                let (target_delta_x, target_delta_z) =
                    preview
                        .transform
                        .transform_delta_in_bounds(delta_x, delta_z, delta_bounds);
                let target = ChunkPos {
                    x: preview.target_anchor.x.saturating_add(target_delta_x),
                    z: preview.target_anchor.z.saturating_add(target_delta_z),
                    dimension: preview.target_anchor.dimension,
                };
                paste_preview_image_for_chunk(&source_image, target, preview.transform)
            })
            .collect()
    }

    fn copied_chunk_preview_images_for_chunks(
        &self,
        chunks: &[ChunkPos],
    ) -> BTreeMap<ChunkPos, CopiedChunkPreviewImage> {
        chunks
            .iter()
            .filter_map(|chunk| self.copied_chunk_preview_image_from_canvas(*chunk))
            .map(|image| (image.chunk, image))
            .collect()
    }

    fn copied_chunk_preview_image_from_canvas(
        &self,
        source: ChunkPos,
    ) -> Option<CopiedChunkPreviewImage> {
        let chunks_per_tile = i32::try_from(self.active_layout.chunks_per_tile)
            .ok()?
            .max(1);
        let tile_coord = (
            source.x.div_euclid(chunks_per_tile),
            source.z.div_euclid(chunks_per_tile),
        );
        let tile = self
            .canvas_tile_snapshot
            .tiles
            .iter()
            .find(|tile| tile.coord == tile_coord)?;
        copy_chunk_preview_image_from_tile(
            source,
            tile.coord,
            chunks_per_tile,
            tile.pixels.as_ref()?,
            tile.pixel_format?,
            tile.width,
            tile.height,
        )
    }

    fn set_paste_preview(
        &mut self,
        target_anchor: ChunkPos,
        transform: PasteTransform,
        display_degrees: f32,
        drag: Option<PastePreviewDrag>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            self.status = SharedString::from("没有已复制的 chunk");
            toast::error(cx, self.status.clone());
            cx.notify();
            return false;
        };
        let source_anchor = copied_chunk.anchor_chunk();
        let tools_expanded = self
            .professional
            .paste_preview
            .as_ref()
            .is_some_and(|preview| preview.tools_expanded);
        let auto_pan = self
            .professional
            .paste_preview
            .as_ref()
            .and_then(|preview| preview.auto_pan);
        let targets = pasted_chunk_targets(copied_chunk, source_anchor, target_anchor, transform);
        self.professional.paste_preview = Some(PastePreview {
            source_anchor,
            target_anchor,
            rotation: transform.rotation,
            transform,
            display_degrees,
            drag,
            targets,
            tools_expanded,
            auto_pan,
        });
        self.professional.pending_quick_write_confirmation = None;
        self.context_menu = None;
        self.ui_state.context_paste_open = false;
        self.rebuild_paste_preview_images(cx);
        self.sync_import_preview_3d_transform(transform);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        true
    }

    pub(super) fn start_paste_preview_from_keyboard(&mut self, cx: &mut Context<Self>) {
        let target = self.viewport_center_chunk_pos();
        if !self.set_paste_preview(
            target,
            PasteTransform::default(),
            paste_rotation_degrees(PasteRotation::NoRotation),
            None,
            cx,
        ) {
            toast::error(cx, SharedString::from("没有可粘贴的区块副本"));
            return;
        }
        self.status = SharedString::from(
            "粘贴预览：点击预览区域或“移动”拖动定位，↺/↻ 旋转，Enter/确认写入，Esc/取消关闭",
        );
        cx.notify();
    }

    pub(super) fn rotate_paste_preview(&mut self, clockwise: bool, cx: &mut Context<Self>) {
        let Some(preview) = self.professional.paste_preview.as_ref() else {
            self.start_paste_preview_from_keyboard(cx);
            return;
        };
        let transform = if clockwise {
            preview.transform.rotate_clockwise()
        } else {
            preview.transform.rotate_counter_clockwise()
        };
        let target = preview.target_anchor;
        if !self.set_paste_preview(
            target,
            transform,
            paste_rotation_degrees(transform.rotation),
            None,
            cx,
        ) {
            return;
        }
        self.status = SharedString::from(format!(
            "粘贴预览：{} · 可拖动定位，Enter 写入，Esc 取消",
            transform.label()
        ));
        cx.notify();
    }

    pub(super) fn toggle_paste_preview_mirror_x(&mut self, cx: &mut Context<Self>) {
        self.transform_paste_preview(|transform| transform.toggle_mirror_x(), cx);
    }

    pub(super) fn toggle_paste_preview_mirror_z(&mut self, cx: &mut Context<Self>) {
        self.transform_paste_preview(|transform| transform.toggle_mirror_z(), cx);
    }

    pub(super) fn toggle_paste_preview_tools(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.professional.paste_preview.as_mut() else {
            return;
        };
        preview.tools_expanded = !preview.tools_expanded;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    fn transform_paste_preview(
        &mut self,
        update: impl FnOnce(PasteTransform) -> PasteTransform,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.professional.paste_preview.clone() else {
            self.start_paste_preview_from_keyboard(cx);
            return;
        };
        let transform = update(preview.transform);
        if !self.set_paste_preview(
            preview.target_anchor,
            transform,
            paste_rotation_degrees(transform.rotation),
            preview.drag,
            cx,
        ) {
            return;
        }
        self.status = SharedString::from(format!(
            "粘贴预览：{} · 可拖动定位，Enter 写入，Esc 取消",
            transform.label()
        ));
        cx.notify();
    }

    pub(super) fn confirm_paste_preview(&mut self, cx: &mut Context<Self>) {
        self.snap_paste_preview_rotation(cx);
        let Some(preview) = self.professional.paste_preview.clone() else {
            return;
        };
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            self.status = SharedString::from("没有已复制的 chunk");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        };
        let action = if self.professional.imported_structure.is_some() {
            QuickWriteAction::PasteImportedStructure {
                source_anchor: preview.source_anchor,
                target_anchor: preview.target_anchor,
                chunk_count: copied_chunk.chunk_count(),
                transform: preview.transform,
            }
        } else {
            QuickWriteAction::PasteCopiedChunks {
                source_anchor: preview.source_anchor,
                target_anchor: preview.target_anchor,
                chunk_count: copied_chunk.chunk_count(),
                transform: preview.transform,
            }
        };
        self.professional.pending_quick_write_confirmation = Some(action.clone());
        self.run_quick_write_action(action, cx);
    }

    pub(super) fn cancel_paste_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let changed =
            self.professional.paste_preview.is_some() || !self.paste_preview_images.is_empty();
        self.clear_paste_preview_state(cx);
        self.professional.pending_quick_write_confirmation = None;
        if changed {
            self.status = SharedString::from("已取消粘贴预览");
            let colors = self.theme_colors(cx);
            self.sync_canvas_snapshot(colors, cx);
            cx.notify();
        }
        changed
    }

    fn update_paste_preview_target_from_hover(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(preview) = self.professional.paste_preview.as_ref() else {
            return false;
        };
        let target = self.hover_chunk_pos();
        if target == preview.target_anchor {
            return false;
        }
        let transform = preview.transform;
        let display_degrees = preview.display_degrees;
        let drag = preview.drag;
        self.set_paste_preview(target, transform, display_degrees, drag, cx)
    }

    fn move_paste_preview_to_stage_position(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.professional.paste_preview.is_none() {
            return false;
        }
        let auto_pan = self.paste_preview_auto_pan_for_position(position);
        if let Some(preview) = self.professional.paste_preview.as_mut() {
            preview.auto_pan = auto_pan;
        }
        let changed_hover = self.update_hover_block(position);
        if changed_hover {
            return self.update_paste_preview_target_from_hover(cx);
        }
        false
    }

    fn paste_preview_auto_pan_for_position(
        &self,
        position: Point<Pixels>,
    ) -> Option<PastePreviewAutoPan> {
        const EDGE_PX: f32 = 44.0;
        const MAX_SPEED_PX: f32 = 34.0;
        let x = position.x / px(1.0);
        let y = position.y / px(1.0);
        let edge_speed = |value: f32, max: f32| -> f32 {
            if value < EDGE_PX {
                ((EDGE_PX - value) / EDGE_PX).clamp(0.0, 1.0) * MAX_SPEED_PX
            } else if value > max - EDGE_PX {
                -((value - (max - EDGE_PX)) / EDGE_PX).clamp(0.0, 1.0) * MAX_SPEED_PX
            } else {
                0.0
            }
        };
        let velocity_x = edge_speed(x, self.viewport.width);
        let velocity_y = edge_speed(y, self.viewport.height);
        if velocity_x.abs() < 0.1 && velocity_y.abs() < 0.1 {
            return None;
        }
        Some(PastePreviewAutoPan {
            velocity_x,
            velocity_y,
            local_position: position,
        })
    }

    pub(super) fn tick_paste_preview_auto_pan(&mut self, cx: &mut Context<Self>) -> bool {
        let auto_pan = self
            .professional
            .paste_preview
            .as_ref()
            .and_then(|preview| preview.auto_pan);
        let Some(auto_pan) = auto_pan else {
            return false;
        };
        self.viewport.offset_x += auto_pan.velocity_x;
        self.viewport.offset_y += auto_pan.velocity_y;
        self.ensure_visible_tiles_throttled(cx);
        self.professional.pending_overlay_refresh = true;
        self.move_paste_preview_to_stage_position(auto_pan.local_position, cx);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        true
    }

    fn begin_paste_preview_move_at(
        &mut self,
        window_position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(preview) = self.professional.paste_preview.as_ref() else {
            return false;
        };
        let local_position = self.stage_local_position(window_position);
        let chunk = self.chunk_at_stage_position(local_position);
        if !preview.targets.contains(&chunk) {
            return false;
        }
        self.begin_exclusive_pointer_interaction();
        self.set_paste_preview_drag(Some(PastePreviewDrag::Move), cx);
        self.move_paste_preview_to_stage_position(local_position, cx);
        self.status = SharedString::from("粘贴预览移动中：拖动预览区域定位，松开后停止移动");
        cx.notify();
        true
    }

    fn begin_paste_preview_move_from_toolbar(&mut self, cx: &mut Context<Self>) -> bool {
        if self.professional.paste_preview.is_none() {
            return false;
        }
        self.begin_exclusive_pointer_interaction();
        self.set_paste_preview_drag(Some(PastePreviewDrag::Move), cx);
        self.status = SharedString::from("粘贴预览移动中：拖动预览区域定位，松开后停止移动");
        cx.notify();
        true
    }

    fn set_paste_preview_drag(&mut self, drag: Option<PastePreviewDrag>, cx: &mut Context<Self>) {
        let Some(preview) = self.professional.paste_preview.as_mut() else {
            return;
        };
        preview.drag = drag;
        if drag.is_none() {
            preview.auto_pan = None;
        }
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    fn snap_paste_preview_rotation(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(preview) = self.professional.paste_preview.as_mut() else {
            return false;
        };
        let rotation = snapped_paste_rotation(preview.display_degrees);
        preview.rotation = rotation;
        preview.transform.rotation = rotation;
        preview.display_degrees = paste_rotation_degrees(rotation);
        preview.drag = None;
        let source_anchor = preview.source_anchor;
        let target_anchor = preview.target_anchor;
        let copied_chunk = self.professional.copied_chunk.as_ref();
        preview.targets = copied_chunk
            .map(|copied_chunk| {
                pasted_chunk_targets(
                    copied_chunk,
                    source_anchor,
                    target_anchor,
                    preview.transform,
                )
            })
            .unwrap_or_default();
        let transform = preview.transform;
        self.rebuild_paste_preview_images(cx);
        self.sync_import_preview_3d_transform(transform);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
        true
    }

    pub(super) fn sync_import_preview_3d_transform(&mut self, transform: PasteTransform) {
        if self.professional.imported_structure.is_none()
            && !self.professional.imported_region_package
        {
            return;
        }
        self.preview_3d.model_rotation.yaw = paste_rotation_radians(transform.rotation);
        self.preview_3d.model_rotation.mirror_x = transform.mirror_x;
        self.preview_3d.model_rotation.mirror_z = transform.mirror_z;
        #[cfg(target_os = "windows")]
        self.preview_3d.clear_surface();
    }

    pub(super) fn open_context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let local_position = self.stage_local_position(position);
        let (block_x, block_z) = self
            .viewport
            .screen_to_block(local_position, self.active_layout);
        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;
        self.ui_state.context_paste_open = false;
        self.context_menu = Some(ContextMenuState {
            position,
            block_x,
            block_z,
        });
        self.hover_block_x = block_x;
        self.hover_block_z = block_z;
        if self.professional.selection.is_none() {
            let chunk = chunk_from_block(block_x, block_z, self.dimension);
            self.professional.selection = Some(ChunkSelection {
                start: chunk,
                end: chunk,
            });
            self.professional.selection_stats = None;
        }
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        let changed = self.context_menu.take().is_some()
            || self.ui_state.context_more_open
            || self.ui_state.context_paste_open;
        self.ui_state.context_more_open = false;
        self.ui_state.context_paste_open = false;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn handle_canvas_action(&mut self, action: MapCanvasAction, cx: &mut Context<Self>) {
        match action {
            MapCanvasAction::BeginDrag(position) => {
                if !self.begin_paste_preview_move_at(position, cx) {
                    self.begin_drag(self.stage_local_position(position), cx);
                }
            }
            MapCanvasAction::EndDrag(position) => {
                let paste_drag = self
                    .professional
                    .paste_preview
                    .as_ref()
                    .and_then(|preview| preview.drag);
                if matches!(paste_drag, Some(PastePreviewDrag::Move)) {
                    self.set_paste_preview_drag(None, cx);
                } else if self.drag.is_some() || self.ui_state.dock_drag.is_some() {
                    self.end_drag(self.stage_local_position(position), cx);
                } else {
                    self.release_pointer_captures("map canvas left mouse up", cx);
                }
            }
            MapCanvasAction::ZoomAt(position, factor) => {
                self.zoom_at(self.stage_local_position(position), factor, cx);
            }
            MapCanvasAction::BeginRightSelection(position) => {
                if self.professional.paste_preview.is_some() {
                    self.close_context_menu(cx);
                } else {
                    self.begin_right_selection(position, cx)
                }
            }
            MapCanvasAction::EndRightSelection(position) => {
                if self.right_selection_drag.is_some() {
                    self.end_right_selection(position, cx);
                } else {
                    self.release_pointer_captures("map canvas right mouse up", cx);
                }
            }
            MapCanvasAction::PointerMoved {
                position,
                pressed_button,
            } => self.handle_canvas_pointer_moved(position, pressed_button, cx),
            MapCanvasAction::BeginPastePreviewMove => {
                self.begin_paste_preview_move_from_toolbar(cx);
            }
            MapCanvasAction::ConfirmPastePreview => self.confirm_paste_preview(cx),
            MapCanvasAction::CancelPastePreview => {
                self.cancel_paste_preview(cx);
            }
            MapCanvasAction::RotatePastePreviewClockwise => self.rotate_paste_preview(true, cx),
            MapCanvasAction::RotatePastePreviewCounterClockwise => {
                self.rotate_paste_preview(false, cx)
            }
            MapCanvasAction::MirrorPastePreviewX => self.toggle_paste_preview_mirror_x(cx),
            MapCanvasAction::MirrorPastePreviewZ => self.toggle_paste_preview_mirror_z(cx),
            MapCanvasAction::TogglePastePreviewTools => self.toggle_paste_preview_tools(cx),
            MapCanvasAction::ExportPastePreviewImage => self.export_chunks_image(cx),
            MapCanvasAction::OpenPastePreview3d => {
                self.show_right_preview_3d_panel(cx);
                self.refresh_import_preview_3d(cx);
            }
        }
    }

    pub(super) fn handle_canvas_pointer_moved(
        &mut self,
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self
            .professional
            .paste_preview
            .as_ref()
            .and_then(|preview| preview.drag)
        {
            match (drag, pressed_button) {
                (PastePreviewDrag::Move, Some(MouseButton::Left)) => {
                    let local_position = self.stage_local_position(position);
                    if self.move_paste_preview_to_stage_position(local_position, cx) {
                        return;
                    }
                }
                (_, None) => {
                    self.set_paste_preview_drag(None, cx);
                    return;
                }
                _ => {}
            }
        }
        match canvas_pointer_move_action(
            pressed_button,
            self.drag.is_some(),
            self.right_selection_drag.is_some(),
            self.preview_3d.drag_origin.is_some(),
            self.ui_state.dock_drag.is_some(),
        ) {
            CanvasPointerMoveAction::UpdateMapPointer => {
                let position = if self.ui_state.dock_drag.is_some() {
                    position
                } else {
                    self.stage_local_position(position)
                };
                self.update_drag(position, cx);
            }
            CanvasPointerMoveAction::UpdateRightSelection => {
                self.update_right_selection(position, cx);
            }
            CanvasPointerMoveAction::Ignore => {}
            CanvasPointerMoveAction::ReleaseStaleCaptures => {
                let hover_changed = self.update_hover_block(self.stage_local_position(position));
                let released = self
                    .release_pointer_captures("map canvas mouse move without pressed button", cx);
                if hover_changed && !released {
                    let colors = self.theme_colors(cx);
                    self.sync_canvas_snapshot(colors, cx);
                }
            }
        }
    }

    pub(super) fn copy_context_tp(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu {
            cx.write_to_clipboard(ClipboardItem::new_string(format!(
                "/tp {} ~ {}",
                menu.block_x, menu.block_z
            )));
            self.status = SharedString::from("已复制传送命令");
        }
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn export_selection_as_obj(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("没有可导出的选区");
            cx.notify();
            return;
        };
        let default_file_name = format!(
            "chunk-selection-{}-{}-{}-{}.obj",
            selection.bounds().min_chunk_x,
            selection.bounds().min_chunk_z,
            selection.bounds().max_chunk_x,
            selection.bounds().max_chunk_z
        );
        let Some(path) = pick_save_path_with_filter("Wavefront OBJ", &["obj"], &default_file_name)
        else {
            self.status = SharedString::from("已取消导出 OBJ");
            cx.notify();
            return;
        };
        let path = PathBuf::from(path);
        let chunk_count = selection.bounds().chunk_count().max(1);
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导出 OBJ"),
            completed: 0,
            total: chunk_count.saturating_add(2),
        });
        self.begin_edit_toast(
            SharedString::from(format!("正在导出 {chunk_count} 个区块 OBJ...")),
            cx,
        );
        self.status = SharedString::from("正在导出选区 OBJ...");
        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let bounds = selection.bounds();
        let package_paths = preview_3d_resource_package_paths(&world_path, cx);
        cx.notify();

        cx.spawn(async move |handle, cx| {
            enum ObjExportEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<ObjExportComplete, String>),
            }

            struct ObjExportComplete {
                export_dir: PathBuf,
                material_count: usize,
                textured_material_count: usize,
            }

            let (event_sender, mut event_receiver) = unbounded::<ObjExportEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task = cx.background_spawn(async move {
                let result = (|| {
                    let mut resolved_package_paths =
                        bedrock_block_model::world_resource_pack_paths(&world_path);
                    for package_path in package_paths {
                        bedrock_block_model::push_unique_resource_pack_path(
                            &mut resolved_package_paths,
                            package_path,
                        );
                    }
                    for package_path in
                        crate::core::minecraft::paths::discover_local_package_roots_with_vanilla()
                    {
                        bedrock_block_model::push_unique_resource_pack_path(
                            &mut resolved_package_paths,
                            package_path,
                        );
                    }
                    let block_models = bedrock_block_model::BlockModelRepository::load_packs(
                        resolved_package_paths.iter().map(PathBuf::as_path),
                    )
                    .map(Arc::new)
                    .map_err(|error| format!("加载方块模型资源失败：{error}"))?;

                    let mesh = load_preview_3d_mesh_blocking_incremental_with_block_models(
                        &world_path,
                        bounds,
                        Some(block_models),
                        None,
                        {
                            let progress_sender = progress_sender.clone();
                            move |mesh, _status| {
                                let completed = mesh.processed_chunk_count.min(chunk_count);
                                if progress_sender
                                    .unbounded_send(ObjExportEvent::Progress(
                                        ChunkTransferProgress {
                                            phase: SharedString::from("构建 OBJ 模型"),
                                            completed,
                                            total: chunk_count.saturating_add(2),
                                        },
                                    ))
                                    .is_err()
                                {
                                    tracing::debug!("obj export mesh progress receiver dropped");
                                }
                            }
                        },
                    )
                    .map_err(|error| error.to_string())?;
                    if progress_sender
                        .unbounded_send(ObjExportEvent::Progress(ChunkTransferProgress {
                            phase: SharedString::from("生成 OBJ 文本"),
                            completed: chunk_count,
                            total: chunk_count.saturating_add(2),
                        }))
                        .is_err()
                    {
                        tracing::debug!("obj export text progress receiver dropped");
                    }
                    let export_target = bedrock_block_model::ObjExportTarget::from_obj_path(&path)
                        .map_err(|error| error.to_string())?;
                    let texture_directory_name = "textures";
                    let export = export_preview_3d_obj_with_materials_with_progress(
                        &mesh,
                        &export_target.material_library_name,
                        texture_directory_name,
                        &resolved_package_paths,
                        |completed, total| {
                            let scaled = usize::from(total == 0 || completed >= total);
                            if progress_sender
                                .unbounded_send(ObjExportEvent::Progress(ChunkTransferProgress {
                                    phase: SharedString::from("生成 OBJ 文本"),
                                    completed: chunk_count.saturating_add(scaled.min(1)),
                                    total: chunk_count.saturating_add(2),
                                }))
                                .is_err()
                            {
                                tracing::debug!("obj export text progress receiver dropped");
                            }
                        },
                    );
                    if progress_sender
                        .unbounded_send(ObjExportEvent::Progress(ChunkTransferProgress {
                            phase: SharedString::from("写入 OBJ 文件"),
                            completed: chunk_count.saturating_add(1),
                            total: chunk_count.saturating_add(2),
                        }))
                        .is_err()
                    {
                        tracing::debug!("obj export write progress receiver dropped");
                    }
                    bedrock_block_model::write_obj_export_files(
                        &export,
                        &export_target.obj_path,
                        &export_target.material_library_path,
                        &export_target.export_root,
                    )
                    .map_err(|error| error.to_string())?;
                    Ok::<_, String>(ObjExportComplete {
                        export_dir: export_target.export_root,
                        material_count: export.material_count,
                        textured_material_count: export.textured_material_count,
                    })
                })();
                if completion_sender
                    .unbounded_send(ObjExportEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("obj export completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, ObjExportEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != generation {
                        return;
                    }
                    match event {
                        ObjExportEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        ObjExportEvent::Complete(result) => match result {
                            Ok(complete) => {
                                this.complete_chunk_transfer_progress();
                                let message = if complete.material_count > 0
                                    && complete.textured_material_count == 0
                                {
                                    SharedString::from(format!(
                                        "已导出选区 OBJ: {}（未找到 vanilla 材质，textures 为空）",
                                        complete.export_dir.display()
                                    ))
                                } else {
                                    SharedString::from(format!(
                                        "已导出选区 OBJ: {}（材质 {}/{}）",
                                        complete.export_dir.display(),
                                        complete.textured_material_count,
                                        complete.material_count
                                    ))
                                };
                                this.status = message.clone();
                                this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                this.status = SharedString::from(error.clone());
                                this.resolve_edit_toast(
                                    toast::ToastKind::Error,
                                    SharedString::from(error),
                                    cx,
                                );
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn export_selection_region_package(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("没有可导出的选区");
            cx.notify();
            return;
        };
        let chunks = selection.chunks();
        if chunks.is_empty() {
            self.status = SharedString::from("没有可导出的 chunk");
            cx.notify();
            return;
        }
        let default_file_name = region_package::default_region_package_file_name(selection);
        let Some(path) = pick_save_path_with_filter(
            "BMCBL Region",
            &[region_package::REGION_PACKAGE_EXTENSION, "bmcbl-region"],
            &default_file_name,
        ) else {
            self.status = SharedString::from("已取消导出区域包");
            cx.notify();
            return;
        };
        let path = PathBuf::from(path);
        let source_anchor = chunks[0];
        let chunk_count = chunks.len();
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导出区域包"),
            completed: 0,
            total: chunk_count,
        });
        self.begin_edit_toast(
            SharedString::from(format!("正在导出 {chunk_count} 个 chunk 区域包...")),
            cx,
        );
        self.status = SharedString::from("正在导出跨地图区域包...");
        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        cx.notify();

        cx.spawn(async move |handle, cx| {
            enum RegionPackageExportEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<PathBuf, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<RegionPackageExportEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task = cx.background_spawn(async move {
                let result = (|| {
                    let bytes = region_package::export_region_package_blocking(
                        &world_path,
                        source_anchor,
                        chunks,
                        |progress| {
                            if progress_sender
                                .unbounded_send(RegionPackageExportEvent::Progress(progress))
                                .is_err()
                            {
                                tracing::debug!("region package export progress receiver dropped");
                            }
                        },
                    )?;
                    region_package::write_region_package(&path, &bytes)?;
                    Ok::<PathBuf, String>(path)
                })();
                if completion_sender
                    .unbounded_send(RegionPackageExportEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("region package export completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, RegionPackageExportEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != generation {
                        return;
                    }
                    match event {
                        RegionPackageExportEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        RegionPackageExportEvent::Complete(result) => match result {
                            Ok(path) => {
                                this.complete_chunk_transfer_progress();
                                let message = SharedString::from(format!(
                                    "已导出跨地图区域包: {}",
                                    path.display()
                                ));
                                this.status = message.clone();
                                this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                this.status = SharedString::from(error.clone());
                                this.resolve_edit_toast(
                                    toast::ToastKind::Error,
                                    SharedString::from(error),
                                    cx,
                                );
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn import_structure_paths_from_drop(
        &mut self,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            self.status = SharedString::from("未检测到可导入文件");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        }
        let target = self.active_target_chunk_pos();
        let Some(path) = paths
            .iter()
            .find(|path| region_package::is_region_package_path(path))
            .cloned()
        else {
            if let Some(path) = paths
                .iter()
                .find(|path| mcstructure::is_mcstructure_path(path))
                .cloned()
            {
                self.import_mcstructure_at(path, target, cx);
            } else {
                self.status =
                    SharedString::from("拖拽文件不支持，请使用 .bmcblregion 或 .mcstructure");
                toast::error(cx, self.status.clone());
                cx.notify();
            }
            return;
        };
        self.import_region_package_at(path, target, cx);
    }

    pub(super) fn open_import_structure_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(path) = pick_file_path_with_filter(
            "Bedrock Region / Structure",
            &[
                region_package::REGION_PACKAGE_EXTENSION,
                "bmcbl-region",
                mcstructure::MCSTRUCTURE_EXTENSION,
            ],
        ) else {
            self.status = SharedString::from("已取消导入区域/结构文件");
            cx.notify();
            return;
        };
        let target = self.active_target_chunk_pos();
        let path = PathBuf::from(path);
        if region_package::is_region_package_path(&path) {
            self.import_region_package_at(path, target, cx);
        } else if mcstructure::is_mcstructure_path(&path) {
            self.import_mcstructure_at(path, target, cx);
        } else {
            self.status = SharedString::from("不支持的导入文件类型");
            toast::error(cx, self.status.clone());
            cx.notify();
        }
    }

    fn import_region_package_at(
        &mut self,
        path: PathBuf,
        target: ChunkPos,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导入区域包"),
            completed: 0,
            total: 1,
        });
        self.begin_edit_toast(SharedString::from("正在导入跨地图区域包..."), cx);
        self.status = SharedString::from(format!(
            "正在导入区域包到 chunk {},{}...",
            target.x, target.z
        ));
        let generation = self.metadata_generation;
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    region_package::read_region_package(&path)
                        .map(|copied_chunk| (path, copied_chunk))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                match result {
                    Ok((path, copied_chunk)) => {
                        this.complete_chunk_transfer_progress();
                        let chunk_count = copied_chunk.chunk_count();
                        let preview_images =
                            import_preview::copied_chunk_preview_images_for_import(&copied_chunk)
                                .unwrap_or_default();
                        this.professional.copied_chunk = Some(copied_chunk);
                        this.professional.imported_region_package = true;
                        this.professional.imported_structure = None;
                        this.professional.copied_chunk_preview_images = preview_images;
                        this.clear_paste_preview_state(cx);
                        this.invalidate_preview_3d_mesh();
                        if this.set_paste_preview(
                            target,
                            PasteTransform::default(),
                            paste_rotation_degrees(PasteRotation::NoRotation),
                            None,
                            cx,
                        ) {
                            this.center_paste_preview_in_view(cx);
                            let message = SharedString::from(format!(
                                "已导入 {} 个 chunk 区域包，目标 {},{} · 确认后写入",
                                chunk_count, target.x, target.z
                            ));
                            this.status = message.clone();
                            this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                "bmcbl-region {}",
                                path.display()
                            )));
                        }
                    }
                    Err(error) => {
                        this.finish_chunk_transfer_progress();
                        this.status = SharedString::from(error.clone());
                        this.resolve_edit_toast(
                            toast::ToastKind::Error,
                            SharedString::from(error),
                            cx,
                        );
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn import_mcstructure_at(&mut self, path: PathBuf, target: ChunkPos, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导入结构"),
            completed: 0,
            total: 1,
        });
        self.begin_edit_toast(SharedString::from("正在导入 .mcstructure..."), cx);
        self.status = SharedString::from(format!(
            "正在导入结构到 chunk {},{}，Y {}...",
            target.x, target.z, self.y_layer
        ));
        let generation = self.metadata_generation;
        let origin_y = self.y_layer;
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let import =
                        mcstructure::read_mcstructure_as_copied_chunk(&path, target, origin_y)?;
                    Ok::<_, String>((path, import))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation {
                    return;
                }
                match result {
                    Ok((path, import)) => {
                        this.complete_chunk_transfer_progress();
                        let chunk_count = import.copied_chunk.chunk_count();
                        this.professional.copied_chunk = Some(import.copied_chunk);
                        this.professional.imported_region_package = false;
                        this.professional.imported_structure = Some(import.imported_structure);
                        this.professional.copied_chunk_preview_images = import.preview_images;
                        this.clear_paste_preview_state(cx);
                        this.invalidate_preview_3d_mesh();
                        if this.set_paste_preview(
                            target,
                            PasteTransform::default(),
                            paste_rotation_degrees(PasteRotation::NoRotation),
                            None,
                            cx,
                        ) {
                            this.center_paste_preview_in_view(cx);
                            let message = SharedString::from(format!(
                                "已导入结构 {}x{}x{}，生成 {} 个 chunk 预览，确认后写入",
                                import.size.x, import.size.y, import.size.z, chunk_count
                            ));
                            this.status = message.clone();
                            this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                "mcstructure {}",
                                path.display()
                            )));
                        }
                    }
                    Err(error) => {
                        this.finish_chunk_transfer_progress();
                        this.status = SharedString::from(error.clone());
                        this.resolve_edit_toast(
                            toast::ToastKind::Error,
                            SharedString::from(error),
                            cx,
                        );
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn export_selection_mcstructure(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("没有可导出的选区");
            cx.notify();
            return;
        };
        let export_center_y = self.y_layer;
        let default_file_name =
            mcstructure::default_mcstructure_file_name(selection, export_center_y);
        let Some(path) = pick_save_path_with_filter(
            "Bedrock Structure",
            &[mcstructure::MCSTRUCTURE_EXTENSION],
            &default_file_name,
        ) else {
            self.status = SharedString::from("已取消导出 .mcstructure");
            cx.notify();
            return;
        };
        let path = PathBuf::from(path);
        let bounds = selection.bounds();
        let chunk_count = selection.chunks().len();
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导出结构"),
            completed: 0,
            total: chunk_count.max(1),
        });
        self.begin_edit_toast(
            SharedString::from(format!(
                "正在导出 {chunk_count} 个 chunk 为 .mcstructure，中心 Y {export_center_y}..."
            )),
            cx,
        );
        self.status = SharedString::from("正在导出 Bedrock 结构文件...");
        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        cx.notify();

        cx.spawn(async move |handle, cx| {
            enum McStructureExportEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<PathBuf, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<McStructureExportEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task = cx.background_spawn(async move {
                let result = mcstructure::export_selection_mcstructure_blocking(
                    &world_path,
                    bounds,
                    export_center_y,
                    &path,
                    |progress| {
                        if progress_sender
                            .unbounded_send(McStructureExportEvent::Progress(progress))
                            .is_err()
                        {
                            tracing::debug!("mcstructure export progress receiver dropped");
                        }
                    },
                );
                if completion_sender
                    .unbounded_send(McStructureExportEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("mcstructure export completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, McStructureExportEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != generation {
                        return;
                    }
                    match event {
                        McStructureExportEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        McStructureExportEvent::Complete(result) => match result {
                            Ok(path) => {
                                this.complete_chunk_transfer_progress();
                                let message = SharedString::from(format!(
                                    "已导出 .mcstructure: {}",
                                    path.display()
                                ));
                                this.status = message.clone();
                                this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                this.status = SharedString::from(error.clone());
                                this.resolve_edit_toast(
                                    toast::ToastKind::Error,
                                    SharedString::from(error),
                                    cx,
                                );
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn export_chunks_image(&mut self, cx: &mut Context<Self>) {
        let cached_export = self.chunk_image_export_source();
        let chunks = if cached_export.is_none() {
            self.professional
                .selection
                .map(ChunkSelection::chunks)
                .filter(|chunks| !chunks.is_empty())
                .unwrap_or_else(|| vec![self.active_target_chunk_pos()])
        } else {
            Vec::new()
        };
        let default_file_name = cached_export.as_ref().map_or_else(
            || export_file_name_for_chunks("chunk-image", &chunks),
            |export| export.file_name.clone(),
        );
        let Some(path) = pick_save_path_with_filter("PNG Image", &["png"], &default_file_name)
        else {
            self.status = SharedString::from("已取消导出区块图片");
            cx.notify();
            return;
        };

        self.context_menu = None;
        let chunk_count = cached_export
            .as_ref()
            .map_or_else(|| chunks.len().max(1), |export| export.chunk_count);
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("导出图片"),
            completed: 0,
            total: chunk_count,
        });
        self.begin_edit_toast(
            SharedString::from(format!("正在导出 {chunk_count} 个区块图片...")),
            cx,
        );
        self.status = SharedString::from("正在导出区块图片...");
        let world_path = self.world_path.clone();
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let mode = self.current_render_mode();
        let dimension = self.dimension;
        let layout = self.active_layout;
        let cpu_budget = self.cpu_budget;
        let tile_chunk_index =
            preview_tile_chunk_index_for_chunks(&chunks, layout, &self.tile_chunk_index);
        let canvas_preview_images = self.copied_chunk_preview_images_for_chunks(&chunks);
        cx.notify();

        cx.spawn(async move |handle, cx| {
            enum ExportImageEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<String, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<ExportImageEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task = cx.background_spawn(async move {
                let result = (|| {
                    let export = if let Some(export) = cached_export {
                        export
                    } else {
                        build_chunk_image_export_blocking(
                            world_path,
                            render_backend,
                            render_gpu_backend,
                            mode,
                            dimension,
                            layout,
                            cpu_budget,
                            tile_chunk_index,
                            canvas_preview_images,
                            chunks,
                            |progress| {
                                if progress_sender
                                    .unbounded_send(ExportImageEvent::Progress(progress))
                                    .is_err()
                                {
                                    tracing::debug!("map image export progress receiver dropped");
                                }
                            },
                        )?
                    };
                    encode_chunk_image_export_png(&export)
                        .and_then(|bytes| {
                            std::fs::write(&path, bytes)
                                .map_err(|error| format!("写入区块图片失败：{}", error))
                        })
                        .map(|()| path)
                })();
                if completion_sender
                    .unbounded_send(ExportImageEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("map image export completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, ExportImageEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    match event {
                        ExportImageEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        ExportImageEvent::Complete(result) => match result {
                            Ok(path) => {
                                this.complete_chunk_transfer_progress();
                                let message = SharedString::from(format!("已导出区块图片: {path}"));
                                this.status = message.clone();
                                this.resolve_edit_toast(toast::ToastKind::Success, message, cx);
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                this.status = SharedString::from(error.clone());
                                this.resolve_edit_toast(
                                    toast::ToastKind::Error,
                                    SharedString::from(error),
                                    cx,
                                );
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn chunk_image_export_source(&self) -> Option<ChunkImageExport> {
        if !self.paste_preview_images.is_empty() {
            return chunk_image_export_from_paste_preview(
                "paste-preview",
                self.paste_preview_images.as_ref(),
            );
        }
        if self.professional.copied_chunk_preview_images.is_empty() {
            return None;
        }
        chunk_image_export_from_copied_images(
            "copied-chunks",
            &self.professional.copied_chunk_preview_images,
        )
    }

    pub(super) fn copy_context_chunks(&mut self, cx: &mut Context<Self>) {
        let chunks = self
            .professional
            .selection
            .map(ChunkSelection::chunks)
            .filter(|chunks| !chunks.is_empty())
            .unwrap_or_else(|| vec![self.active_target_chunk_pos()]);
        self.copy_chunks(chunks, cx);
    }

    pub(super) fn copy_current_chunk(&mut self, cx: &mut Context<Self>) {
        self.copy_chunks(vec![self.active_target_chunk_pos()], cx);
    }

    pub(super) fn copy_chunks(&mut self, chunks: Vec<ChunkPos>, cx: &mut Context<Self>) {
        if chunks.is_empty() {
            self.status = SharedString::from("没有可复制的 chunk");
            cx.notify();
            return;
        }
        let source_anchor = chunks[0];
        let chunk_count = chunks.len();
        self.context_menu = None;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from("复制区块"),
            completed: 0,
            total: chunk_count,
        });
        self.begin_edit_toast(
            SharedString::from(format!("正在复制 {chunk_count} 个 chunk...")),
            cx,
        );
        self.status = if chunk_count == 1 {
            SharedString::from(format!(
                "正在复制 chunk {},{}...",
                source_anchor.x, source_anchor.z
            ))
        } else {
            SharedString::from(format!("正在复制 {chunk_count} 个 chunk..."))
        };
        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let mode = self.current_render_mode();
        let dimension = self.dimension;
        let layout = self.active_layout;
        let cpu_budget = self.cpu_budget;
        let render_backend = self.render_backend;
        let render_gpu_backend = self.render_gpu_backend;
        let tile_chunk_index =
            preview_tile_chunk_index_for_chunks(&chunks, layout, &self.tile_chunk_index);
        let canvas_preview_images = self.copied_chunk_preview_images_for_chunks(&chunks);
        cx.notify();

        cx.spawn(async move |handle, cx| {
            enum CopyChunkEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<CopyChunkComplete, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<CopyChunkEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let task = cx.background_spawn(async move {
                let result = (|| {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::OpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let editor = MapWorldEditor::from_world(world);
                    let copied_chunk =
                        copy_chunks_blocking(&editor, source_anchor, chunks, |progress| {
                            let _ =
                                progress_sender.unbounded_send(CopyChunkEvent::Progress(progress));
                        })
                        .map_err(|error| error.to_string())?;
                    drop(editor);
                    let _ = progress_sender.unbounded_send(CopyChunkEvent::Progress(
                        ChunkTransferProgress {
                            phase: SharedString::from("生成粘贴预览"),
                            completed: 0,
                            total: copied_chunk.chunk_count(),
                        },
                    ));
                    let (preview_images, preview_error) =
                        match render_copied_chunk_preview_images_blocking(
                            world_path,
                            render_backend,
                            render_gpu_backend,
                            mode,
                            dimension,
                            layout,
                            cpu_budget,
                            tile_chunk_index,
                            &copied_chunk,
                        ) {
                            Ok(mut preview_images) if !preview_images.is_empty() => {
                                for (chunk, image) in canvas_preview_images {
                                    preview_images.entry(chunk).or_insert(image);
                                }
                                (preview_images, None)
                            }
                            Ok(_) => (canvas_preview_images, None),
                            Err(error) => (canvas_preview_images, Some(error)),
                        };
                    Ok(CopyChunkComplete {
                        copied_chunk,
                        preview_images,
                        preview_error,
                    })
                })();
                if completion_sender
                    .unbounded_send(CopyChunkEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("copy chunk completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, CopyChunkEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != generation {
                        return;
                    }
                    match event {
                        CopyChunkEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        CopyChunkEvent::Complete(result) => match result {
                            Ok(complete) => {
                                this.complete_chunk_transfer_progress();
                                let CopyChunkComplete {
                                    copied_chunk,
                                    preview_images,
                                    preview_error,
                                } = complete;
                                let source = copied_chunk.source;
                                let chunk_count = copied_chunk.chunk_count();
                                this.professional.copied_chunk = Some(copied_chunk);
                                this.professional.imported_region_package = false;
                                this.professional.imported_structure = None;
                                this.professional.copied_chunk_preview_images = preview_images;
                                this.clear_paste_preview_state(cx);
                                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                    "bmcbl-chunks {} {} {} {}",
                                    source.dimension.id(),
                                    source.x,
                                    source.z,
                                    chunk_count
                                )));
                                let mut status = if chunk_count == 1 {
                                    SharedString::from(format!(
                                        "已复制 chunk {},{}，可在其他 chunk 上粘贴",
                                        source.x, source.z
                                    ))
                                } else {
                                    SharedString::from(format!(
                                        "已复制 {chunk_count} 个 chunk，锚点 {},{}",
                                        source.x, source.z
                                    ))
                                };
                                if let Some(error) = preview_error {
                                    let status_text = status.as_ref().to_string();
                                    status = SharedString::from(format!(
                                        "{status_text}；预览图使用当前画布缓存（{error}）"
                                    ));
                                    toast::error(
                                        cx,
                                        SharedString::from(format!(
                                            "粘贴预览图生成失败，已使用画布缓存：{error}"
                                        )),
                                    );
                                }
                                this.status = status.clone();
                                this.resolve_edit_toast(toast::ToastKind::Success, status, cx);
                            }
                            Err(error) => {
                                this.finish_chunk_transfer_progress();
                                this.status = SharedString::from(error.clone());
                                this.resolve_edit_toast(
                                    toast::ToastKind::Error,
                                    SharedString::from(error),
                                    cx,
                                );
                            }
                        },
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn paste_copied_chunk_to_context(
        &mut self,
        rotation: PasteRotation,
        cx: &mut Context<Self>,
    ) {
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            self.status = SharedString::from("没有已复制的 chunk");
            toast::error(cx, self.status.clone());
            cx.notify();
            return;
        };
        let chunk_count = copied_chunk.chunk_count();
        let target = self.active_target_chunk_pos();
        let transform = PasteTransform::from_rotation(rotation);
        if !self.set_paste_preview(
            target,
            transform,
            paste_rotation_degrees(rotation),
            None,
            cx,
        ) {
            return;
        }
        self.status = SharedString::from(format!(
            "粘贴预览：{chunk_count} 个 chunk · {} · 确认后写入",
            transform.label()
        ));
        cx.notify();
    }

    pub(super) fn add_context_marker(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.context_menu {
            self.markers
                .entry(self.dimension)
                .or_default()
                .push(Marker {
                    x: menu.block_x,
                    z: menu.block_z,
                    label: SharedString::from(format!("{}, {}", menu.block_x, menu.block_z)),
                });
            self.status = SharedString::from("已添加地图标记");
        }
        self.context_menu = None;
        cx.notify();
    }

    pub(super) fn clear_dimension_markers(&mut self, cx: &mut Context<Self>) {
        self.markers.remove(&self.dimension);
        self.context_menu = None;
        self.status = SharedString::from("已清除当前维度标记");
        cx.notify();
    }

    pub(super) fn toggle_slime_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.slime_chunks = !self.overlay_options.slime_chunks;
        cx.notify();
    }

    pub(super) fn toggle_entity_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.entities = !self.overlay_options.entities;
        self.professional.overlay_bounds = None;
        self.professional.overlays = None;
        self.professional.overlay_paint = None;
        self.refresh_professional_overlays(cx);
        cx.notify();
    }

    pub(super) fn toggle_block_entity_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.block_entities = !self.overlay_options.block_entities;
        self.professional.overlay_bounds = None;
        self.professional.overlays = None;
        self.professional.overlay_paint = None;
        self.refresh_professional_overlays(cx);
        cx.notify();
    }

    pub(super) fn toggle_village_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.villages = !self.overlay_options.villages;
        self.professional.overlay_bounds = None;
        self.professional.overlays = None;
        self.professional.overlay_paint = None;
        if self.overlay_options.villages {
            self.refresh_village_index_if_needed(cx);
        }
        self.refresh_professional_overlays(cx);
        cx.notify();
    }

    pub(super) fn toggle_hsa_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.hardcoded_spawn_areas = !self.overlay_options.hardcoded_spawn_areas;
        self.professional.overlay_bounds = None;
        self.professional.overlays = None;
        self.professional.overlay_paint = None;
        self.refresh_professional_overlays(cx);
        cx.notify();
    }

    pub(super) fn toggle_write_mode(&mut self, cx: &mut Context<Self>) {
        self.professional.write_mode = !self.professional.write_mode;
        self.professional.pending_delete_confirmation = false;
        self.professional.pending_edit_confirmation = None;
        self.professional.pending_quick_write_confirmation = None;
        self.clear_paste_preview_state(cx);
        self.players.pending_save_confirmation = None;
        self.status = if self.professional.write_mode {
            SharedString::from("写入模式已开启 · 修改存档前请确认已备份")
        } else {
            SharedString::from("写入模式已关闭")
        };
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    pub(super) fn set_slime_query_window_size(
        &mut self,
        size: SlimeQueryWindowSize,
        cx: &mut Context<Self>,
    ) {
        self.slime_query_window_size = size;
        self.professional.highlighted_window = None;
        self.refresh_professional_render_caches();
        cx.notify();
    }

    pub(super) fn set_context_selection_start(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        let chunk = context_menu_chunk(menu, self.dimension);
        self.clear_paste_preview_state(cx);
        self.professional.selection = Some(ChunkSelection {
            start: chunk,
            end: self
                .professional
                .selection
                .map_or(chunk, |selection| selection.end),
        });
        self.professional.pending_delete_confirmation = false;
        self.invalidate_preview_3d_mesh();
        self.refresh_professional_render_caches();
        self.context_menu = None;
        self.status = SharedString::from(format!("选区起点已设为 chunk {}, {}", chunk.x, chunk.z));
        cx.notify();
    }

    pub(super) fn set_context_selection_end(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        let chunk = context_menu_chunk(menu, self.dimension);
        self.clear_paste_preview_state(cx);
        self.professional.selection = Some(ChunkSelection {
            start: self
                .professional
                .selection
                .map_or(chunk, |selection| selection.start),
            end: chunk,
        });
        self.professional.pending_delete_confirmation = false;
        self.invalidate_preview_3d_mesh();
        self.refresh_professional_render_caches();
        self.context_menu = None;
        self.status = SharedString::from(format!("选区终点已设为 chunk {}, {}", chunk.x, chunk.z));
        cx.notify();
    }

    pub(super) fn clear_professional_selection(&mut self, cx: &mut Context<Self>) {
        self.professional.selection = None;
        self.professional.highlighted_window = None;
        self.professional.selection_stats = None;
        self.professional.pending_delete_confirmation = false;
        self.professional.pending_edit_confirmation = None;
        self.professional.pending_quick_write_confirmation = None;
        self.clear_paste_preview_state(cx);
        self.clear_preview_3d_resources(true);
        self.refresh_professional_render_caches();
        self.status = SharedString::from("已清除专业查询选区");
        cx.notify();
    }

    pub(super) fn open_context_hsa_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chunk) = self.context_chunk_pos() else {
            self.status = SharedString::from("无法确定当前 chunk");
            cx.notify();
            return;
        };
        self.load_edit_detail(EditTarget::HsaChunk(chunk), cx);
    }

    pub(super) fn open_context_block_entities_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chunk) = self.context_chunk_pos() else {
            self.status = SharedString::from("无法确定当前 chunk");
            cx.notify();
            return;
        };
        self.load_edit_detail(EditTarget::BlockEntities(chunk), cx);
    }

    pub(super) fn open_context_block_entity_at_editor(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        let chunk = BlockPos {
            x: menu.block_x,
            y: self.y_layer,
            z: menu.block_z,
        }
        .to_chunk_pos(self.dimension);
        let block = BlockPos {
            x: menu.block_x,
            y: self.y_layer,
            z: menu.block_z,
        };
        self.load_edit_detail(EditTarget::BlockEntityAt { chunk, block }, cx);
    }

    pub(super) fn open_context_actors_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chunk) = self.context_chunk_pos() else {
            self.status = SharedString::from("无法确定当前 chunk");
            cx.notify();
            return;
        };
        self.load_edit_detail(EditTarget::Actors(chunk), cx);
    }

    pub(super) fn open_context_heightmap_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chunk) = self.context_chunk_pos() else {
            self.status = SharedString::from("无法确定当前 chunk");
            cx.notify();
            return;
        };
        self.load_edit_detail(EditTarget::HeightMap(chunk), cx);
    }

    pub(super) fn open_context_biome_storage_editor(&mut self, cx: &mut Context<Self>) {
        let Some(chunk) = self.context_chunk_pos() else {
            self.status = SharedString::from("无法确定当前 chunk");
            cx.notify();
            return;
        };
        self.load_edit_detail(EditTarget::BiomeStorage(chunk), cx);
    }

    pub(super) fn context_chunk_pos(&self) -> Option<ChunkPos> {
        self.context_menu.map(|menu| {
            BlockPos {
                x: menu.block_x,
                y: self.y_layer,
                z: menu.block_z,
            }
            .to_chunk_pos(self.dimension)
        })
    }

    pub(super) fn set_professional_detail(
        &mut self,
        detail: Option<ProfessionalDetail>,
        cx: &mut Context<Self>,
    ) {
        self.professional.detail = detail;
        self.editor_document.loading = false;
        self.editor_document.saving = false;
        self.editor_document.dirty = false;
        if let Some(detail) = self.professional.detail.as_ref() {
            self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
            self.ui_state.set_right_panel_open(true);
            self.editor_document.target = detail.edit_target();
            self.editor_document.title = detail.title();
            self.editor_document.text = detail.json();
        } else {
            self.editor_document = EditorDocument::default();
        }
        let text = self.editor_document.text.clone();
        self.editor_state.update(cx, |editor, cx| {
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor.set_value(text, cx);
        });
    }
}

pub(super) fn canvas_pointer_move_action(
    pressed_button: Option<MouseButton>,
    map_drag_active: bool,
    right_selection_active: bool,
    preview_3d_drag_active: bool,
    dock_drag_active: bool,
) -> CanvasPointerMoveAction {
    match pressed_button {
        Some(MouseButton::Left) if map_drag_active || dock_drag_active => {
            CanvasPointerMoveAction::UpdateMapPointer
        }
        Some(MouseButton::Right) | None if right_selection_active => {
            CanvasPointerMoveAction::UpdateRightSelection
        }
        Some(MouseButton::Left) if preview_3d_drag_active => CanvasPointerMoveAction::Ignore,
        _ if map_drag_active
            || right_selection_active
            || preview_3d_drag_active
            || dock_drag_active =>
        {
            CanvasPointerMoveAction::ReleaseStaleCaptures
        }
        _ => CanvasPointerMoveAction::UpdateMapPointer,
    }
}

fn preview_3d_resource_package_paths(world_path: &Path, cx: &App) -> Vec<PathBuf> {
    let mut package_paths = Vec::new();
    let launcher_path = cx.read_global(|state: &LauncherState, _cx| {
        let package_path = state.package_path.to_string();
        (!package_path.trim().is_empty()).then(|| PathBuf::from(package_path))
    });
    if let Some(package_path) = launcher_path {
        bedrock_block_model::push_unique_resource_pack_path(&mut package_paths, package_path);
    }

    cx.read_global(|state: &LocalVersionsState, _cx| {
        for version in state.versions.iter() {
            bedrock_block_model::push_unique_resource_pack_path(
                &mut package_paths,
                PathBuf::from(version.path.as_ref()),
            );
        }
    });

    for package_path in
        crate::core::minecraft::paths::infer_package_roots_from_world_path(world_path)
    {
        bedrock_block_model::push_unique_resource_pack_path(&mut package_paths, package_path);
    }

    package_paths
}

pub(super) fn take_pointer_captures(
    drag: &mut Option<DragState>,
    right_selection_drag: &mut Option<RightSelectionDrag>,
    preview_3d_drag_origin: &mut Option<Point<Pixels>>,
    dock_drag: &mut Option<DockDragState>,
) -> PointerCaptureRelease {
    PointerCaptureRelease {
        map_drag: drag.take().is_some(),
        right_selection: right_selection_drag.take().is_some(),
        preview_3d_drag: preview_3d_drag_origin.take().is_some(),
        dock_drag: dock_drag.take().is_some(),
    }
}

pub(super) fn log_pointer_capture_release(source: &'static str, release: PointerCaptureRelease) {
    tracing::debug!(
        target: "bmcbl::ui::window::map_viewer::view",
        source,
        map_drag = release.map_drag,
        right_selection = release.right_selection,
        preview_3d_drag = release.preview_3d_drag,
        dock_drag = release.dock_drag,
        "map_viewer pointer_capture_released"
    );
}

pub(super) fn replace_paste_preview_image_set(
    current: &mut Arc<Vec<PastePreviewImage>>,
    images: Vec<PastePreviewImage>,
) {
    *current = Arc::new(images);
}

fn paste_preview_image_for_chunk(
    source: &CopiedChunkPreviewImage,
    target: ChunkPos,
    transform: PasteTransform,
) -> Option<PastePreviewImage> {
    let expected_len = usize::try_from(source.width)
        .ok()
        .and_then(|width| {
            usize::try_from(source.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixel_count| pixel_count.checked_mul(4))?;
    if source.pixels.len() < expected_len {
        return None;
    }

    let (output_width, output_height) = match transform.rotation {
        PasteRotation::NoRotation | PasteRotation::Rotate180 => (source.width, source.height),
        PasteRotation::Clockwise90 | PasteRotation::CounterClockwise90 => {
            (source.height, source.width)
        }
    };
    let output_len = usize::try_from(output_width)
        .ok()
        .and_then(|width| {
            usize::try_from(output_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixel_count| pixel_count.checked_mul(4))?;
    let mut output = vec![0_u8; output_len];
    let output_width_usize = usize::try_from(output_width).ok()?;
    let source_width_usize = usize::try_from(source.width).ok()?;
    for source_y in 0..source.height {
        for source_x in 0..source.width {
            let (target_x, target_y) = transformed_preview_pixel(
                source_x,
                source_y,
                source.width,
                source.height,
                transform,
            );
            let source_index = usize::try_from(source_y)
                .ok()?
                .checked_mul(source_width_usize)?
                .checked_add(usize::try_from(source_x).ok()?)?
                .checked_mul(4)?;
            let target_index = usize::try_from(target_y)
                .ok()?
                .checked_mul(output_width_usize)?
                .checked_add(usize::try_from(target_x).ok()?)?
                .checked_mul(4)?;
            output[target_index] = source.pixels[source_index];
            output[target_index + 1] = source.pixels[source_index + 1];
            output[target_index + 2] = source.pixels[source_index + 2];
            output[target_index + 3] =
                ((u16::from(source.pixels[source_index + 3]) * 184) / 255) as u8;
        }
    }
    let image = import_preview::copied_preview_image_to_render_image(&CopiedChunkPreviewImage {
        chunk: target,
        pixels: Arc::<[u8]>::from(output.clone()),
        width: output_width,
        height: output_height,
    })?;
    Some(PastePreviewImage {
        target,
        image,
        pixels: Arc::<[u8]>::from(output),
        width: output_width,
        height: output_height,
    })
}

fn copied_chunk_chunk_bounds(copied_chunk: &CopiedChunkData) -> Option<(i32, i32, i32, i32)> {
    let mut chunks = copied_chunk.chunks.iter().map(|chunk| chunk.chunk);
    let first = chunks.next()?;
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (first.x, first.x, first.z, first.z);
    for chunk in chunks {
        min_x = min_x.min(chunk.x);
        max_x = max_x.max(chunk.x);
        min_z = min_z.min(chunk.z);
        max_z = max_z.max(chunk.z);
    }
    Some((min_x, max_x, min_z, max_z))
}

fn fallback_copied_chunk_preview_image(
    chunk: ChunkPos,
    source_anchor: ChunkPos,
    bounds: (i32, i32, i32, i32),
) -> CopiedChunkPreviewImage {
    let (min_x, max_x, min_z, max_z) = bounds;
    let mut pixels = vec![0_u8; 16 * 16 * 4];
    let is_origin = chunk == source_anchor;
    let is_positive_x_edge = chunk.x == max_x;
    let is_positive_z_edge = chunk.z == max_z;
    for z in 0..16 {
        for x in 0..16 {
            let is_border = x == 0 || x == 15 || z == 0 || z == 15;
            let color = if is_origin && x <= 5 && z <= 5 {
                [34, 197, 94, 224]
            } else if is_positive_x_edge && x >= 11 {
                [239, 68, 68, 210]
            } else if is_positive_z_edge && z >= 11 {
                [59, 130, 246, 210]
            } else if is_border {
                [245, 158, 11, 160]
            } else {
                let width = max_x.saturating_sub(min_x).max(1);
                let depth = max_z.saturating_sub(min_z).max(1);
                let x_ratio = (chunk.x.saturating_sub(min_x) as f32 / width as f32).clamp(0.0, 1.0);
                let z_ratio = (chunk.z.saturating_sub(min_z) as f32 / depth as f32).clamp(0.0, 1.0);
                [
                    (220.0 + 24.0 * x_ratio) as u8,
                    (150.0 + 36.0 * (1.0 - z_ratio)) as u8,
                    (36.0 + 84.0 * z_ratio) as u8,
                    118,
                ]
            };
            let index = (z * 16 + x) * 4;
            pixels[index] = color[0];
            pixels[index + 1] = color[1];
            pixels[index + 2] = color[2];
            pixels[index + 3] = color[3];
        }
    }
    CopiedChunkPreviewImage {
        chunk,
        pixels: Arc::<[u8]>::from(pixels),
        width: 16,
        height: 16,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_copied_chunk_preview_images_blocking(
    world_path: PathBuf,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    cpu_budget: RenderCpuBudget,
    tile_chunk_index: BTreeMap<(i32, i32), Vec<ChunkPos>>,
    copied_chunk: &CopiedChunkData,
) -> Result<BTreeMap<ChunkPos, CopiedChunkPreviewImage>, String> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .map_err(|_| "复制预览瓦片布局无效".to_string())?
        .max(1);
    let mut chunks_by_tile: BTreeMap<(i32, i32), Vec<ChunkPos>> = BTreeMap::new();
    for chunk in &copied_chunk.chunks {
        let coord = (
            chunk.chunk.x.div_euclid(chunks_per_tile),
            chunk.chunk.z.div_euclid(chunks_per_tile),
        );
        chunks_by_tile.entry(coord).or_default().push(chunk.chunk);
    }
    if chunks_by_tile.is_empty() {
        return Ok(BTreeMap::new());
    }

    let render_session = Arc::new(open_map_render_session(
        world_path.clone(),
        render_backend,
        render_gpu_backend,
    )?);
    let mut plans = Vec::with_capacity(chunks_by_tile.len());
    for (coord, chunks) in &chunks_by_tile {
        let indexed_positions = tile_chunk_index
            .get(coord)
            .cloned()
            .unwrap_or_else(|| chunks.clone());
        plans.push(RenderTilePlan::new(
            dimension,
            mode,
            layout,
            *coord,
            indexed_positions,
        )?);
    }

    let center_tile = (
        copied_chunk.source.x.div_euclid(chunks_per_tile),
        copied_chunk.source.z.div_euclid(chunks_per_tile),
    );
    let render_cancel = RenderCancelFlag::new();
    let (event_sender, mut event_receiver) = unbounded::<TileRenderEvent>();
    let cache_identity = decoded_cache_identity(&world_path, render_backend, render_gpu_backend);
    let tile_cache_validation_seed = cache_identity.validation_seed;
    render_tile_batch_stream(
        TileBatchRequest {
            render_session,
            world_path: world_path.clone(),
            mode,
            dimension,
            layout,
            center_tile,
            cache_policy: RenderCachePolicy::Use,
            plans,
            cpu_budget,
            render_backend,
            render_gpu_backend,
            cache_identity,
            tile_cache_validation_seed,
            quick_reveal: true,
            render_cancel,
        },
        event_sender,
    )?;

    let mut preview_images = BTreeMap::new();
    while let Ok(Some(event)) = event_receiver.try_next() {
        if let TileRenderEvent::ReadyBatch { tiles } = event {
            for ReadyTile { coord, tile, .. } in tiles {
                let Some(chunks) = chunks_by_tile.get(&coord) else {
                    continue;
                };
                let Some(pixels) = tile.pixels.as_ref() else {
                    continue;
                };
                let Some(pixel_format) = tile.pixel_format else {
                    continue;
                };
                for chunk in chunks {
                    if let Some(image) = copy_chunk_preview_image_from_tile(
                        *chunk,
                        coord,
                        chunks_per_tile,
                        pixels,
                        pixel_format,
                        tile.width,
                        tile.height,
                    ) {
                        preview_images.insert(image.chunk, image);
                    }
                }
            }
        }
    }

    Ok(preview_images)
}

fn preview_tile_chunk_index_for_chunks(
    chunks: &[ChunkPos],
    layout: RenderLayout,
    tile_chunk_index: &BTreeMap<(i32, i32), Vec<ChunkPos>>,
) -> BTreeMap<(i32, i32), Vec<ChunkPos>> {
    let Ok(chunks_per_tile) = i32::try_from(layout.chunks_per_tile) else {
        return BTreeMap::new();
    };
    let chunks_per_tile = chunks_per_tile.max(1);
    chunks
        .iter()
        .filter_map(|chunk| {
            let coord = (
                chunk.x.div_euclid(chunks_per_tile),
                chunk.z.div_euclid(chunks_per_tile),
            );
            tile_chunk_index
                .get(&coord)
                .cloned()
                .map(|indexed| (coord, indexed))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn copy_chunk_preview_image_from_tile(
    source: ChunkPos,
    tile_coord: (i32, i32),
    chunks_per_tile: i32,
    pixels: &[u8],
    pixel_format: TilePixelFormat,
    tile_width: u32,
    tile_height: u32,
) -> Option<CopiedChunkPreviewImage> {
    if chunks_per_tile <= 0 {
        return None;
    }
    let expected_tile_x = source.x.div_euclid(chunks_per_tile);
    let expected_tile_z = source.z.div_euclid(chunks_per_tile);
    if tile_coord != (expected_tile_x, expected_tile_z) {
        return None;
    }

    let chunks_per_tile_u32 = u32::try_from(chunks_per_tile).ok()?;
    let chunk_width = tile_width.checked_div(chunks_per_tile_u32)?.max(1);
    let chunk_height = tile_height.checked_div(chunks_per_tile_u32)?.max(1);
    let local_chunk_x = u32::try_from(source.x.rem_euclid(chunks_per_tile)).ok()?;
    let local_chunk_z = u32::try_from(source.z.rem_euclid(chunks_per_tile)).ok()?;
    let source_left = local_chunk_x.checked_mul(chunk_width)?;
    let source_top = local_chunk_z.checked_mul(chunk_height)?;
    if source_left.checked_add(chunk_width)? > tile_width
        || source_top.checked_add(chunk_height)? > tile_height
    {
        return None;
    }
    let tile_width_usize = usize::try_from(tile_width).ok()?;
    let expected_len = usize::try_from(tile_width)
        .ok()
        .and_then(|width| {
            usize::try_from(tile_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixel_count| pixel_count.checked_mul(4))?;
    if pixels.len() < expected_len {
        return None;
    }
    let chunk_len = usize::try_from(chunk_width)
        .ok()
        .and_then(|width| {
            usize::try_from(chunk_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixel_count| pixel_count.checked_mul(4))?;
    let mut output = vec![0_u8; chunk_len];
    let output_width = usize::try_from(chunk_width).ok()?;
    for y in 0..chunk_height {
        for x in 0..chunk_width {
            let source_index = usize::try_from(source_top.checked_add(y)?)
                .ok()?
                .checked_mul(tile_width_usize)?
                .checked_add(usize::try_from(source_left.checked_add(x)?).ok()?)?
                .checked_mul(4)?;
            let target_index = usize::try_from(y)
                .ok()?
                .checked_mul(output_width)?
                .checked_add(usize::try_from(x).ok()?)?
                .checked_mul(4)?;
            let rgba = preview_source_pixel_to_rgba(pixel_format, &pixels[source_index..])?;
            output[target_index..target_index + 4].copy_from_slice(&rgba);
        }
    }
    Some(CopiedChunkPreviewImage {
        chunk: source,
        pixels: Arc::<[u8]>::from(output),
        width: chunk_width,
        height: chunk_height,
    })
}

fn transformed_preview_pixel(
    source_x: u32,
    source_y: u32,
    width: u32,
    height: u32,
    transform: PasteTransform,
) -> (u32, u32) {
    let source_x = if transform.mirror_x {
        width.saturating_sub(1).saturating_sub(source_x)
    } else {
        source_x
    };
    let source_y = if transform.mirror_z {
        height.saturating_sub(1).saturating_sub(source_y)
    } else {
        source_y
    };
    match transform.rotation {
        PasteRotation::NoRotation => (source_x, source_y),
        PasteRotation::Clockwise90 => (height.saturating_sub(1).saturating_sub(source_y), source_x),
        PasteRotation::Rotate180 => (
            width.saturating_sub(1).saturating_sub(source_x),
            height.saturating_sub(1).saturating_sub(source_y),
        ),
        PasteRotation::CounterClockwise90 => {
            (source_y, width.saturating_sub(1).saturating_sub(source_x))
        }
    }
}

fn preview_source_pixel_to_rgba(pixel_format: TilePixelFormat, pixel: &[u8]) -> Option<[u8; 4]> {
    let red = *pixel.first()?;
    let green = *pixel.get(1)?;
    let blue = *pixel.get(2)?;
    let alpha = *pixel.get(3)?;
    Some(match pixel_format {
        TilePixelFormat::Rgba8 => [red, green, blue, alpha],
        TilePixelFormat::Bgra8 => [blue, green, red, alpha],
    })
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_image_export_blocking(
    world_path: PathBuf,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    cpu_budget: RenderCpuBudget,
    tile_chunk_index: BTreeMap<(i32, i32), Vec<ChunkPos>>,
    canvas_preview_images: BTreeMap<ChunkPos, CopiedChunkPreviewImage>,
    chunks: Vec<ChunkPos>,
    mut progress: impl FnMut(ChunkTransferProgress),
) -> Result<ChunkImageExport, String> {
    if chunks.is_empty() {
        return Err("没有可导出的区块图片".to_string());
    }
    let source_anchor = chunks[0];
    let chunk_count = chunks.len();
    progress(ChunkTransferProgress {
        phase: SharedString::from("读取区块"),
        completed: 0,
        total: chunk_count,
    });
    let world = BedrockWorld::open_blocking(&world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
    let editor = MapWorldEditor::from_world(world);
    let copied_chunk = copy_chunks_blocking(&editor, source_anchor, chunks, |copy_progress| {
        progress(copy_progress);
    })
    .map_err(|error| error.to_string())?;
    drop(editor);
    progress(ChunkTransferProgress {
        phase: SharedString::from("生成图片"),
        completed: 0,
        total: chunk_count,
    });
    let mut preview_images = render_copied_chunk_preview_images_blocking(
        world_path,
        render_backend,
        render_gpu_backend,
        mode,
        dimension,
        layout,
        cpu_budget,
        tile_chunk_index,
        &copied_chunk,
    )?;
    for (chunk, image) in canvas_preview_images {
        preview_images.entry(chunk).or_insert(image);
    }
    progress(ChunkTransferProgress {
        phase: SharedString::from("编码图片"),
        completed: chunk_count,
        total: chunk_count,
    });
    chunk_image_export_from_copied_images("chunk-image", &preview_images)
        .ok_or_else(|| "区块图片生成失败".to_string())
}

pub(super) struct ChunkImageExport {
    file_name: String,
    chunks: Vec<ChunkImageExportChunk>,
    chunk_count: usize,
}

pub(super) struct ChunkImageExportChunk {
    pub(super) chunk: ChunkPos,
    pub(super) pixels: Arc<[u8]>,
    pub(super) width: u32,
    pub(super) height: u32,
}

fn chunk_image_export_from_paste_preview(
    prefix: &str,
    images: &[PastePreviewImage],
) -> Option<ChunkImageExport> {
    let chunks = images
        .iter()
        .map(|image| ChunkImageExportChunk {
            chunk: image.target,
            pixels: image.pixels.clone(),
            width: image.width,
            height: image.height,
        })
        .collect::<Vec<_>>();
    chunk_image_export_from_chunks(prefix, chunks)
}

fn chunk_image_export_from_copied_images(
    prefix: &str,
    images: &BTreeMap<ChunkPos, CopiedChunkPreviewImage>,
) -> Option<ChunkImageExport> {
    let chunks = images
        .values()
        .map(|image| ChunkImageExportChunk {
            chunk: image.chunk,
            pixels: image.pixels.clone(),
            width: image.width,
            height: image.height,
        })
        .collect::<Vec<_>>();
    chunk_image_export_from_chunks(prefix, chunks)
}

pub(super) fn chunk_image_export_from_chunks(
    prefix: &str,
    chunks: Vec<ChunkImageExportChunk>,
) -> Option<ChunkImageExport> {
    if chunks.is_empty() {
        return None;
    }
    let min_x = chunks.iter().map(|chunk| chunk.chunk.x).min()?;
    let min_z = chunks.iter().map(|chunk| chunk.chunk.z).min()?;
    let max_x = chunks.iter().map(|chunk| chunk.chunk.x).max()?;
    let max_z = chunks.iter().map(|chunk| chunk.chunk.z).max()?;
    let file_name = format!("{prefix}-{min_x}-{min_z}-{max_x}-{max_z}.png");
    let chunk_count = chunks.len();
    Some(ChunkImageExport {
        file_name,
        chunks,
        chunk_count,
    })
}

fn export_file_name_for_chunks(prefix: &str, chunks: &[ChunkPos]) -> String {
    let Some(min_x) = chunks.iter().map(|chunk| chunk.x).min() else {
        return format!("{prefix}.png");
    };
    let min_z = chunks.iter().map(|chunk| chunk.z).min().unwrap_or(0);
    let max_x = chunks.iter().map(|chunk| chunk.x).max().unwrap_or(min_x);
    let max_z = chunks.iter().map(|chunk| chunk.z).max().unwrap_or(min_z);
    format!("{prefix}-{min_x}-{min_z}-{max_x}-{max_z}.png")
}

pub(super) fn encode_chunk_image_export_png(export: &ChunkImageExport) -> Result<Vec<u8>, String> {
    let min_x = export
        .chunks
        .iter()
        .map(|chunk| chunk.chunk.x)
        .min()
        .ok_or_else(|| "没有可导出的区块图片".to_string())?;
    let min_z = export
        .chunks
        .iter()
        .map(|chunk| chunk.chunk.z)
        .min()
        .ok_or_else(|| "没有可导出的区块图片".to_string())?;
    let max_x = export
        .chunks
        .iter()
        .map(|chunk| chunk.chunk.x)
        .max()
        .ok_or_else(|| "没有可导出的区块图片".to_string())?;
    let max_z = export
        .chunks
        .iter()
        .map(|chunk| chunk.chunk.z)
        .max()
        .ok_or_else(|| "没有可导出的区块图片".to_string())?;
    let chunk_width = export
        .chunks
        .iter()
        .map(|chunk| chunk.width)
        .max()
        .ok_or_else(|| "区块图片宽度无效".to_string())?;
    let chunk_height = export
        .chunks
        .iter()
        .map(|chunk| chunk.height)
        .max()
        .ok_or_else(|| "区块图片高度无效".to_string())?;
    if chunk_width == 0 || chunk_height == 0 {
        return Err("区块图片尺寸无效".to_string());
    }
    let grid_width = u32::try_from(max_x.saturating_sub(min_x).saturating_add(1))
        .map_err(|_| "区块图片范围过大".to_string())?;
    let grid_height = u32::try_from(max_z.saturating_sub(min_z).saturating_add(1))
        .map_err(|_| "区块图片范围过大".to_string())?;
    let width = grid_width
        .checked_mul(chunk_width)
        .ok_or_else(|| "区块图片宽度过大".to_string())?;
    let height = grid_height
        .checked_mul(chunk_height)
        .ok_or_else(|| "区块图片高度过大".to_string())?;
    let mut output = gpui::image::RgbaImage::new(width, height);
    for chunk in &export.chunks {
        blit_export_chunk(&mut output, chunk, min_x, min_z, chunk_width, chunk_height)?;
    }
    let mut encoded = Cursor::new(Vec::new());
    output
        .write_to(&mut encoded, gpui::image::ImageFormat::Png)
        .map_err(|error| format!("编码区块图片失败：{}", error))?;
    Ok(encoded.into_inner())
}

fn blit_export_chunk(
    output: &mut gpui::image::RgbaImage,
    chunk: &ChunkImageExportChunk,
    min_x: i32,
    min_z: i32,
    chunk_width: u32,
    chunk_height: u32,
) -> Result<(), String> {
    let expected_len = usize::try_from(chunk.width)
        .ok()
        .and_then(|width| {
            usize::try_from(chunk.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixel_count| pixel_count.checked_mul(4))
        .ok_or_else(|| "区块图片像素尺寸过大".to_string())?;
    if chunk.pixels.len() < expected_len {
        return Err("区块图片像素数据不完整".to_string());
    }
    let origin_x = u32::try_from(chunk.chunk.x.saturating_sub(min_x))
        .map_err(|_| "区块图片 X 偏移无效".to_string())?
        .checked_mul(chunk_width)
        .ok_or_else(|| "区块图片 X 偏移过大".to_string())?;
    let origin_y = u32::try_from(chunk.chunk.z.saturating_sub(min_z))
        .map_err(|_| "区块图片 Z 偏移无效".to_string())?
        .checked_mul(chunk_height)
        .ok_or_else(|| "区块图片 Z 偏移过大".to_string())?;
    let source_width = usize::try_from(chunk.width).map_err(|_| "区块图片宽度无效".to_string())?;
    for y in 0..chunk.height {
        for x in 0..chunk.width {
            let source_index = usize::try_from(y)
                .ok()
                .and_then(|row| row.checked_mul(source_width))
                .and_then(|row| row.checked_add(usize::try_from(x).ok()?))
                .and_then(|index| index.checked_mul(4))
                .ok_or_else(|| "区块图片像素索引过大".to_string())?;
            let pixel = gpui::image::Rgba([
                chunk.pixels[source_index],
                chunk.pixels[source_index + 1],
                chunk.pixels[source_index + 2],
                chunk.pixels[source_index + 3],
            ]);
            output.put_pixel(origin_x + x, origin_y + y, pixel);
        }
    }
    Ok(())
}

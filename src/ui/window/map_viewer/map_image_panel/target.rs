use super::*;

impl View {
    pub(in super::super) fn begin_map_frame_drag(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.map_image_positioning_frames()
            || self.map_image_operation_busy()
            || self.map_write_busy()
            || self.map_image.placement_preview.is_none()
            || self.map_image.pending_bundle.is_some()
            || (!self.map_image.picking_frame_center && self.map_frame_confirmed_center().is_none())
        {
            return false;
        }
        let pointer = self
            .viewport
            .screen_to_block(self.stage_local_position(position), self.active_layout);
        if !self.map_image.picking_frame_center && self.map_frame_confirmed_center().is_some() {
            let Some(preview) = self.map_frame_grid_preview() else {
                return false;
            };
            if !frame_grid_contains(&preview, pointer) {
                return false;
            }
        }
        self.begin_exclusive_pointer_interaction();
        self.map_image.frame_drag_position = Some(position);
        self.move_map_frame_drag(position, cx);
        cx.notify();
        true
    }

    pub(in super::super) fn move_map_frame_drag(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.map_image.frame_drag_position.is_none() {
            return;
        }
        self.map_image.frame_drag_position = Some(position);
        let local = self.stage_local_position(position);
        self.map_image.frame_auto_pan = self.paste_preview_auto_pan(local);
        self.update_hover_block(local);
        let anchor = self.viewport.screen_to_block(local, self.active_layout);
        if self.map_image.picking_frame_center || self.map_frame_confirmed_center() != Some(anchor)
        {
            self.set_map_frame_anchor(anchor, cx);
        }
    }

    /// Route the world context menu to destination selection while this importer owns it.
    pub(in super::super) fn map_image_selecting_block(&self) -> bool {
        self.import_active()
            && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image.install_destination == MapImageInstallDestination::SelectedBlock
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Placement
    }

    /// World-context interaction is active only for the frame-grid placement workflow.
    pub(in super::super) fn map_image_positioning_frames(&self) -> bool {
        self.import_active()
            && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image.install_destination == MapImageInstallDestination::NewUpFrame
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Placement
    }

    /// Select an exact world coordinate with the context menu. No world writes occur.
    pub(in super::super) fn select_map_frame_center(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.context_menu else {
            return;
        };
        self.context_menu = None;
        self.set_map_frame_anchor((menu.block_x, menu.block_z), cx);
    }

    /// Reuse the map viewer's existing viewport navigation and tile scheduling.
    pub(in super::super) fn focus_selected_map_image_block(&mut self, cx: &mut Context<Self>) {
        if let Some((chunk, block)) = self.map_image.selected_block {
            if chunk.dimension == self.dimension {
                self.center_viewport_on_block(block.x, block.z, cx);
                cx.notify();
            } else {
                self.status = SharedString::from("当前选中的容器属于其他维度，请重新选择目标");
                cx.notify();
            }
        }
    }

    pub(in super::super) fn clear_selected_map_image_block(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.selected_block = None;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    /// Remember a destination without changing the NBT document or writing to the world.
    /// The install operation validates the current container and available slots.
    pub(in super::super) fn select_map_image_block(
        &mut self,
        chunk: ChunkPos,
        block: BlockPos,
        cx: &mut Context<Self>,
    ) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.selected_block = Some((chunk, block));
        self.context_menu = None;
        self.status = SharedString::from(format!(
            "地图写入目标：{}, {}, {}；确认写入时检查容器与空槽位",
            block.x, block.y, block.z,
        ));
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }
}

fn frame_grid_contains(
    preview: &super::super::model::MapFrameGridPreview,
    pointer: (i32, i32),
) -> bool {
    pointer.0 >= preview.min_block_x
        && pointer.1 >= preview.min_block_z
        && pointer.0 < preview.min_block_x.saturating_add(preview.columns as i32)
        && pointer.1 < preview.min_block_z.saturating_add(preview.rows as i32)
}

#[cfg(test)]
mod drag_tests {
    use super::frame_grid_contains;

    #[test]
    fn drag_hit_area_covers_exactly_the_frame_blocks() {
        let preview = super::super::super::model::MapFrameGridPreview {
            controls: None,
            min_block_x: -12,
            min_block_z: 18,
            columns: 5,
            rows: 4,
            image: None,
        };
        assert!(frame_grid_contains(&preview, (-12, 18)));
        assert!(frame_grid_contains(&preview, (-8, 21)));
        for pointer in [(-13, 18), (-7, 18), (-12, 17), (-12, 22)] {
            assert!(!frame_grid_contains(&preview, pointer));
        }
    }
}

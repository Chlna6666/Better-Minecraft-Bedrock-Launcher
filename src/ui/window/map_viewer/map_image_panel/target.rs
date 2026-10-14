use super::*;

impl MapViewerWindowView {
    /// Route the world context menu to destination selection while this importer owns it.
    pub(in super::super) fn map_image_selecting_block(&self) -> bool {
        self.import_workspace_active()
            && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image.install_destination == MapImageInstallDestination::SelectedBlock
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Placement
    }

    /// World-context interaction is active only for the frame-grid placement workflow.
    pub(in super::super) fn map_image_positioning_frames(&self) -> bool {
        self.import_workspace_active()
            && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image.install_destination == MapImageInstallDestination::NewUpFrame
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Placement
    }

    /// Select an exact world coordinate with the context menu. No world writes occur.
    pub(in super::super) fn select_map_frame_center_from_context(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.context_menu else {
            return;
        };
        self.context_menu = None;
        self.set_map_frame_anchor((menu.block_x, menu.block_z), cx);
    }

    /// Reuse the map viewer's existing viewport navigation and tile scheduling.
    pub(in super::super) fn focus_selected_map_image_block(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if let Some((chunk, block)) = self.map_image.selected_block {
            if chunk.dimension == self.dimension {
                self.center_viewport_on_block(block.x, block.z, cx);
                cx.notify();
            } else {
                self.status = SharedString::from(
                    "当前选中的容器属于其他维度，请重新选择目标",
                );
                cx.notify();
            }
        }
    }

    pub(in super::super) fn clear_selected_map_image_block(
        &mut self,
        cx: &mut Context<Self>,
    ) {
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

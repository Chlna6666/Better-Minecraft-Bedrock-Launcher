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
        cx.notify();
    }
}

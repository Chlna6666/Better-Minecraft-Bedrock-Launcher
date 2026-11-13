use super::super::canvas::MapCanvasAction;
use super::super::model::{PasteRotation, PasteTransform};
use super::*;

impl View {
    pub(in super::super) fn clear_map_frame_placement(&mut self, cx: &mut Context<Self>) {
        self.map_image.frame_anchor = None;
        self.map_image.frame_anchor_dimension = None;
        self.map_image.picking_frame_center = false;
        self.map_image.frame_drag_position = None;
        self.map_image.frame_auto_pan = None;
        self.map_image.frame_check_key = None;
        if let Some(task_id) = self.map_image.frame_check_task_id.take() {
            task_manager::cancel_task(&task_id);
        }
        self.map_image.tools_expanded = false;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    pub(in super::super) fn map_frame_transform(&self) -> PasteTransform {
        let options = self.map_image.options.transform;
        PasteTransform {
            rotation: PasteRotation::ALL[usize::from(options.quarter_turns % 4)],
            mirror_x: options.mirror_x,
            mirror_z: options.mirror_z,
        }
    }

    pub(in super::super) fn handle_map_frame_action(
        &mut self,
        action: &MapCanvasAction,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.map_image_positioning_frames() {
            return false;
        }
        match action {
            MapCanvasAction::BeginPastePreviewMove(position) => {
                if !self.map_image_operation_busy() && !self.map_image_busy() {
                    self.begin_exclusive_pointer_interaction();
                    self.map_image.frame_drag_position = Some(*position);
                    cx.notify();
                }
            }
            MapCanvasAction::RotatePastePreviewClockwise => {
                self.transform_map_frames(self.map_frame_transform().rotate_clockwise(), cx);
            }
            MapCanvasAction::RotatePastePreviewCounterClockwise => {
                self.transform_map_frames(
                    self.map_frame_transform().rotate_counter_clockwise(),
                    cx,
                );
            }
            MapCanvasAction::MirrorPastePreviewX => {
                self.transform_map_frames(self.map_frame_transform().toggle_mirror_x(), cx);
            }
            MapCanvasAction::MirrorPastePreviewZ => {
                self.transform_map_frames(self.map_frame_transform().toggle_mirror_z(), cx);
            }
            MapCanvasAction::TogglePastePreviewTools => {
                self.map_image.tools_expanded = !self.map_image.tools_expanded;
                let colors = self.theme_colors(cx);
                self.sync_canvas_snapshot(colors, cx);
                cx.notify();
            }
            MapCanvasAction::ConfirmPastePreview => self.install_map_bundle_selected(cx),
            MapCanvasAction::CancelPastePreview => {
                if !self.map_image_operation_busy() {
                    self.clear_map_frame_placement(cx);
                    self.set_import_workspace_mode(
                        super::super::state::ImportWorkspaceMode::Preview,
                        cx,
                    );
                }
            }
            MapCanvasAction::ExportPastePreviewImage => self.export_map_bundle(cx),
            _ => return false,
        }
        true
    }

    fn transform_map_frames(&mut self, transform: PasteTransform, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() || self.map_image_busy() {
            return;
        }
        let options = &mut self.map_image.options;
        let quarter_turns = PasteRotation::ALL
            .iter()
            .position(|rotation| *rotation == transform.rotation)
            .unwrap_or(0) as u8;
        if options.transform.quarter_turns % 2 != quarter_turns % 2 {
            std::mem::swap(&mut options.columns, &mut options.rows);
        }
        options.transform = image_to_map::transform::Options {
            quarter_turns,
            mirror_x: transform.mirror_x,
            mirror_z: transform.mirror_z,
        };
        self.start_map_preview(cx);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
    }
}

//! Primary import actions stay on the central canvas, beside their result.

use super::model::MapViewerWindowView;
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn import_task_snapshot(&self) -> Option<&Arc<TaskSnapshot>> {
        let write_task = self
            .professional
            .active_write_task_id
            .as_deref()
            .and_then(|id| self.task_snapshots.get(id));
        if write_task.is_some_and(|task| !task.is_terminal()) {
            return write_task;
        }
        let tasks = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => [
                self.generator.preview_task_id.as_deref(),
                self.generator_export_task_id(),
                None,
            ],
            MapViewerRightPanel::ImageGenerator => [
                self.image_generator.preview_task_id.as_deref(),
                self.image_export_task_id(),
                None,
            ],
            _ => {
                return self
                    .map_image_task_snapshot()
                    .into_iter()
                    .chain(write_task)
                    .max_by_key(|task| {
                        (
                            !task.is_terminal(),
                            task.started_at_unix,
                            task.last_update_unix,
                        )
                    });
            }
        };
        tasks
            .into_iter()
            .flatten()
            .filter_map(|id| self.task_snapshots.get(id))
            .chain(write_task)
            .max_by_key(|task| {
                (
                    !task.is_terminal(),
                    task.started_at_unix,
                    task.last_update_unix,
                )
            })
    }

    pub(super) fn render_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let actions = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.render_obj_import_actions(colors, cx),
            MapViewerRightPanel::ImageGenerator => self.render_image_import_actions(colors, cx),
            _ => self.render_map_import_actions(colors, cx),
        };
        let task = self.import_task_snapshot();
        div()
            .flex_none()
            .m(px(12.0))
            .occlude()
            .p(px(10.0))
            .rounded(px(8.0))
            .bg(colors.surface)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(
                self.ui_state.import_workspace_mode == super::state::ImportWorkspaceMode::Preview
                    || self.ui_state.active_right_panel == MapViewerRightPanel::MapImage,
                |bar| bar.child(actions),
            )
            .when_some(task, |bar, task| {
                bar.child(self.render_import_task_status(colors, task, cx))
            })
    }
}

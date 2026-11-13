//! Primary import actions stay on the central canvas, beside their result.

use super::model::View;
use super::prelude::*;
use super::state::ImportWorkspaceMode;
use crate::ui::components::button::{primary_button, secondary_button};

impl View {
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

    /// Persistent placement command strip shared by OBJ and image-to-block tools.
    /// The conversion mode owns file/parameter controls; this strip only manipulates
    /// an existing, non-destructive world-space placement preview.
    fn render_structure_placement_commands(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let preview = self.professional.paste_preview.as_ref();
        let valid_source = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.preview_result_active,
            MapViewerRightPanel::ImageGenerator => self.image_generator.preview_result_active,
            _ => false,
        };
        let can_position = valid_source
            && preview.is_some_and(|preview| !preview.is_writing())
            && !self.map_write_busy();
        let can_commit = can_position && self.converter_target_selected() == Some(true);
        let footprint = preview
            .map(|preview| preview.targets.len())
            .unwrap_or_default();
        let anchor_text = preview.map_or_else(
            || "预览尚未准备，完成转换后再选择放置位置".to_owned(),
            |preview| {
                format!(
                    "锚点 X {} · Z {} · {} 个区块 · {}",
                    preview.target_anchor.x,
                    preview.target_anchor.z,
                    footprint,
                    if self.converter_target_selected() == Some(true) {
                        "已选点"
                    } else {
                        "拖动预览选择位置"
                    },
                )
            },
        );

        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .child(anchor_text),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        secondary_button(colors, "map-structure-target-center", "放到当前视野中心")
                            .when(can_position, |button| {
                                button.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.move_preview_to_view_center(cx);
                                    }),
                                )
                            })
                            .opacity(if can_position { 1.0 } else { 0.45 }),
                    )
                    .child(
                        secondary_button(colors, "map-structure-focus-preview", "居中查看预览")
                            .when(preview.is_some(), |button| {
                                button.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.center_view_on_preview(cx);
                                    }),
                                )
                            })
                            .opacity(if preview.is_some() { 1.0 } else { 0.45 }),
                    )
                    .child(
                        primary_button(
                            colors,
                            "map-structure-confirm-placement",
                            "确认写入当前世界",
                        )
                        .when(can_commit, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.confirm_paste_preview(cx);
                                }),
                            )
                        })
                        .opacity(if can_commit { 1.0 } else { 0.45 }),
                    ),
            )
    }

    pub(super) fn render_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let task = self.import_task_snapshot();
        let placement = self.ui_state.import_workspace_mode == ImportWorkspaceMode::Placement;
        let structure_converter = matches!(
            self.ui_state.active_right_panel,
            MapViewerRightPanel::Generator | MapViewerRightPanel::ImageGenerator
        );
        // A compact command inspector occupies actual layout space before the preview.
        // The canvas retains the remaining flex height rather than being occluded.
        div()
            .flex_none()
            .min_w(px(0.0))
            .max_h(px(164.0))
            .overflow_y_scrollbar()
            .mx(px(8.0))
            .mt(px(6.0))
            .occlude()
            .p(px(10.0))
            .rounded(px(8.0))
            .bg(colors.surface)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(
                (!placement || self.ui_state.active_right_panel == MapViewerRightPanel::MapImage)
                    && self.ui_state.active_right_panel != MapViewerRightPanel::Generator,
                |bar| {
                    // Avoid constructing hidden conversion actions (and their event
                    // handlers) on every world-placement render.
                    let actions = match self.ui_state.active_right_panel {
                        MapViewerRightPanel::Generator => self.obj_import_actions(colors, cx),
                        MapViewerRightPanel::ImageGenerator => {
                            self.image_import_actions(colors, cx)
                        }
                        _ => self.render_map_import_actions(colors, cx),
                    };
                    bar.child(actions)
                },
            )
            .when(placement && structure_converter, |bar| {
                bar.child(self.render_structure_placement_commands(colors, cx))
            })
            .when_some(
                task.filter(|task| !task.is_terminal() || task.status.as_ref() == "error"),
                |bar, task| bar.child(self.render_import_task_status(colors, task, cx)),
            )
    }
}

//! Import previews and placement controls owned by the central workspace.

use super::generator_panel::generator_choice;
use super::model::{MapViewerWindowView, PasteTransform};
use super::prelude::*;
use super::state::ImportWorkspaceMode;

impl MapViewerWindowView {
    /// An open import editor owns the central preview while keeping its parameters visible.
    pub(super) fn import_workspace_active(&self) -> bool {
        self.ui_state.right_panel_open && self.ui_state.active_right_panel.is_import()
    }

    /// Switch central content, rebuilding cancelled placement from the decoded result.
    /// This changes UI state only; confirmation owns the world write.
    pub(super) fn set_import_workspace_mode(
        &mut self,
        mode: ImportWorkspaceMode,
        cx: &mut Context<Self>,
    ) {
        self.cancel_pointer_captures_for_panel_interaction("import workspace switch", cx);
        if mode == ImportWorkspaceMode::Placement && self.professional.paste_preview.is_none() {
            let anchor = match self.ui_state.active_right_panel {
                MapViewerRightPanel::Generator if self.generator.preview_result_active => {
                    self.generator.preview_anchor
                }
                MapViewerRightPanel::ImageGenerator
                    if self.image_generator.preview_result_active =>
                {
                    self.image_generator.preview_anchor
                }
                _ => None,
            };
            if let Some(anchor) = anchor {
                self.set_paste_preview(anchor, PasteTransform::default(), 0.0, None, cx);
            }
        }
        self.ui_state.import_workspace_mode = mode;
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    /// Project import snapshots and camera state; interactions schedule conversion separately.
    pub(super) fn render_import_workspace(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let preview = self.ui_state.import_workspace_mode == ImportWorkspaceMode::Preview;
        let body = if preview {
            self.render_import_result(colors, cx).into_any_element()
        } else {
            self.canvas_view.clone().into_any_element()
        };
        div()
            .relative()
            .flex_1()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(body)
            .child(self.render_import_workspace_toolbar(colors, cx))
            .when(
                preview
                    || self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
                    || self.import_task_snapshot().is_some(),
                |workspace| workspace.child(self.render_import_actions(colors, cx)),
            )
    }

    fn render_import_workspace_toolbar(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let preview = self.ui_state.import_workspace_mode == ImportWorkspaceMode::Preview;
        div()
            .absolute()
            .top(px(8.0))
            .left(px(8.0))
            .right(px(8.0))
            .occlude()
            .p(px(8.0))
            .rounded(px(8.0))
            .bg(colors.surface)
            .flex()
            .items_center()
            .gap(px(8.0))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .children(
                [
                    (ImportWorkspaceMode::Preview, "转换预览"),
                    (ImportWorkspaceMode::Placement, "地图与放置"),
                ]
                .into_iter()
                .map(|(mode, label)| {
                    generator_choice(colors, label, self.ui_state.import_workspace_mode == mode)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.set_import_workspace_mode(mode, cx);
                                cx.stop_propagation();
                            }),
                        )
                }),
            )
            .child(div().flex_1())
            .when(
                preview && self.ui_state.active_right_panel != MapViewerRightPanel::MapImage,
                |bar| {
                    bar.child(generator_choice(colors, "重置视角", false).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.reset_preview_3d_camera(cx)),
                    ))
                },
            )
    }

    fn render_import_result(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let body = match self.ui_state.active_right_panel {
            MapViewerRightPanel::MapImage => self.render_map_image_comparison(colors),
            MapViewerRightPanel::ImageGenerator => self.render_image_block_comparison(colors, cx),
            _ => self.render_import_block_canvas(colors, cx),
        };
        div()
            .size_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .pt(px(66.0))
            .flex()
            .flex_col()
            .child(body)
    }

    pub(super) fn render_import_block_canvas(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let owner_ready = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.preview_result_active,
            MapViewerRightPanel::ImageGenerator => self.image_generator.preview_result_active,
            _ => false,
        };
        let view = cx.entity();
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(import_preview_label(
                colors,
                if owner_ready {
                    "方块结果 · 左键旋转模型，右键环绕，滚轮缩放"
                } else {
                    "选择素材后自动转换；调整右侧参数更新预览"
                },
            ))
            .when(owner_ready, |panel| {
                panel.child(import_preview_label(colors, self.preview_3d_status_label()))
            })
            .when(owner_ready, |panel| {
                panel.child(self.render_preview_3d_canvas(
                    colors,
                    self.preview_3d.mesh.clone(),
                    self.preview_3d.camera,
                    self.preview_3d.model_rotation,
                    view,
                    cx,
                ))
            })
    }
}

pub(super) fn import_preview_label(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    div()
        .px(px(12.0))
        .py(px(8.0))
        .text_size(px(12.0))
        .text_color(colors.text_secondary)
        .child(text.into())
}

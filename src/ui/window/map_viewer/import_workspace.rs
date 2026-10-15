//! Import previews and placement controls owned by the central workspace.

use super::generator_panel::generator_choice;
use crate::ui::components::button::{primary_button, secondary_button};
use super::model::{ImportNumericAxis, MapViewerWindowView, PasteTransform};
use super::map_image_panel::{GridAxis, MapImageInstallDestination};
use super::image_generator_panel::ImageAxis;
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
        if mode == ImportWorkspaceMode::Placement
            && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
        {
            self.ensure_map_image_frame_anchor();
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
        let inventory_target = self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image_installing_to_player();
        let source_missing = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.source.is_none(),
            MapViewerRightPanel::ImageGenerator => self.image_generator.source.is_none(),
            MapViewerRightPanel::MapImage => {
                self.map_image.source.is_none() && self.map_image.pending_bundle.is_none()
            }
            _ => false,
        };
        let body = if source_missing {
            self.render_import_source_empty_state(colors, cx).into_any_element()
        } else if preview {
            self.render_import_result(colors, cx).into_any_element()
        } else if inventory_target {
            self.render_map_image_inventory_workspace(colors).into_any_element()
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
            // Commands are part of layout, above the preview; never overlay the world.
            .child(self.render_import_workspace_toolbar(colors, cx))
            .when(
                preview
                    || self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
                    || self.import_task_snapshot().is_some(),
                |workspace| workspace.child(self.render_import_actions(colors, cx)),
            )
            .child(body)
            .when(
                !preview && self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
                    && !inventory_target
                    && self.map_image_placement_inspector_visible(),
                |workspace| workspace.child(self.render_map_image_placement_thumbnail(colors)),
            )
    }

    fn render_import_workspace_toolbar(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let preview = self.ui_state.import_workspace_mode == ImportWorkspaceMode::Preview;
        let is_map_image = self.ui_state.active_right_panel == MapViewerRightPanel::MapImage;
        let frame_target = is_map_image
            && !preview
            && self.map_image.install_destination == MapImageInstallDestination::NewUpFrame;
        let can_place = self.map_image_can_place_world_grid();
        let source_available = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.source.is_some(),
            MapViewerRightPanel::ImageGenerator => self.image_generator.source.is_some(),
            MapViewerRightPanel::MapImage => self.map_image.source.is_some(),
            _ => false,
        };
        div()
            .flex_none()
            .mx(px(8.0))
            .mt(px(8.0))
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
                    (ImportWorkspaceMode::Preview, if is_map_image { "1 · 图片转换" } else { "转换预览" }),
                    (ImportWorkspaceMode::Placement, if is_map_image { "2 · 目标预览" } else { "地图与放置" }),
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
            .when(source_available, |bar| {
                bar.child(
                    secondary_button(colors, "map-import-change-source", "更换素材…").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.choose_import_source_from_workspace(cx)
                        }),
                    ),
                )
            })
            .child(div().flex_1())
            .when(is_map_image && !frame_target, |bar| {
                bar.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .child(self.map_image_workflow_status_label()),
                )
            })
            .when(frame_target, |bar| {
                bar.child(
                    primary_button(
                        colors,
                        "map-image-place-directly-in-world",
                        if self.map_image_operation_busy() {
                            "正在写入世界…"
                        } else {
                            "直接放置到当前世界"
                        },
                    )
                    .opacity(if can_place { 1.0 } else { 0.46 })
                    .when(can_place, |button| {
                        button.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.install_map_bundle_selected(cx);
                                cx.stop_propagation();
                            }),
                        )
                    }),
                )
            })
            .when(is_map_image && !preview && !self.map_image_installing_to_player(), |bar| {
                bar.child(
                    generator_choice(
                        colors,
                        if self.map_image_placement_inspector_visible() {
                            "收起缩略图"
                        } else {
                            "显示缩略图"
                        },
                        false,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.toggle_map_image_placement_inspector(cx)
                        }),
                    ),
                )
            })
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
            .p(px(8.0))
            .flex()
            .flex_col()
            .child(body)
    }

    /// Commit only complete, valid numeric values. Mid-edit strings never run conversions.
    pub(super) fn commit_import_numeric_axis(
        &mut self,
        axis: ImportNumericAxis,
        cx: &mut Context<Self>,
    ) {
        let (parsed, previous, min, max) = match axis {
            ImportNumericAxis::MapColumns => {
                let other = self.map_image.options.rows.max(1);
                let max = (1024 / other).min(64);
                (self.num_map_columns.parse(cx, 1, max),
                    self.map_image.options.columns, 1, max)
            }
            ImportNumericAxis::MapRows => {
                let other = self.map_image.options.columns.max(1);
                let max = (1024 / other).min(64);
                (self.num_map_rows.parse(cx, 1, max),
                    self.map_image.options.rows, 1, max)
            }
            ImportNumericAxis::ImageWidth => (
                self.num_image_width.parse(cx, 1, 512),
                self.image_generator.options.width, 1, 512,
            ),
            ImportNumericAxis::ImageHeight => (
                self.num_image_height.parse(cx, 1, 512),
                self.image_generator.options.height, 1, 512,
            ),
            ImportNumericAxis::ObjSize => (
                self.num_obj_size.parse(cx, 1, 384),
                u32::from(self.generator.options.longest_side_blocks), 1, 384,
            ),
        };
        if let Some(value) = parsed {
            if value != previous {
                match axis {
                    ImportNumericAxis::MapColumns => {
                        self.set_grid_size(GridAxis::Columns, value, cx);
                    }
                    ImportNumericAxis::MapRows => {
                        self.set_grid_size(GridAxis::Rows, value, cx);
                    }
                    ImportNumericAxis::ImageWidth => {
                        self.set_image_dimension(ImageAxis::Width, value, cx);
                    }
                    ImportNumericAxis::ImageHeight => {
                        self.set_image_dimension(ImageAxis::Height, value, cx);
                    }
                    ImportNumericAxis::ObjSize => {
                        self.set_obj_precision_size(value as u16, cx);
                    }
                }
            }
        } else {
            self.status = SharedString::from(format!(
                "请输入 {min}–{max} 范围内的整数，已恢复为 {previous}",
            ));
        }
        // Normalize leading zeros and restore invalid edits without forcing a new task.
        match axis {
            ImportNumericAxis::MapColumns => self.num_map_columns.sync(
                self.map_image.options.columns, cx,
            ),
            ImportNumericAxis::MapRows => self.num_map_rows.sync(
                self.map_image.options.rows, cx,
            ),
            ImportNumericAxis::ImageWidth => self.num_image_width.sync(
                self.image_generator.options.width, cx,
            ),
            ImportNumericAxis::ImageHeight => self.num_image_height.sync(
                self.image_generator.options.height, cx,
            ),
            ImportNumericAxis::ObjSize => self.num_obj_size.sync(
                u32::from(self.generator.options.longest_side_blocks), cx,
            ),
        }
        cx.notify();
    }

    /// Shared entry point for every import mode. The file picker is not buried in settings.
    fn choose_import_source_from_workspace(&mut self, cx: &mut Context<Self>) {
        match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.choose_generator_obj(cx),
            MapViewerRightPanel::ImageGenerator => self.choose_generator_image(cx),
            MapViewerRightPanel::MapImage => self.choose_map_source(cx),
            _ => {}
        }
    }

    /// Deliberately empty preview area: actionable, centered, and keyboard-focusable through
    /// the same reusable choice control used by the configuration panel.
    fn render_import_source_empty_state(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let (title, description, action) = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => (
                "从 OBJ 模型开始",
                "选择 OBJ 模型。支持关联 MTL 与同目录纹理，转换结果会显示在这里。",
                "选择 OBJ 模型…",
            ),
            MapViewerRightPanel::ImageGenerator => (
                "从图片创建方块模型",
                "导入 PNG、JPEG、TGA 或 WebP，先预览比例和色彩，再调整目标尺寸。",
                "选择图片…",
            ),
            MapViewerRightPanel::MapImage => (
                "从图片创建地图物品",
                "导入图片后可预览每张 128×128 地图、编辑拼图网格，再选择写入目标。",
                "选择图片…",
            ),
            _ => ("选择源素材", "选择素材以开始转换。", "选择素材…"),
        };
        div()
            .size_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .pt(px(68.0))
            .pb(px(12.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .max_w(px(480.0))
                    .w_full()
                    .mx(px(20.0))
                    .p(px(24.0))
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(17.0))
                            .text_color(colors.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .text_center()
                            .child(description),
                    )
                    .child(
                        primary_button(colors, "map-import-choose-source", action).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.choose_import_source_from_workspace(cx);
                            }),
                        ),
                    ),
            )
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

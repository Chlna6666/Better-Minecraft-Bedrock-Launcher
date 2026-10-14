use super::super::import_workspace::import_preview_label;
use super::*;

impl MapViewerWindowView {
    /// Non-destructive placement inspector sharing the generated map image.
    pub(in super::super) fn render_map_image_placement_thumbnail(
        &self,
        colors: &ThemeColors,
    ) -> Div {
        let target = match self.map_image.install_destination {
            MapImageInstallDestination::Player => "目标：玩家背包；不修改世界地形".to_owned(),
            MapImageInstallDestination::SelectedBlock => self.map_image.selected_block.map_or_else(
                || "在地图上右键选择箱子、潜影盒或空展示框".to_owned(),
                |(_, block)| format!("目标：{}, {}, {}", block.x, block.y, block.z),
            ),
            MapImageInstallDestination::NewUpFrame => {
                let (x, z) = self.map_frame_anchor();
                format!(
                    "展示框中心：{x}, {}, {z} · {}×{} 张",
                    self.y_layer, self.map_image.options.columns, self.map_image.options.rows,
                )
            }
        };
        div()
            .absolute()
            .top(px(66.0))
            .right(px(12.0))
            .w(px(260.0))
            .p(px(10.0))
            .rounded(px(8.0))
            .bg(colors.surface)
            .occlude()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
            .child(import_preview_label(colors, "地图物品 · 分片结果"))
            .when_some(self.map_image.preview.as_ref(), |panel, preview| {
                panel.child(
                    div()
                        .w_full()
                        .h(px(212.0))
                        .overflow_hidden()
                        .child(img(preview.clone()).size_full().object_fit(ObjectFit::Contain)),
                )
            })
            .when(self.map_image.preview.is_none(), |panel| {
                panel.child(import_preview_label(colors, "选择图片并完成转换后显示地图像素"))
            })
            .child(import_preview_label(colors, target))
            .child(import_preview_label(
                colors,
                "绿色网格对应世界放置范围；预览不会修改存档",
            ))
    }

    pub(in super::super) fn render_map_image_comparison(&self, colors: &ThemeColors) -> Div {
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(import_preview_label(colors, "源图片"))
                    .when_some(self.map_image.source.as_ref(), |pane, source| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .child(
                                    img(source.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(import_preview_label(
                        colors,
                        format!(
                            "地图像素 · {} 列 × {} 行",
                            self.map_image.options.columns, self.map_image.options.rows
                        ),
                    ))
                    .when_some(self.map_image.preview.as_ref(), |pane, preview| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .child(
                                    img(preview.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                    })
                    .when_some(self.map_image.preview_note.as_ref(), |pane, note| {
                        pane.child(import_preview_label(colors, note.clone()))
                    }),
            )
    }
}

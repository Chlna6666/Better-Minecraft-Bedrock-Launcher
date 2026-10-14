use super::super::import_workspace::import_preview_label;
use super::*;

impl MapViewerWindowView {
    /// Inventory delivery is not a world placement. Show the actual map mosaic and
    /// selected player here instead of opening an unrelated terrain canvas.
    pub(in super::super) fn render_map_image_inventory_workspace(
        &self,
        colors: &ThemeColors,
    ) -> Div {
        let player = super::super::players::player_id_label(&self.map_image.selected_player);
        let columns = self.map_image.options.columns;
        let rows = self.map_image.options.rows;
        div()
            .size_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .pt(px(66.0))
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(colors.text_primary)
                    .child("背包写入预览"),
            )
            .child(import_preview_label(
                colors,
                format!(
                    "目标玩家：{player} · {columns} 列 × {rows} 行 · {} 张地图物品",
                    columns * rows
                ),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(colors.surface)
                    .overflow_hidden()
                    .when_some(self.map_image.preview.as_ref(), |pane, preview| {
                        pane.child(
                            img(preview.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    .when(self.map_image.preview.is_none(), |pane| {
                        pane.child(import_preview_label(
                            colors,
                            "尚未生成地图像素。请先选择图片并完成转换。",
                        ))
                    }),
            )
            .child(import_preview_label(
                colors,
                "这是发放给玩家的地图物品，不会放置展示框或修改地图地形。若要在世界中铺设，请在下方选择「新建展示框」。",
            ))
    }

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
                        .h(px(152.0))
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
                match self.map_image.install_destination {
                    MapImageInstallDestination::SelectedBlock =>
                        "地图上高亮的是选中容器。此卡片预览待写入物品，不代表容器内已经存在这些地图。",
                    MapImageInstallDestination::NewUpFrame =>
                        "世界画布展示地图拼图与绿色分片网格；右键地图可重新指定中心。",
                    MapImageInstallDestination::Player =>
                        "背包地图物品预览，不会修改世界地形。",
                },
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

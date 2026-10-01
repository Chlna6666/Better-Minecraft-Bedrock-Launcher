use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_map_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.map_image_operation_busy() || self.map_write_busy();
        let ready = self.map_image.preview.is_some()
            && self.map_image.task_id.as_deref().is_some_and(|id| {
                self.task_snapshots
                    .get(id)
                    .is_some_and(|task| task.status.as_ref() == "completed")
            });
        let target = match self.map_image.install_destination {
            MapImageInstallDestination::Player => format!(
                "写入玩家：{}",
                super::super::players::player_id_label(&self.map_image.selected_player)
            ),
            MapImageInstallDestination::SelectedBlock => self.map_image.selected_block.map_or_else(
                || "右键地图 → 更多 → 选为地图写入目标".to_owned(),
                |(_, block)| format!("写入目标：{}, {}, {}", block.x, block.y, block.z),
            ),
            MapImageInstallDestination::NewUpFrame => {
                let (x, z) = self.viewport.center_block(self.active_layout);
                format!(
                    "创建展示框：中心 {x}, {}, {z} · {} 列 × {} 行",
                    self.y_layer, self.map_image.options.columns, self.map_image.options.rows
                )
            }
        };
        let target_ready = self.map_image.install_destination
            != MapImageInstallDestination::SelectedBlock
            || self.map_image.selected_block.is_some();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .child(target),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.0))
                    .child(
                        generator_choice(
                            colors,
                            if busy {
                                "正在写入 / 导出…"
                            } else {
                                "确认写入当前目标"
                            },
                            true,
                        )
                        .when(ready && target_ready && !busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.install_map_bundle_selected(cx)),
                            )
                        }),
                    )
                    .when(ready && !busy, |bar| {
                        bar.child(
                            generator_choice(colors, "导出地图包…", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.export_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(!busy, |bar| {
                        bar.child(
                            generator_choice(colors, "导入地图包 / 容器 NBT…", false)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.import_map_bundle_file_selected(cx)
                                    }),
                                ),
                        )
                    })
                    .when(
                        self.map_image.source.is_some() && !self.map_image_busy() && !busy,
                        |bar| {
                            bar.child(generator_choice(colors, "刷新预览", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.release_map_image_preview();
                                    this.start_map_preview(cx);
                                }),
                            ))
                        },
                    ),
            )
    }
}

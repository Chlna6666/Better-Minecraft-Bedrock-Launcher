use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_map_image_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let operation_busy = self.map_image_operation_busy();
        let can_export = self.map_image.preview.is_some()
            && self.map_image.task_id.as_ref().is_some_and(|id| {
                self.task_snapshots
                    .get(id.as_str())
                    .is_some_and(|snapshot| snapshot.status.as_ref() == "completed")
            });
        let map_count = self.map_image.options.columns * self.map_image.options.rows;
        let grid_resolved = !self.map_image.options.auto_fit
            || self.map_image.grid_estimated
            || self.map_image.preview_done > 0
            || can_export;
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.render_import_panel_header(colors, "图片转地图物品", cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(15.0))
                    .child(generator_section_title(colors, "源图片"))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(self.map_image.source.as_ref().map_or_else(
                                || "尚未选择图片".to_owned(),
                                |path| path.display().to_string(),
                            )),
                    )
                    .child(generator_choice(colors, "选择图片…", false).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.choose_map_source(cx)),
                        ))
                    .when(operation_busy, |panel| panel.child(
                        div().text_size(px(12.0)).text_color(colors.text_secondary)
                            .child("导出或写入期间保留当前参数和目标；可取消当前任务。"),
                    ))
                    .child(self.render_grid_axis(GridAxis::Columns, colors, cx))
                    .child(self.render_grid_axis(GridAxis::Rows, colors, cx))
                    .child(
                        generator_choice(
                            colors,
                            "按原图尺寸自动匹配分片",
                            self.map_image.options.auto_fit,
                        )
                        .when(self.map_image.source.is_some(), |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.enable_grid_auto_fit(cx)
                                }),
                            )
                        }),
                    )
                    .child(generator_section_title(colors, "地图包写入目标"))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            (MapImageInstallDestination::Player, "选中玩家背包"),
                            (MapImageInstallDestination::SelectedBlock, "选中箱子／潜影盒／空展示框"),
                            (MapImageInstallDestination::NewUpFrame, "新建朝上展示框"),
                        ]
                        .into_iter()
                        .map(|(destination, label)| {
                            generator_choice(
                                colors,
                                label,
                                self.map_image.install_destination == destination,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_map_install_destination(destination, cx)
                                }),
                            )
                        }),
                    ))
                    .when(
                        self.map_image.install_destination == MapImageInstallDestination::Player,
                        |panel| {
                            panel.child(self.render_map_player_picker(colors, cx))
                        },
                    )
                    .when(self.map_image.install_destination == MapImageInstallDestination::NewUpFrame, |panel| panel
                    .child(generator_section_title(colors, "朝上展示框放置"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(generator_choice(colors, "Y −1", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.step_y(-1, cx)),
                            ))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.text_primary)
                                    .child(format!("展示框 Y {} · 支撑 Y {}", self.y_layer, self.y_layer.saturating_sub(1))),
                            )
                            .child(generator_choice(colors, "Y +1", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.step_y(1, cx)),
                            )),
                    )
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            ("minecraft:stone", "石头"),
                            ("minecraft:glass", "玻璃"),
                            ("minecraft:white_stained_glass", "白色染色玻璃"),
                            ("minecraft:coal_block", "煤炭块"),
                            ("minecraft:iron_block", "铁块"),
                            ("minecraft:diamond_block", "钻石块"),
                        ]
                        .into_iter()
                        .map(|(name, label)| {
                            generator_choice(colors, label, self.map_image.frame_support == name)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        this.set_map_frame_support(name, cx)
                                    }),
                                )
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("按地图行列在世界画布中心铺设朝上展示框；默认石头支撑，可选其他支撑块。框位必须为空，支撑位可为空或已是所选方块。"),
                    ))
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .whitespace_normal()
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .child(if self.map_image.source.is_none() {
                                "选择图片后会根据原图宽高自动估算分片数量。".to_owned()
                            } else if grid_resolved {
                                format!(
                                    "{map_count} 张地图 · {}×{} 像素，每张固定 128×128。",
                                    self.map_image.options.columns * 128,
                                    self.map_image.options.rows * 128,
                                )
                            } else {
                                "正在按源图宽高估算分片；单边最多 64 张，总计最多 1024 张。完成预览后显示准确尺寸。"
                                    .to_owned()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("多图按网格编号；写入玩家背包时分组为潜影盒，写入现有箱子时在箱内分组。"),
                    )
                    .child(generator_section_title(colors, "裁剪比例"))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [(true, "居中裁剪 · 匹配拼图比例"), (false, "保留全图 · 可拉伸")]
                            .into_iter()
                            .map(|(center_crop, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.map_image.options.center_crop == center_crop,
                                )
                                .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            if this.map_image_operation_busy() { return; }
                                            this.map_image.options.center_crop = center_crop;
                                            this.start_map_preview(cx);
                                        }),
                                    )
                            }),
                    ))
                    .child(generator_section_title(colors, "缩放采样"))
                    .child(div().flex().gap(px(6.0)).children(
                        [
                            (MapResample::Nearest, "最近邻"),
                            (MapResample::Lanczos3, "Lanczos3"),
                        ]
                        .into_iter()
                        .map(|(resample, label)| {
                            generator_choice(
                                colors,
                                label,
                                self.map_image.options.resample == resample,
                            )
                            .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if this.map_image_operation_busy() { return; }
                                        this.map_image.options.resample = resample;
                                        this.start_map_preview(cx);
                                    }),
                                )
                        }),
                    ))
                    .child(generator_section_title(colors, "透明度"))
                    .child(div().flex().gap(px(6.0)).children(
                        [(1_u8, "保留边缘"), (128, "忽略半透明"), (255, "仅不透明")]
                            .into_iter()
                            .map(|(alpha, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.map_image.options.alpha_threshold == alpha,
                                )
                                .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            if this.map_image_operation_busy() { return; }
                                            this.map_image.options.alpha_threshold = alpha;
                                            this.start_map_preview(cx);
                                        }),
                                    )
                            }),
                    ))
                    ,
            )
            .into_any_element()
    }
}

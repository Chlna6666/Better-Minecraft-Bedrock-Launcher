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
                    .child(generator_section_title(colors, "01  素材"))
                    .child(crate::ui::components::inspector::inspector_source_card(
                        colors,
                        "MAP / 128",
                        self.map_image.source.as_deref(),
                        "PNG / JPG / TGA / WebP · 可拖入",
                        crate::ui::components::button::secondary_button(
                            colors, "map-image-source", "选择图片…",
                        )
                        .when(!operation_busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.choose_map_source(cx)),
                            )
                        })
                        .opacity(if operation_busy { 0.45 } else { 1.0 }),
                    ))
                    .when(operation_busy, |panel| panel.child(
                        div().text_size(px(11.0)).text_color(colors.text_secondary)
                            .child("导出或写入中 · 目标与设置暂时锁定"),
                    ))
                    .child(crate::ui::components::inspector::inspector_result_summary(
                        colors, "地图网格",
                        if grid_resolved {
                            format!(
                                "{} × {} · 共 {} 张 · {} × {} px",
                                self.map_image.options.columns, self.map_image.options.rows,
                                map_count,
                                self.map_image.options.columns * 128,
                                self.map_image.options.rows * 128,
                            )
                        } else {
                            "正在计算最佳分片数".to_owned()
                        },
                    ))
                    .child(generator_section_title(colors, "02  网格与画面利用率"))
                    .child(
                        generator_choice(
                            colors,
                            "智能行列 · 按图片比例减少留白",
                            self.map_image.options.auto_fit,
                        )
                        .when(self.map_image.source.is_some() && !operation_busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.enable_grid_auto_fit(cx)
                                }),
                            )
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("智能模式先去除透明外边距，再匹配横纵比例；必要时增加少量地图张数。手动修改列/行即切换固定网格。"),
                    )
                    .child(self.render_grid_axis(GridAxis::Columns, colors, cx))
                    .child(self.render_grid_axis(GridAxis::Rows, colors, cx))
                    .child(generator_section_title(colors, "03  写入目标"))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("在中央操作栏选择写入目标；此处仅显示对应目标的参数。"),
                    )
                    .when(
                        self.map_image.install_destination == MapImageInstallDestination::Player,
                        |panel| {
                            panel.child(self.render_map_player_picker(colors, cx))
                        },
                    )
                    .when(
                        self.map_image.install_destination == MapImageInstallDestination::SelectedBlock,
                        |panel| {
                            panel
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(colors.text_primary)
                                        .child(self.map_image.selected_block.map_or_else(
                                            || "未选择容器或展示框".to_owned(),
                                            |(chunk, block)| {
                                                if chunk.dimension != self.dimension {
                                                    "所选目标在其他维度，请重新选择".to_owned()
                                                } else {
                                                    format!(
                                                        "已选：X {} · Y {} · Z {}",
                                                        block.x, block.y, block.z,
                                                    )
                                                }
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(colors.text_secondary)
                                        .child("在世界地图上右键方块，选择「设为地图写入目标」。写入任务会检查容器类型和容量。"),
                                )
                                .when(self.map_image.selected_block.is_some(), |panel| {
                                    panel.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                crate::ui::components::button::secondary_button(
                                                    colors, "map-container-focus", "定位目标",
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.focus_selected_map_image_block(cx);
                                                    }),
                                                ),
                                            )
                                            .when(!self.map_image_operation_busy(), |row| {
                                                row.child(
                                                    crate::ui::components::button::ghost_button(
                                                        colors, "map-container-clear", "清除选择",
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.clear_selected_map_image_block(cx);
                                                        }),
                                                    ),
                                                )
                                            }),
                                    )
                                })
                                .child(self.render_map_container_candidates(colors, cx))
                        },
                    )
                    .when(self.map_image.install_destination == MapImageInstallDestination::NewUpFrame, |panel| panel
                    .child(generator_section_title(colors, "朝上展示框放置"))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(if self.map_frame_confirmed_center().is_some() {
                                        colors.text_primary
                                    } else {
                                        colors.danger
                                    })
                                    .child(self.map_frame_confirmed_center().map_or_else(
                                        || "未选择放置位置：请点击下方按钮，然后在中央地图上点选".to_owned(),
                                        |(x, z)| format!("已选择中心：X {x} · Z {z}"),
                                    )),
                            )
                            .child(
                                crate::ui::components::button::primary_button(
                                    colors,
                                    "map-frame-pick-world-center",
                                    if self.map_image.picking_frame_center {
                                        "正在选点 · 点击地图选择位置"
                                    } else if self.map_frame_confirmed_center().is_some() {
                                        "重新在地图上选择中心"
                                    } else {
                                        "在地图上选择放置位置"
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.begin_map_frame_center_pick(cx);
                                    }),
                                ),
                            )
                            .child(
                                crate::ui::components::button::secondary_button(
                                    colors, "map-frame-use-viewport-center",
                                    "使用当前视图中心（主动确认）",
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.set_map_frame_anchor_to_viewport(cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child("选点仅确定 X/Z，展示框使用下方 Y 层。已选位置会在世界地图上显示分片占地，移动视口不会改变坐标。"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(crate::ui::components::numeric_field::numeric_step_button(
                colors, "Y −1", true,
            ).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.step_y(-1, cx)),
                            ))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.text_primary)
                                    .child(format!("展示框 Y {} · 支撑 Y {}", self.y_layer, self.y_layer.saturating_sub(1))),
                            )
                            .child(crate::ui::components::numeric_field::numeric_step_button(
                colors, "Y +1", true,
            ).on_mouse_down(
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
                            .child("在当前地图编辑器中直接写入世界展示框（无需进入游戏手动摆放）；默认石头支撑，可选其他支撑块。框位必须为空，支撑位可为空或已是所选方块。"),
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
                    .child(generator_section_title(colors, "04  图片适配与采样"))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            (MapImageFit::Contain, "完整图片 · 等比留边"),
                            (MapImageFit::CenterCrop, "居中裁切 · 丢失边缘"),
                            (MapImageFit::Stretch, "完整拉伸 · 可能变形"),
                        ]
                        .into_iter()
                        .map(|(fit, label)| {
                            generator_choice(colors, label, self.map_image.options.fit == fit)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if this.map_image_operation_busy() { return; }
                                        this.map_image.options.fit = fit;
                                        this.start_map_preview(cx);
                                    }),
                                )
                        }),
                    ))
                    .when(self.map_image.options.fit == MapImageFit::Contain, |panel| {
                        panel
                            .child(
                                generator_choice(
                                    colors,
                                    "自动去除 PNG 透明外边距",
                                    self.map_image.options.trim_transparent_edges,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        if this.map_image_operation_busy() {
                                            return;
                                        }
                                        this.map_image.options.trim_transparent_edges =
                                            !this.map_image.options.trim_transparent_edges;
                                        this.start_map_preview(cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child(if self.map_image.options.auto_fit {
                                        "自动行列会按可见内容比例选择分片；固定行列仍可能产生必要留白。"
                                    } else {
                                        "当前为固定行列。为了减少留白，请使用「按原图尺寸自动匹配分片」。"
                                    }),
                            )
                    })
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

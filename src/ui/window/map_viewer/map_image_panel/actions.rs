use super::*;

impl MapViewerWindowView {
    /// One action bar for all map-image destinations. Selection is kept next to
    /// the result instead of hidden below a long options form.
    pub(in super::super) fn render_map_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.map_image_operation_busy() || self.map_write_busy();
        let converting = self.map_image_busy();
        let ready = self.map_image.preview.is_some()
            && self.map_image.task_id.as_deref().is_some_and(|id| {
                self.task_snapshots
                    .get(id)
                    .is_some_and(|task| task.status.as_ref() == "completed")
            });
        let selected = self.map_image.install_destination;
        let pending = self.map_image.pending_bundle.as_ref();
        let (primary, target, tip, target_ready) = match selected {
            MapImageInstallDestination::Player => (
                "写入玩家背包",
                format!(
                    "玩家：{}",
                    super::super::players::player_id_label(&self.map_image.selected_player)
                ),
                "地图物品会发放到玩家物品栏／潜影盒，不在世界中建造展示框。",
                true,
            ),
            MapImageInstallDestination::SelectedBlock => (
                "写入所选容器",
                self.map_image.selected_block.map_or_else(
                    || "尚未选择容器或展示框".to_owned(),
                    |(chunk, block)| {
                        if chunk.dimension == self.dimension {
                            format!("已选目标：{}, {}, {}", block.x, block.y, block.z)
                        } else {
                            "原目标属于其他维度，请在当前维度重新选择".to_owned()
                        }
                    },
                ),
                "在地图上右键目标方块，选择首项「设为地图写入目标」；选中后会在世界画布高亮位置。",
                self.map_image.selected_block.is_some_and(|(chunk, _)| {
                    chunk.dimension == self.dimension
                }),
            ),
            MapImageInstallDestination::NewUpFrame => {
                let (x, z) = self.map_frame_anchor();
                (
                    "创建并写入展示框",
                    if pending.is_some() {
                        format!("锁定位置：X {x} · Y {} · Z {z}；待解析 NBT 分片", self.y_layer)
                    } else {
                        format!(
                            "锁定位置：X {x} · Y {} · Z {z}；{}×{} 张",
                            self.y_layer, self.map_image.options.columns, self.map_image.options.rows
                        )
                    },
                    "世界画布显示展示框合成图；右键地图可以精确设置中心，拖动视口不会移动已锁定的位置。",
                    true,
                )
            }
        };
        let status = if busy {
            "写入／导出中，暂时锁定目标与参数"
        } else if pending.is_some() {
            "已暂存 NBT 文件：请检查目标后确认导入。外部文件尚无地图像素预览"
        } else if self.map_image.source.is_none() {
            "第一步：在右侧选择图片，生成真实地图像素"
        } else if converting {
            "正在转换图片，完成后可选择写入目标"
        } else if !ready {
            "地图像素未准备好，请先完成转换或刷新预览"
        } else if !target_ready {
            "请先在世界地图上指定有效写入目标"
        } else {
            "地图像素已就绪。检查目标后再确认写入，不会在预览时修改存档"
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child("写入目标"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(status),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(
                        [
                            (MapImageInstallDestination::Player, "玩家背包"),
                            (MapImageInstallDestination::SelectedBlock, "现有容器／展示框"),
                            (MapImageInstallDestination::NewUpFrame, "新建展示框网格"),
                        ]
                        .into_iter()
                        .map(|(destination, label)| {
                            generator_choice(colors, label, selected == destination)
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.set_map_install_destination(destination, cx)
                                        }),
                                    )
                                })
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(target),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(tip),
                    ),
            )
            .when_some(pending, |bar, path| {
                bar.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_primary)
                        .child(format!("待确认文件：{}", path.display())),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        generator_choice(
                            colors,
                            if pending.is_some() { "确认 NBT 导入并写入" } else { primary },
                            (ready || pending.is_some()) && target_ready && !busy,
                        )
                        .when((ready || pending.is_some()) && target_ready && !busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.map_image.pending_bundle.is_some() {
                                        this.confirm_pending_map_bundle(cx);
                                    } else {
                                        this.install_map_bundle_selected(cx);
                                    }
                                }),
                            )
                        }),
                    )
                    .when(ready && pending.is_none() && !busy, |bar| {
                        bar.child(
                            generator_choice(colors, "导出地图包…", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.export_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(!busy, |bar| {
                        bar.child(
                            generator_choice(
                                colors,
                                if pending.is_some() { "更换待导入 NBT…" } else { "选择地图包／容器 NBT…" },
                                false,
                            )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.import_map_bundle_file_selected(cx)
                                    }),
                                ),
                        )
                    })
                    .when(pending.is_some() && !busy, |bar| {
                        bar.child(
                            generator_choice(colors, "取消待导入", false).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.cancel_pending_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(
                        self.map_image.source.is_some() && pending.is_none() && !converting && !busy,
                        |bar| {
                            bar.child(generator_choice(colors, "重新生成", false).on_mouse_down(
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

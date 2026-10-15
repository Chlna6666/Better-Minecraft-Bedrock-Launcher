use super::*;
use crate::ui::components::button::{ghost_button, primary_button, secondary_button};

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
        let frame_preview = selected == MapImageInstallDestination::NewUpFrame
            && pending.is_none();
        let needs_target_review = frame_preview
            && self.ui_state.import_workspace_mode
                == super::super::state::ImportWorkspaceMode::Preview;
        let check = if frame_preview {
            self.map_frame_preflight_snapshot()
        } else {
            None
        };
        let needs_position = frame_preview && !needs_target_review
            && (self.map_frame_confirmed_center().is_none() || self.map_image.picking_frame_center);
        let check_running = check.is_some_and(|task| !task.is_terminal());
        let check_passed = self.map_frame_preflight_passed();
        let action_enabled = (ready || pending.is_some()) && !busy && !check_running
            && (!frame_preview || self.map_image.placement_preview.is_some());
        let (primary, target, tip, target_ready) = match selected {
            MapImageInstallDestination::Player => (
                "写入玩家背包",
                format!(
                    "玩家：{}",
                    super::super::players::player_id_label(&self.map_image.selected_player)
                ),
                if self.map_image.selected_player.storage_key().is_some() {
                    "地图物品会发放到所选玩家的物品栏或潜影盒，不在世界中建造展示框。"
                } else {
                    "当前玩家存储在 level.dat，不能与 map 记录原子写入；请选择列表中可写入的服务器玩家。"
                },
                self.map_image.selected_player.storage_key().is_some(),
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
                let target = if let Some((x, z)) = self.map_frame_confirmed_center() {
                    format!(
                        "已选择中心：X {x} · Y {} · Z {z}；{}×{} 张地图",
                        self.y_layer, self.map_image.options.columns, self.map_image.options.rows
                    )
                } else {
                    "尚未选择放置位置；不会自动使用地图视口中心".to_owned()
                };
                (
                    "直接放置到当前世界",
                    target,
                    "在地图上左键选择放置中心，预览完整网格范围；然后检查空位与支撑，最后确认写入。",
                    true,
                )
            }
        };
        let status = if busy {
            "写入／导出中，暂时锁定目标与参数"
        } else if pending.is_some() {
            "已暂存 NBT 文件：请检查目标后确认导入。外部文件尚无地图像素预览"
        } else if self.map_image.source.is_none() {
            "第一步：点击中央「选择图片」导入素材，生成真实地图像素"
        } else if converting {
            "正在转换图片，完成后可选择写入目标"
        } else if !ready {
            "地图像素未准备好，请先完成转换或刷新预览"
        } else if !target_ready {
            "目标尚不可写入：请选择可写入的玩家，或在当前维度指定容器"
        } else if needs_target_review {
            "先进入目标预览，再在世界地图上主动选取放置中心"
        } else if needs_position {
            if self.map_image.picking_frame_center {
                "选点模式：请左键点击中央世界地图；不会写入存档"
            } else {
                "第二步：在地图上选择展示框中心；没有选点时不允许预检和写入"
            }
        } else if frame_preview && check_running {
            "正在后台检查展示框网格；不会修改存档"
        } else if frame_preview && !check_passed {
            "先点击「检查放置区域」；有冲突时会显示准确的方块坐标与解决方法"
        } else if frame_preview {
            "已通过只读检查，可直接放置；正式写入前还会再次校验"
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
            .when_some(
                check.filter(|task| task.status.as_ref() == "error")
                    .and_then(|task| task.message.as_deref()),
                |bar, error| {
                    let reason = error.strip_prefix("validation failed: ").unwrap_or(error);
                    bar.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.0))
                            .w_full()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(colors.danger)
                                    .child(format!("无法在此放置：{reason}")),
                            )
                            .child(
                                secondary_button(
                                    colors, "map-frame-raise-and-recheck",
                                    "展示框 Y +1 并重新检查",
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.step_y(1, cx);
                                        this.check_map_frame_grid(cx);
                                    }),
                                ),
                            ),
                    )
                },
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
                        primary_button(
                            colors,
                            "map-import-primary-action",
                            if pending.is_some() {
                                "确认 NBT 导入并写入"
                            } else if needs_target_review {
                                "下一步 · 目标预览"
                            } else if needs_position {
                                if self.map_image.picking_frame_center {
                                    "取消选点模式"
                                } else {
                                    "在地图上选择放置中心"
                                }
                            } else if frame_preview && !check_passed {
                                if check_running { "检查中…" } else { "检查放置区域" }
                            } else {
                                primary
                            },
                        )
                        .opacity(if action_enabled && target_ready { 1.0 } else { 0.45 })
                        .when(action_enabled && target_ready, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    if needs_target_review {
                                        this.set_import_workspace_mode(
                                            super::super::state::ImportWorkspaceMode::Placement, cx
                                        );
                                    } else if needs_position {
                                        if this.map_image.picking_frame_center {
                                            this.cancel_map_frame_center_pick(cx);
                                        } else {
                                            this.begin_map_frame_center_pick(cx);
                                        }
                                    } else if this.map_image.pending_bundle.is_some() {
                                        this.confirm_pending_map_bundle(cx);
                                    } else if this.map_image.install_destination
                                        == MapImageInstallDestination::NewUpFrame
                                        && !this.map_frame_preflight_passed()
                                    {
                                        this.check_map_frame_grid(cx);
                                    } else {
                                        this.install_map_bundle_selected(cx);
                                    }
                                }),
                            )
                        }),
                    )
                    .when(ready && pending.is_none() && !busy, |bar| {
                        bar.child(
                            secondary_button(colors, "map-import-export-bundle", "导出地图包…")
                                .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.export_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(!busy, |bar| {
                        bar.child(
                            secondary_button(
                                colors,
                                "map-import-select-nbt",
                                if pending.is_some() {
                                    "更换 NBT…"
                                } else {
                                    "导入地图包／容器 NBT…"
                                },
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
                            ghost_button(colors, "map-import-cancel-staged-nbt", "取消待导入")
                                .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.cancel_pending_map_bundle(cx)),
                            ),
                        )
                    })
                    .when(
                        self.map_image.source.is_some() && pending.is_none() && !converting && !busy,
                        |bar| {
                            bar.child(ghost_button(colors, "map-import-regenerate-preview", "重新生成")
                                .on_mouse_down(
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

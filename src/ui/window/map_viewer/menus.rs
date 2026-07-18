use super::editor::*;
use super::helpers::*;
use super::layout::top_toolbar_layout;
use super::model::*;
use super::prelude::*;
use super::viewport::coordinate_text;

impl MapViewerWindowView {
    pub(super) fn paste_copied_chunk_context_label(
        &self,
        chunk: ChunkPos,
        rotation: PasteRotation,
    ) -> String {
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            return "粘贴已复制 chunk".to_string();
        };
        let chunk_count = copied_chunk.chunk_count();
        if rotation.is_default() {
            format!("预览粘贴 {chunk_count} 个 chunk")
        } else {
            format!("预览粘贴 {chunk_count} 个 chunk · {}", rotation.label())
        }
    }

    fn paste_rotation_entries(
        &self,
        chunk: ChunkPos,
        cx: &mut Context<Self>,
    ) -> Vec<ContextMenuEntry> {
        let disabled = self.professional.copied_chunk.is_none();
        let expanded = self.ui_state.context_paste_open;
        let entity = cx.entity();
        let mut entries = vec![ContextMenuEntry::item(
            ContextMenuItem::new(if expanded {
                "收起粘贴方向"
            } else {
                "粘贴已复制 chunk..."
            })
            .disabled(disabled)
            .keep_open()
            .on_click(move |cx| entity.update(cx, |this, cx| this.toggle_context_paste(cx))),
        )];
        if expanded {
            entries.extend(PasteRotation::ALL.into_iter().map(|rotation| {
                let entity = cx.entity();
                ContextMenuEntry::item(
                    ContextMenuItem::new(self.paste_copied_chunk_context_label(chunk, rotation))
                        .disabled(disabled)
                        .checked(
                            self.professional
                                .paste_preview
                                .as_ref()
                                .is_some_and(|preview| {
                                    preview.target_anchor == chunk
                                        && preview.transform
                                            == PasteTransform::from_rotation(rotation)
                                }),
                        )
                        .description(rotation.label())
                        .on_click(move |cx| {
                            entity.update(cx, move |this, cx| {
                                this.paste_copied_chunk_to_context(rotation, cx)
                            })
                        }),
                )
            }));
        }
        entries
    }

    fn copy_chunks_context_label(&self) -> &'static str {
        if self.professional.selection.is_some() {
            "复制选中区块"
        } else {
            "复制当前 chunk"
        }
    }

    fn current_chunk_write_entries(
        &self,
        chunk: ChunkPos,
        cx: &mut Context<Self>,
    ) -> Vec<ContextMenuEntry> {
        [
            (
                "删除当前 chunk（清空为空气）",
                QuickWriteAction::DeleteCurrentChunk(chunk),
            ),
            (
                "重置当前 chunk（删除记录重新加载）",
                QuickWriteAction::ResetCurrentChunk(chunk),
            ),
            (
                "删除当前 chunk 方块实体",
                QuickWriteAction::DeleteCurrentChunkBlockEntities(chunk),
            ),
            (
                "删除当前 chunk 实体",
                QuickWriteAction::DeleteCurrentChunkActors(chunk),
            ),
        ]
        .into_iter()
        .map(|(label, action)| {
            let entity = cx.entity();
            ContextMenuEntry::item(
                ContextMenuItem::new(label)
                    .danger(true)
                    .on_click(move |cx| {
                        let action = action.clone();
                        entity.update(cx, move |this, cx| this.run_quick_write_action(action, cx))
                    }),
            )
        })
        .collect()
    }

    fn selection_delete_entry(&self, cx: &mut Context<Self>) -> ContextMenuEntry {
        let entity = cx.entity();
        ContextMenuEntry::item(
            ContextMenuItem::new("删除选区 chunk")
                .danger(true)
                .on_click(move |cx| entity.update(cx, |this, cx| this.delete_selection_chunks(cx))),
        )
    }

    pub(super) fn render_context_menu(
        &self,
        colors: &ThemeColors,
        menu: ContextMenuState,
        cx: &mut Context<Self>,
    ) -> Div {
        let placement = place_context_menu_at_anchor(
            ContextMenuAnchor::Cursor(menu.position),
            self.window_width,
            self.window_height,
            284.0,
            460.0,
        );
        let has_selection = self.professional.selection.is_some();
        let more_items = vec![
            ContextMenuItem::new("编辑 HSA 生成区").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_hsa_editor(cx))
            }),
            ContextMenuItem::new("查看/编辑方块实体").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_block_entities_editor(cx))
            }),
            ContextMenuItem::new("编辑当前位置方块实体").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_block_entity_at_editor(cx))
            }),
            ContextMenuItem::new("查看/编辑实体 Actors").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_actors_editor(cx))
            }),
            ContextMenuItem::new("查看/编辑高度图").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_heightmap_editor(cx))
            }),
            ContextMenuItem::new("查看/编辑生物群系").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_biome_storage_editor(cx))
            }),
        ];
        let more_open = self.ui_state.context_more_open;
        let more_edit_entries = context_more_edit_entries(more_open, more_items, {
            let entity = cx.entity();
            move |cx| entity.update(cx, |this, cx| this.toggle_context_more(cx))
        });
        let selection_entries = if has_selection {
            vec![
                ContextMenuEntry::item(ContextMenuItem::new("统计当前选区").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.query_selection_stats(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("导出选中区块 OBJ").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.export_selection_as_obj(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("导出跨地图区域包").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.export_selection_region_package(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("导出选区 .mcstructure").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.export_selection_mcstructure(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("导出选中区块图片").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.export_chunks_image(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("加载/刷新 3D 预览").on_click({
                    let entity = cx.entity();
                    move |cx| {
                        entity.update(cx, |this, cx| {
                            this.show_right_preview_3d_panel(cx);
                            this.refresh_preview_3d(cx);
                        })
                    }
                })),
                ContextMenuEntry::item(ContextMenuItem::new("清除选区").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.clear_professional_selection(cx))
                })),
            ]
        } else {
            vec![
                ContextMenuEntry::item(ContextMenuItem::new("设为选区起点").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.set_context_selection_start(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("设为选区终点").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.set_context_selection_end(cx))
                })),
            ]
        };
        let mut groups = vec![
            ContextMenuGroup::new(vec![
                ContextMenuEntry::item(
                    ContextMenuItem::new(format!("复制 /tp {} ~ {}", menu.block_x, menu.block_z))
                        .on_click({
                            let entity = cx.entity();
                            move |cx| entity.update(cx, |this, cx| this.copy_context_tp(cx))
                        }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(self.copy_chunks_context_label()).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.copy_context_chunks(cx))
                    }),
                ),
                ContextMenuEntry::item(ContextMenuItem::new("在此处添加红点标记").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.add_context_marker(cx))
                })),
                ContextMenuEntry::item(ContextMenuItem::new("清除当前维度标记").on_click({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.clear_dimension_markers(cx))
                })),
            ]),
            ContextMenuGroup::titled(
                "查询",
                vec![
                    ContextMenuEntry::item(ContextMenuItem::new("查询此方块").on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.query_context_block(cx))
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("打开 chunk 详情").on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.open_context_chunk_detail(cx))
                    })),
                ],
            ),
            ContextMenuGroup::titled("选区", selection_entries),
            ContextMenuGroup::titled("编辑", more_edit_entries),
        ];
        {
            let Some(chunk) = self.context_chunk_pos() else {
                return div().child(
                    ContextMenu::new(colors, groups)
                        .header(coordinate_text(menu.block_x, menu.block_z))
                        .placement(placement)
                        .on_dismiss({
                            let entity = cx.entity();
                            move |cx| entity.update(cx, |this, cx| this.close_context_menu(cx))
                        }),
                );
            };
            let mut write_entries = self.paste_rotation_entries(chunk, cx);
            if write_targets_selection(self.professional.selection) {
                write_entries.push(self.selection_delete_entry(cx));
            } else {
                write_entries.extend(self.current_chunk_write_entries(chunk, cx));
            }
            groups.push(ContextMenuGroup::titled("写入", write_entries));
        }
        groups.push(ContextMenuGroup::new(vec![ContextMenuEntry::item(
            ContextMenuItem::new("关闭菜单").on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.close_context_menu(cx))
            }),
        )]));

        div().child(
            ContextMenu::new(colors, groups)
                .header(coordinate_text(menu.block_x, menu.block_z))
                .placement(placement)
                .on_dismiss({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.close_context_menu(cx))
                }),
        )
    }

    pub(super) fn render_top_more_menu(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let placement = place_context_menu_at_anchor(
            ContextMenuAnchor::RectEdge(Bounds::new(
                point(px((self.window_width - 116.0).max(8.0)), px(8.0)),
                size(px(92.0), px(38.0)),
            )),
            self.window_width,
            self.window_height,
            284.0,
            520.0,
        );
        let toolbar_layout = top_toolbar_layout(self.window_width);
        let mut groups = Vec::new();
        let mut navigation_entries = Vec::new();
        if !toolbar_layout.show_modes {
            for (mode, label) in [
                (ViewerMode::Surface, "模式：地形"),
                (ViewerMode::Biome, "模式：群系"),
                (ViewerMode::Height, "模式：高度"),
                (ViewerMode::Layer, "模式：Y层"),
                (ViewerMode::Cave, "模式：洞穴"),
            ] {
                let entity = cx.entity();
                navigation_entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(label)
                        .checked(self.mode == mode)
                        .on_click(move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.set_mode(mode, cx);
                            })
                        }),
                ));
            }
        }
        if !toolbar_layout.show_y_controls {
            for (delta, label) in [(-1, "Y 层 -"), (1, "Y 层 +")] {
                let entity = cx.entity();
                navigation_entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(label).on_click(move |cx| {
                        entity.update(cx, |this, cx| {
                            this.close_top_more();
                            this.step_y(delta, cx);
                        })
                    }),
                ));
            }
        }
        if !toolbar_layout.show_zoom_controls {
            for (factor, label) in [(0.87, "缩小"), (1.15, "放大")] {
                let entity = cx.entity();
                navigation_entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(label).on_click(move |cx| {
                        entity.update(cx, |this, cx| {
                            this.close_top_more();
                            this.zoom_by_center(factor, cx);
                        })
                    }),
                ));
            }
        }
        if !navigation_entries.is_empty() {
            groups.push(ContextMenuGroup::titled("地图导航", navigation_entries));
        }
        groups.extend([
            ContextMenuGroup::titled(
                "视图",
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new("左侧工具栏")
                            .checked(self.ui_state.left_panel_open)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_left_panel(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("底部区块树/诊断")
                            .checked(self.ui_state.bottom_panel_open)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_bottom_panel(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("右侧 NBT 编辑")
                            .checked(
                                self.ui_state.right_panel_open
                                    && self.ui_state.active_right_panel == MapViewerRightPanel::Nbt,
                            )
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_right_panel_kind(MapViewerRightPanel::Nbt, cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("右侧 3D 预览")
                            .checked(
                                self.ui_state.right_panel_open
                                    && self.ui_state.active_right_panel
                                        == MapViewerRightPanel::Preview3d,
                            )
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_right_panel_kind(
                                            MapViewerRightPanel::Preview3d,
                                            cx,
                                        );
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(ContextMenuItem::new("区块树").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.toggle_bottom_tab(MapViewerBottomTab::ChunkTree, cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("详情").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.toggle_bottom_tab(MapViewerBottomTab::Details, cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("诊断").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.toggle_bottom_tab(MapViewerBottomTab::Diagnostics, cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("历史").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.toggle_bottom_tab(MapViewerBottomTab::History, cx);
                            })
                        }
                    })),
                ],
            ),
            ContextMenuGroup::titled(
                "历史",
                vec![
                    ContextMenuEntry::item(ContextMenuItem::new("撤回地图修改").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.undo_map_edit(cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("重做地图修改").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.redo_map_edit(cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("创建整图备份").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.create_map_backup(cx);
                            })
                        }
                    })),
                ],
            ),
            ContextMenuGroup::titled(
                "渲染",
                vec![
                    ContextMenuEntry::item(ContextMenuItem::new("绕过缓存重绘").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.redraw_bypassing_cache(cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(render_backend_label(
                            self.render_backend,
                            self.render_gpu_backend,
                        ))
                        .description("切换 Auto / GPU / CPU")
                        .on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_render_backend(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(ContextMenuItem::new("回出生点").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.recenter_to_spawn(cx);
                            })
                        }
                    })),
                ],
            ),
            ContextMenuGroup::titled(
                "叠加层",
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new("坐标轴")
                            .checked(self.overlay_options.axis)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_axis(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("密网格")
                            .checked(self.overlay_options.dense_grid)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_dense_grid(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("标尺")
                            .checked(self.overlay_options.ruler)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_ruler(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("史莱姆区块")
                            .checked(self.overlay_options.slime_chunks)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_slime_overlay(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("实体")
                            .checked(self.overlay_options.entities)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_entity_overlay(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("方块实体")
                            .checked(self.overlay_options.block_entities)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_block_entity_overlay(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("村庄")
                            .checked(self.overlay_options.villages)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_village_overlay(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("HSA 生成区")
                            .checked(self.overlay_options.hardcoded_spawn_areas)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_hsa_overlay(cx);
                                    })
                                }
                            }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                "编辑",
                vec![
                    ContextMenuEntry::item(ContextMenuItem::new("导入区域/结构文件").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.open_import_structure_dialog(cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("导出选区 .mcstructure").on_click(
                        {
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.export_selection_mcstructure(cx);
                                })
                            }
                        },
                    )),
                    ContextMenuEntry::item(ContextMenuItem::new("统计选区").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.query_selection_stats(cx);
                            })
                        }
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("清除选区").on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.close_top_more();
                                this.clear_professional_selection(cx);
                            })
                        }
                    })),
                ],
            ),
        ]);

        div().child(
            ContextMenu::new(colors, groups)
                .header("地图编辑器")
                .placement(placement),
        )
    }
}

fn write_targets_selection(selection: Option<ChunkSelection>) -> bool {
    selection.is_some_and(|selection| selection.chunk_count() > 1)
}

#[cfg(test)]
mod context_write_scope_tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn multi_chunk_selection_replaces_current_chunk_write_actions() {
        let single = ChunkSelection {
            start: ChunkPos {
                x: 1,
                z: 2,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: 1,
                z: 2,
                dimension: Dimension::Overworld,
            },
        };
        let multiple = ChunkSelection {
            end: ChunkPos { x: 3, ..single.end },
            ..single
        };

        assert!(!write_targets_selection(None));
        assert!(!write_targets_selection(Some(single)));
        assert!(write_targets_selection(Some(multiple)));
    }
}

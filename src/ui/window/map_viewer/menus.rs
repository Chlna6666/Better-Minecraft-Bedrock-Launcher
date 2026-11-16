use super::editor::*;
use super::layout::top_toolbar_layout;
use super::model::*;
use super::prelude::*;
use super::render_support::backend_label;
use super::viewport::coordinate_text;
use super::world3d::types::TeleportHeightMode;

impl View {
    pub(super) fn paste_copied_chunk_context_label(
        &self,
        _chunk: ChunkPos,
        rotation: PasteRotation,
    ) -> String {
        let Some(copied_chunk) = self.professional.copied_chunk.as_ref() else {
            return t!("MapViewer.menu.paste_copied_chunk").to_string();
        };
        let chunk_count = copied_chunk.chunk_count();
        if rotation.is_default() {
            t!("MapViewer.menu.preview_paste", count = chunk_count).to_string()
        } else {
            t!(
                "MapViewer.menu.preview_paste_rotation",
                count = chunk_count,
                rotation = rotation.label()
            )
            .to_string()
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
                t!("MapViewer.menu.collapse_paste_direction")
            } else {
                t!("MapViewer.menu.paste_copied_chunk_ellipsis")
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

    fn copy_chunks_context_label(&self) -> SharedString {
        if self.professional.selection.is_some() {
            t!("MapViewer.menu.copy_selected_chunks")
        } else {
            t!("MapViewer.menu.copy_current_chunk")
        }
    }

    fn current_chunk_write_entries(
        &self,
        chunk: ChunkPos,
        cx: &mut Context<Self>,
    ) -> Vec<ContextMenuEntry> {
        [
            (
                t!("MapViewer.menu.delete_current_chunk"),
                QuickWriteAction::DeleteCurrentChunk(chunk),
            ),
            (
                t!("MapViewer.menu.reset_current_chunk"),
                QuickWriteAction::ResetCurrentChunk(chunk),
            ),
            (
                t!("MapViewer.menu.delete_current_chunk_block_entities"),
                QuickWriteAction::DeleteCurrentChunkBlockEntities(chunk),
            ),
            (
                t!("MapViewer.menu.delete_current_chunk_actors"),
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
            ContextMenuItem::new(t!("MapViewer.menu.delete_selection_chunks"))
                .danger(true)
                .on_click(move |cx| entity.update(cx, |this, cx| this.delete_selection_chunks(cx))),
        )
    }

    fn world3d_context_entries(
        &self,
        block_x: i32,
        block_z: i32,
        cx: &mut Context<Self>,
    ) -> Vec<ContextMenuEntry> {
        let editor_open = self.editor_is_open();
        let dimension = super::world3d::types::dimension_label(self.dimension);
        let y_layer = self.y_layer;
        let description = format!("{dimension} · X {block_x} / Z {block_z}");
        let mut entries = vec![
            ContextMenuEntry::item(
                ContextMenuItem::new(if editor_open {
                    t!("MapViewer.menu.teleport_here_surface")
                } else {
                    t!("MapViewer.menu.open_3d_and_teleport_here")
                })
                .description(description.clone())
                .on_click({
                    let entity = cx.entity();
                    move |cx| {
                        entity.update(cx, |this, cx| {
                            this.close_context_menu(cx);
                            this.teleport_world3d_editor(
                                block_x,
                                block_z,
                                TeleportHeightMode::SurfaceSafe,
                                None,
                                cx,
                            );
                        })
                    }
                }),
            ),
            ContextMenuEntry::item(
                ContextMenuItem::new(
                    t!("MapViewer.menu.teleport_current_layer", layer = y_layer).to_string(),
                )
                .description(description)
                .on_click({
                    let entity = cx.entity();
                    move |cx| {
                        entity.update(cx, |this, cx| {
                            this.close_context_menu(cx);
                            this.teleport_world3d_editor(
                                block_x,
                                block_z,
                                TeleportHeightMode::CurrentMapLayer,
                                Some(f64::from(y_layer) + 1.6),
                                cx,
                            );
                        })
                    }
                }),
            ),
        ];
        if editor_open {
            let entity = cx.entity();
            entries.push(ContextMenuEntry::item(
                ContextMenuItem::new(t!(
                    "MapViewer.menu.camera_overlay",
                    mode = self.overlay_mode_label()
                ))
                .description(t!("MapViewer.menu.camera_overlay_desc"))
                .on_click(move |cx| {
                    entity.update(cx, |this, cx| {
                        this.close_context_menu(cx);
                        this.cycle_camera_overlay_mode(cx);
                    })
                }),
            ));
            let entity = cx.entity();
            entries.push(ContextMenuEntry::item(
                ContextMenuItem::new(if self.world3d.follow_3d {
                    t!("MapViewer.menu.stop_follow_3d")
                } else {
                    t!("MapViewer.menu.follow_3d")
                })
                .checked(self.world3d.follow_3d)
                .on_click(move |cx| {
                    entity.update(cx, |this, cx| {
                        this.close_context_menu(cx);
                        this.toggle_world3d_follow(cx);
                    })
                }),
            ));
            let entity = cx.entity();
            entries.push(ContextMenuEntry::item(
                ContextMenuItem::new(t!("MapViewer.menu.locate_3d_camera")).on_click(move |cx| {
                    entity.update(cx, |this, cx| {
                        this.close_context_menu(cx);
                        if let Some(camera) = this.world3d.camera {
                            this.center_viewport_on_block(
                                camera.position[0].floor() as i32,
                                camera.position[2].floor() as i32,
                                cx,
                            );
                            if camera.dimension == this.dimension {
                                this.y_layer = camera.position[1].floor() as i32;
                            }
                        }
                    })
                }),
            ));
        } else {
            let entity = cx.entity();
            entries.push(ContextMenuEntry::item(
                ContextMenuItem::new(t!("MapViewer.menu.open_3d_editor")).on_click(move |cx| {
                    entity.update(cx, |this, cx| {
                        this.close_context_menu(cx);
                        this.open_editor_at_map_center(cx);
                    })
                }),
            ));
        }
        entries
    }

    pub(super) fn render_context_menu(
        &self,
        colors: &ThemeColors,
        menu: ContextMenuState,
        cx: &mut Context<Self>,
    ) -> Div {
        let placement = place_at_anchor(
            ContextMenuAnchor::Cursor(menu.position),
            self.window_width,
            self.window_height,
            284.0,
            460.0,
        );
        let has_selection = self.professional.selection.is_some();
        let more_items = vec![
            ContextMenuItem::new(t!("MapViewer.menu.edit_hsa")).on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_hsa_editor(cx))
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_block_entities")).on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_block_entities_editor(cx))
            }),
            ContextMenuItem::new(if self.map_image_selecting_block() {
                t!("MapViewer.menu.select_map_write_target")
            } else {
                t!("MapViewer.menu.edit_block_entity_here")
            })
            .on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_block_entity_editor(cx))
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_actors")).on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_actors_editor(cx))
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_heightmap")).on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_heightmap_editor(cx))
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_biomes")).on_click({
                let entity = cx.entity();
                move |cx| entity.update(cx, |this, cx| this.open_context_biome_storage_editor(cx))
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_tick_queues")).on_click({
                let entity = cx.entity();
                move |cx| {
                    entity.update(cx, |this, cx| {
                        let Some(menu) = this.context_menu else {
                            return;
                        };
                        let chunk = context_menu_chunk(menu, this.dimension);
                        this.load_edit_detail(
                            EditTarget::TickQueues {
                                chunk,
                                pending: None,
                                random: None,
                            },
                            cx,
                        );
                    })
                }
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_mob_events")).on_click({
                let entity = cx.entity();
                move |cx| {
                    entity.update(cx, |this, cx| {
                        this.load_edit_detail(
                            EditTarget::GlobalRecord {
                                kind: GlobalRecordKind::MobEvents,
                                source: None,
                            },
                            cx,
                        );
                    })
                }
            }),
            ContextMenuItem::new(t!("MapViewer.menu.edit_scoreboard")).on_click({
                let entity = cx.entity();
                move |cx| {
                    entity.update(cx, |this, cx| {
                        this.load_edit_detail(
                            EditTarget::GlobalRecord {
                                kind: GlobalRecordKind::Scoreboard,
                                source: None,
                            },
                            cx,
                        );
                    })
                }
            }),
        ];
        // While picking a map destination, put the action in the first menu group,
        // not in the generic nested editor menu.
        let mut more_items = more_items;
        if self.map_image_selecting_block() {
            more_items.remove(2);
        }
        let more_open = self.ui_state.context_more_open;
        let more_edit_entries = context_more_edit_entries(more_open, more_items, {
            let entity = cx.entity();
            move |cx| entity.update(cx, |this, cx| this.toggle_context_more(cx))
        });
        let selection_entries = if has_selection {
            vec![
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.stats_selection")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.exact_selection_stats(cx))
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.export_selection_3d_model")).on_click(
                        {
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| this.export_exact_selection_model(cx))
                            }
                        },
                    ),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.export_cross_map_region")).on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| this.export_selection_region_package(cx))
                        }
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.export_selection_mcstructure"))
                        .on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| this.export_selection_mcstructure(cx))
                            }
                        }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.export_selection_image")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.export_chunks_image(cx))
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.load_3d_preview")).on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| {
                                this.show_right_preview_3d_panel(cx);
                                this.refresh_preview_3d_exact(cx);
                            })
                        }
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.clear_selection")).on_click({
                        let entity = cx.entity();
                        move |cx| {
                            entity.update(cx, |this, cx| this.clear_professional_selection(cx))
                        }
                    }),
                ),
            ]
        } else {
            vec![
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.set_selection_start")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.set_context_selection_start(cx))
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.set_selection_end")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.set_context_selection_end(cx))
                    }),
                ),
            ]
        };
        let mut groups = vec![
            ContextMenuGroup::new(vec![
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!(
                        "MapViewer.menu.copy_tp_command",
                        x = menu.block_x,
                        z = menu.block_z
                    ))
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
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.add_red_marker")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.add_context_marker(cx))
                    }),
                ),
                ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.clear_dimension_markers")).on_click({
                        let entity = cx.entity();
                        move |cx| entity.update(cx, |this, cx| this.clear_dimension_markers(cx))
                    }),
                ),
            ]),
            ContextMenuGroup::titled(
                t!("MapViewer.menu.query"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.query_block")).on_click({
                            let entity = cx.entity();
                            move |cx| entity.update(cx, |this, cx| this.query_context_block(cx))
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.open_chunk_details")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| this.open_context_chunk_detail(cx))
                            }
                        }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(t!("MapViewer.menu.group_selection"), selection_entries),
            ContextMenuGroup::titled(t!("MapViewer.menu.group_edit"), more_edit_entries),
        ];
        if self.map_image_selecting_block() {
            groups.insert(
                0,
                ContextMenuGroup::titled(
                    t!("MapViewer.menu.map_item_current_target"),
                    vec![ContextMenuEntry::item(
                        ContextMenuItem::new(t!(
                            "MapViewer.menu.set_map_write_target_at",
                            x = menu.block_x,
                            y = self.y_layer,
                            z = menu.block_z
                        ))
                        .on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.open_context_block_entity_editor(cx)
                                })
                            }
                        }),
                    )],
                ),
            );
        } else if self.map_image_positioning_frames() {
            groups.insert(
                0,
                ContextMenuGroup::titled(
                    t!("MapViewer.menu.map_item_place_center"),
                    vec![ContextMenuEntry::item(
                        ContextMenuItem::new(t!(
                            "MapViewer.menu.set_frame_center",
                            x = menu.block_x,
                            z = menu.block_z
                        ))
                        .on_click({
                            let entity = cx.entity();
                            move |cx| entity.update(cx, |this, cx| this.select_map_frame_center(cx))
                        }),
                    )],
                ),
            );
        }
        if let Some(actor) = self.professional.entity_context_target.clone() {
            let identifier = actor
                .identifier
                .clone()
                .unwrap_or_else(|| "minecraft:unknown".to_string());
            let edit_target = actor.unique_id.map(|unique_id| EditTarget::Actor {
                chunk: actor.source_chunk,
                unique_id,
            });
            let mut entries = Vec::new();
            #[cfg(debug_assertions)]
            {
                let normalized_key = actor
                    .identifier
                    .as_deref()
                    .and_then(normalize_entity_avatar_key);
                let resolved_key = actor.identifier.as_deref().and_then(|identifier| {
                    entity_avatar_key_in_pool(identifier, &self.professional.entity_avatar_pool)
                });
                let avatar_status = if let Some(key) = resolved_key {
                    t!("MapViewer.menu.avatar_matched", key = key).to_string()
                } else if actor.identifier.is_none() {
                    t!("MapViewer.menu.nbt_missing_entity_id").to_string()
                } else if self.professional.entity_avatar_pool.is_empty() {
                    t!("MapViewer.menu.avatar_pool_loading").to_string()
                } else {
                    t!("MapViewer.menu.avatar_unmatched").to_string()
                };
                let cache_status = entity_cache_status_label(actor.cache_status);
                let freshness = entity_cache_freshness_label(actor.cache_status);
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new("DEBUG 实体解析信息")
                        .description(format!(
                            "{avatar_status} · 原始 ID {:?} · 规范键 {} · UID {:?} · Pos [{:.3}, {:.3}, {:.3}] · 来源 chunk [{}, {}, {:?}] · 实体数据 {cache_status} · 缓存瓦片 {}/{} · 重建 {} · 指纹已校验 {} · 时效策略：{freshness}",
                            actor.identifier.as_deref(),
                            normalized_key.as_deref().unwrap_or("<none>"),
                            actor.unique_id,
                            actor.position[0],
                            actor.position[1],
                            actor.position[2],
                            actor.source_chunk.x,
                            actor.source_chunk.z,
                            actor.source_chunk.dimension,
                            actor.cache_summary.cached_tile_count,
                            actor.cache_summary.requested_tile_count,
                            actor.cache_summary.rebuilt_tile_count,
                            actor.cache_summary.source_fingerprints_validated,
                        ))
                        .disabled(true),
                ));
            }
            if let Some(target) = edit_target.clone() {
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.edit_entity_nbt"))
                        .description(format!(
                            "{identifier} · UniqueID {}",
                            actor.unique_id.unwrap_or_default()
                        ))
                        .on_click(move |cx| {
                            let target = target.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.load_edit_detail(target, cx);
                            })
                        }),
                ));
                let target = edit_target.expect("entity edit target");
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.delete_entity"))
                        .danger(true)
                        .description(t!("MapViewer.menu.delete_entity_desc"))
                        .on_click(move |cx| {
                            let target = target.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.confirm_or_run_edit(target, EditAction::Delete, cx);
                            })
                        }),
                ));
            } else {
                let entity = cx.entity();
                entries.push(ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.view_chunk_actors"))
                        .description(t!("MapViewer.menu.missing_unique_id_desc"))
                        .on_click(move |cx| {
                            let chunk = actor.source_chunk;
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.professional.entity_context_target = None;
                                this.load_edit_detail(EditTarget::Actors(chunk), cx);
                            })
                        }),
                ));
            }
            groups.insert(0, ContextMenuGroup::titled(identifier, entries));
        }
        if let Some(player_id) = self.players.context_target.clone() {
            let entity = cx.entity();
            groups.insert(
                0,
                ContextMenuGroup::new(vec![ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.menu.edit_player"))
                        .description(t!("MapViewer.menu.edit_player_desc"))
                        .on_click(move |cx| {
                            let player_id = player_id.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.players.context_target = None;
                                this.open_player_workspace(
                                    player_id,
                                    PlayerWorkspaceCenter::Inventory,
                                    cx,
                                );
                            })
                        }),
                )]),
            );
        }
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
            groups.push(ContextMenuGroup::titled(
                t!("MapViewer.menu.group_write"),
                write_entries,
            ));
        }
        groups.push(ContextMenuGroup::titled(
            t!("MapViewer.menu.group_3d_editor"),
            self.world3d_context_entries(menu.block_x, menu.block_z, cx),
        ));
        groups.push(ContextMenuGroup::new(vec![ContextMenuEntry::item(
            ContextMenuItem::new(t!("MapViewer.menu.close_menu")).on_click({
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
        let placement = place_at_anchor(
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
                (ViewerMode::Surface, t!("MapViewer.menu.mode_surface")),
                (ViewerMode::Biome, t!("MapViewer.menu.mode_biome")),
                (ViewerMode::Height, t!("MapViewer.menu.mode_height")),
                (ViewerMode::Layer, t!("MapViewer.menu.mode_layer")),
                (ViewerMode::Cave, t!("MapViewer.menu.mode_cave")),
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
            for (delta, label) in [
                (-1, t!("MapViewer.menu.layer_minus")),
                (1, t!("MapViewer.menu.layer_plus")),
            ] {
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
            for (factor, label) in [
                (0.87, t!("MapViewer.menu.zoom_out")),
                (1.15, t!("MapViewer.menu.zoom_in")),
            ] {
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
            groups.push(ContextMenuGroup::titled(
                t!("MapViewer.menu.group_map_navigation"),
                navigation_entries,
            ));
        }
        groups.extend([
            ContextMenuGroup::titled(
                t!("MapViewer.menu.group_view"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.toggle_left_toolbar"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.toggle_bottom_chunk_tree"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.toggle_right_nbt"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.toggle_right_3d_preview"))
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
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.chunk_tree")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_bottom_tab(MapViewerBottomTab::ChunkTree, cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.details")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_bottom_tab(MapViewerBottomTab::Details, cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.diagnostics")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_bottom_tab(MapViewerBottomTab::Diagnostics, cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(ContextMenuItem::new(t!("MapViewer.history")).on_click(
                        {
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_bottom_tab(MapViewerBottomTab::History, cx);
                                })
                            }
                        },
                    )),
                ],
            ),
            ContextMenuGroup::titled(
                t!("MapViewer.menu.group_3d_editor"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.open_focus_3d_editor"))
                            .description(t!("MapViewer.menu.standalone_window_desc"))
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.show_world3d_editor_window(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!(
                            "MapViewer.menu.camera_overlay",
                            mode = self.overlay_mode_label()
                        ))
                        .on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.cycle_camera_overlay_mode(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(if self.world3d.follow_3d {
                            t!("MapViewer.menu.stop_follow_3d")
                        } else {
                            t!("MapViewer.menu.follow_3d")
                        })
                        .checked(self.world3d.follow_3d)
                        .on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.toggle_world3d_follow(cx);
                                })
                            }
                        }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                t!("MapViewer.menu.group_history"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.undo_map_edit")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.undo_map_edit(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.redo_map_edit")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.redo_map_edit(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.create_full_backup")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.create_map_backup(cx);
                                })
                            }
                        }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                t!("MapViewer.menu.group_render"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.redraw_bypass_cache")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.redraw_bypassing_cache(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(backend_label(
                            self.render_backend,
                            self.render_gpu_backend,
                        ))
                        .description(t!("MapViewer.menu.backend_switch_desc"))
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
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.back_to_spawn")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.recenter_to_spawn(cx);
                                })
                            }
                        }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                t!("MapViewer.menu.group_overlays"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.axis"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.grid"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.ruler"))
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
                        ContextMenuItem::new(t!("MapViewer.slime_analysis"))
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
                        ContextMenuItem::new(t!("MapViewer.load_risk"))
                            .checked(self.overlay_options.load_risk)
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.toggle_load_risk_overlay(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.risk_entities"))
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
                        ContextMenuItem::new(t!("MapViewer.block_entity_overlay"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.villages"))
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
                        ContextMenuItem::new(t!("MapViewer.menu.hsa_regions"))
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
                t!("MapViewer.menu.group_edit"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.import_region_file")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.open_import_structure_dialog(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.menu.export_selection_mcstructure"))
                            .on_click({
                                let entity = cx.entity();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.close_top_more();
                                        this.export_selection_mcstructure(cx);
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.selection_stats")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.exact_selection_stats(cx);
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.clear_selection")).on_click({
                            let entity = cx.entity();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.close_top_more();
                                    this.clear_professional_selection(cx);
                                })
                            }
                        }),
                    ),
                ],
            ),
        ]);

        div().child(
            ContextMenu::new(colors, groups)
                .header(t!("MapViewer.menu.map_editor_header"))
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

pub(super) fn context_menu_chunk(menu: ContextMenuState, dimension: Dimension) -> ChunkPos {
    chunk_from_block(menu.block_x, menu.block_z, dimension)
}

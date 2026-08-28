use super::model::*;
use super::player_workspace::{
    PlayerInspectorMode, PlayerItemSelection, inventory_kind_capacity, parse_workspace_item_import,
    set_workspace_item_slot,
};
use super::players::{PlayerInventoryKind, player_id_label, player_inventory_entries};
use super::prelude::*;
use std::fs;

#[derive(Clone, Copy, Debug)]
pub(super) struct PlayerItemContextMenuState {
    pub(super) selection: PlayerItemSelection,
    pub(super) position: Point<Pixels>,
}

impl MapViewerWindowView {
    pub(super) fn open_player_item_context_menu(
        &mut self,
        selection: PlayerItemSelection,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.players.context_target = None;
        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;
        self.ui_state.context_paste_open = false;
        self.player_workspace.item_context_menu = Some(PlayerItemContextMenuState {
            selection,
            position,
        });
        self.player_workspace.item_context_copy_open = false;
        self.player_workspace.selected_item = Some(selection);
        self.player_workspace.inspector_mode = PlayerInspectorMode::Visual;
        self.player_workspace.item_editor_error = None;
        self.sync_selected_player_item_visual_inputs(cx);
        self.sync_player_item_raw_editor(cx);

        let dock_changed = !self.ui_state.right_panel_open
            || self.ui_state.active_right_panel != MapViewerRightPanel::Player;
        self.ui_state.active_right_panel = MapViewerRightPanel::Player;
        self.ui_state.set_right_panel_open(true);
        if dock_changed {
            self.update_viewport_after_dock_change(cx);
        }
        cx.notify();
    }

    pub(super) fn close_player_item_context_menu(&mut self, cx: &mut Context<Self>) {
        let changed = self.player_workspace.item_context_menu.take().is_some()
            || self.player_workspace.item_context_copy_open;
        self.player_workspace.item_context_copy_open = false;
        if changed {
            cx.notify();
        }
    }

    fn toggle_player_item_copy_menu(&mut self, cx: &mut Context<Self>) {
        self.player_workspace.item_context_copy_open =
            !self.player_workspace.item_context_copy_open;
        cx.notify();
    }

    fn player_item_config_json(&self, selection: PlayerItemSelection) -> Result<String, String> {
        let entry = self
            .workspace_entry_for_selection(selection)
            .ok_or_else(|| "该槽位没有物品".to_string())?;
        let item = serde_json::to_value(&entry.item.nbt).map_err(|error| error.to_string())?;
        serde_json::to_string_pretty(&serde_json::json!({
            "format": "bmcbl.player-item.v1",
            "container": selection.kind.label(),
            "container_key": selection.kind.nbt_key(),
            "slot": selection.slot,
            "id": entry.item.name,
            "item": item,
        }))
        .map_err(|error| error.to_string())
    }

    fn copy_player_item_config(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.player_item_config_json(selection) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = SharedString::from("物品配置已复制 · 可直接右键其他槽位粘贴");
                cx.notify();
                true
            }
            Err(error) => {
                self.status = SharedString::from(error);
                cx.notify();
                false
            }
        }
    }

    fn copy_player_item_nbt(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        let Some(entry) = self.workspace_entry_for_selection(selection) else {
            self.status = SharedString::from("该槽位没有物品");
            cx.notify();
            return;
        };
        match serde_json::to_string_pretty(&entry.item.nbt) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = SharedString::from("原始物品 NBT JSON 已复制");
            }
            Err(error) => self.status = SharedString::from(format!("序列化物品 NBT 失败: {error}")),
        }
        cx.notify();
    }

    fn copy_player_item_id(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        let Some(id) = self
            .workspace_entry_for_selection(selection)
            .and_then(|entry| entry.item.name)
        else {
            self.status = SharedString::from("该槽位没有可复制的物品 ID");
            cx.notify();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(id));
        self.status = SharedString::from("物品 ID 已复制");
        cx.notify();
    }

    fn cut_player_item(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        if self.copy_player_item_config(selection, cx) {
            self.player_workspace.selected_item = Some(selection);
            self.write_player_workspace_slot(selection, None, "玩家物品：剪切", cx);
        }
    }

    fn paste_player_item_context(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
        self.player_workspace.selected_item = Some(selection);
        self.import_selected_player_item_from_clipboard(cx);
    }

    fn import_player_item_file(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        let Some(path) = pick_file_path_with_filter("BMCBL 物品配置", &["json"]) else {
            return;
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                self.status = SharedString::from(format!("读取物品配置失败: {error}"));
                cx.notify();
                return;
            }
        };
        let mut tag = match parse_workspace_item_import(&text, selection.slot) {
            Ok(tag) => tag,
            Err(error) => {
                self.player_workspace.item_editor_error = Some(SharedString::from(error));
                cx.notify();
                return;
            }
        };
        if let Err(error) = set_workspace_item_slot(&mut tag, selection.slot) {
            self.player_workspace.item_editor_error = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        self.player_workspace.selected_item = Some(selection);
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：配置文件导入", cx);
    }

    fn export_player_item_file(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        let text = match self.player_item_config_json(selection) {
            Ok(text) => text,
            Err(error) => {
                self.status = SharedString::from(error);
                cx.notify();
                return;
            }
        };
        let id = self
            .workspace_entry_for_selection(selection)
            .and_then(|entry| entry.item.name)
            .unwrap_or_else(|| "item".to_string());
        let safe = safe_export_stem(&id);
        let Some(path) = pick_save_path_with_filter(
            "BMCBL 物品配置",
            &["json"],
            &format!("{safe}_slot_{}.json", selection.slot),
        ) else {
            return;
        };
        match fs::write(&path, text) {
            Ok(()) => self.status = SharedString::from(format!("物品配置已导出：{path}")),
            Err(error) => self.status = SharedString::from(format!("导出物品配置失败: {error}")),
        }
        cx.notify();
    }

    fn duplicate_player_item_to_free_slot(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.workspace_entry_for_selection(selection) else {
            self.status = SharedString::from("该槽位没有可复制的物品");
            cx.notify();
            return;
        };
        let Some(detail) = self.players.detail.as_ref() else {
            return;
        };
        let capacity = inventory_kind_capacity(selection.kind);
        let occupied = player_inventory_entries(&detail.nbt)
            .into_iter()
            .filter(|item| item.kind == selection.kind)
            .map(|item| item.slot.unwrap_or(item.list_index as i32))
            .collect::<BTreeSet<_>>();
        let Some(slot) = (1..=capacity)
            .map(|offset| (selection.slot + offset).rem_euclid(capacity))
            .find(|slot| !occupied.contains(slot))
        else {
            self.status = SharedString::from(format!("{}没有空槽位", selection.kind.label()));
            cx.notify();
            return;
        };
        let target = PlayerItemSelection {
            kind: selection.kind,
            list_index: None,
            slot,
        };
        let mut tag = entry.item.nbt;
        if let Err(error) = set_workspace_item_slot(&mut tag, slot) {
            self.status = SharedString::from(error);
            cx.notify();
            return;
        }
        self.player_workspace.selected_item = Some(target);
        self.write_player_workspace_slot(target, Some(tag), "玩家物品：复制到空槽", cx);
    }

    fn toggle_context_multi_select(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self
            .player_workspace
            .multi_selected_items
            .iter()
            .position(|item| *item == selection)
        {
            self.player_workspace.multi_selected_items.remove(index);
        } else if self.workspace_entry_for_selection(selection).is_some() {
            self.player_workspace.multi_selected_items.push(selection);
        }
        self.status = SharedString::from(format!(
            "物品多选：{} 项",
            self.player_workspace.multi_selected_items.len()
        ));
        cx.notify();
    }

    fn clear_context_item(&mut self, selection: PlayerItemSelection, cx: &mut Context<Self>) {
        self.player_workspace.selected_item = Some(selection);
        self.write_player_workspace_slot(selection, None, "玩家物品：清空槽位", cx);
    }

    fn export_player_inventory_config(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = self.players.detail.as_ref() else {
            return;
        };
        let player_id = player_id_label(&detail.id);
        let items = player_inventory_entries(&detail.nbt)
            .into_iter()
            .map(|entry| {
                let item = serde_json::to_value(&entry.item.nbt).unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "container": entry.kind.label(),
                    "container_key": entry.kind.nbt_key(),
                    "slot": entry.slot.unwrap_or(entry.list_index as i32),
                    "id": entry.item.name,
                    "item": item,
                })
            })
            .collect::<Vec<_>>();
        let text = match serde_json::to_string_pretty(&serde_json::json!({
            "format": "bmcbl.player-inventory.v1",
            "player": player_id,
            "items": items,
        })) {
            Ok(text) => text,
            Err(error) => {
                self.status = SharedString::from(format!("生成背包配置失败: {error}"));
                cx.notify();
                return;
            }
        };
        let safe = safe_export_stem(&player_id);
        let Some(path) = pick_save_path_with_filter(
            "BMCBL 玩家背包配置",
            &["json"],
            &format!("{safe}_inventory.json"),
        ) else {
            return;
        };
        match fs::write(&path, text) {
            Ok(()) => self.status = SharedString::from(format!("玩家背包配置已导出：{path}")),
            Err(error) => self.status = SharedString::from(format!("导出背包配置失败: {error}")),
        }
        cx.notify();
    }

    pub(super) fn render_player_item_context_menu(
        &self,
        colors: &ThemeColors,
        menu: PlayerItemContextMenuState,
        cx: &mut Context<Self>,
    ) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let entry = self.workspace_entry_for_selection(menu.selection);
        let has_item = entry.is_some();
        let item_name = entry
            .as_ref()
            .and_then(|entry| entry.item.name.as_deref())
            .map_or_else(
                || t!("MapViewer.item_empty"),
                |name| SharedString::from(name.to_string()),
            );
        let placement = place_context_menu_at_anchor(
            ContextMenuAnchor::Cursor(menu.position),
            self.window_width,
            self.window_height,
            292.0,
            500.0,
        );
        let selected_in_multi = self
            .player_workspace
            .multi_selected_items
            .contains(&menu.selection);

        let entity = cx.entity();
        let copy_selection = menu.selection;
        let copy_items = vec![
            ContextMenuItem::new(t!("MapViewer.item_copy_config"))
                .description(t!("MapViewer.item_copy_config_hint"))
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| {
                        entity.update(cx, |this, cx| {
                            this.copy_player_item_config(copy_selection, cx);
                        })
                    }
                }),
            ContextMenuItem::new(t!("MapViewer.item_copy_id"))
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| {
                        entity.update(cx, |this, cx| this.copy_player_item_id(copy_selection, cx))
                    }
                }),
            ContextMenuItem::new(t!("MapViewer.item_copy_nbt"))
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| {
                        entity.update(cx, |this, cx| this.copy_player_item_nbt(copy_selection, cx))
                    }
                }),
            ContextMenuItem::new(t!("MapViewer.item_export_config"))
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| {
                        entity.update(cx, |this, cx| {
                            this.export_player_item_file(copy_selection, cx)
                        })
                    }
                }),
        ];
        let toggle_entity = cx.entity();
        let copy_entry = ContextMenuEntry::submenu(
            if self.player_workspace.item_context_copy_open {
                t!("MapViewer.item_copy_export_collapse")
            } else {
                t!("MapViewer.item_copy_export")
            },
            self.player_workspace.item_context_copy_open,
            copy_items,
            move |cx| toggle_entity.update(cx, |this, cx| this.toggle_player_item_copy_menu(cx)),
        );

        let selection = menu.selection;
        let entity = cx.entity();
        let mut groups = vec![
            ContextMenuGroup::new(vec![copy_entry]),
            ContextMenuGroup::titled(
                t!("MapViewer.item_slot_actions"),
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.item_cut"))
                            .description(t!("MapViewer.item_cut_hint"))
                            .disabled(!has_item)
                            .on_click({
                                let entity = entity.clone();
                                move |cx| {
                                    entity
                                        .update(cx, |this, cx| this.cut_player_item(selection, cx))
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.item_paste")).on_click({
                            let entity = entity.clone();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.paste_player_item_context(selection, cx)
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.item_import")).on_click({
                            let entity = entity.clone();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.import_player_item_file(selection, cx)
                                })
                            }
                        }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(t!("MapViewer.item_duplicate"))
                            .disabled(!has_item)
                            .on_click({
                                let entity = entity.clone();
                                move |cx| {
                                    entity.update(cx, |this, cx| {
                                        this.duplicate_player_item_to_free_slot(selection, cx)
                                    })
                                }
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(if selected_in_multi {
                            t!("MapViewer.item_remove_multi")
                        } else {
                            t!("MapViewer.item_add_multi")
                        })
                        .disabled(!has_item)
                        .on_click({
                            let entity = entity.clone();
                            move |cx| {
                                entity.update(cx, |this, cx| {
                                    this.toggle_context_multi_select(selection, cx)
                                })
                            }
                        }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                t!("MapViewer.item_config"),
                vec![ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.item_export_inventory"))
                        .description(t!("MapViewer.item_export_inventory_hint"))
                        .on_click({
                            let entity = entity.clone();
                            move |cx| {
                                entity
                                    .update(cx, |this, cx| this.export_player_inventory_config(cx))
                            }
                        }),
                )],
            ),
        ];
        if has_item {
            groups.push(ContextMenuGroup::titled(
                t!("MapViewer.item_dangerous"),
                vec![ContextMenuEntry::item(
                    ContextMenuItem::new(t!("MapViewer.item_clear"))
                        .danger(true)
                        .on_click({
                            let entity = entity.clone();
                            move |cx| {
                                entity.update(cx, |this, cx| this.clear_context_item(selection, cx))
                            }
                        }),
                )],
            ));
        }

        div().child(
            ContextMenu::new(colors, groups)
                .header(t!(
                    "MapViewer.item_header",
                    item = &item_name,
                    container = selection.kind.label(),
                    slot = selection.slot
                ))
                .placement(placement)
                .on_dismiss({
                    let entity = cx.entity();
                    move |cx| entity.update(cx, |this, cx| this.close_player_item_context_menu(cx))
                }),
        )
    }
}

fn safe_export_stem(value: &str) -> String {
    let mut output = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while output.contains("__") {
        output = output.replace("__", "_");
    }
    output
        .trim_matches('_')
        .chars()
        .take(80)
        .collect::<String>()
}

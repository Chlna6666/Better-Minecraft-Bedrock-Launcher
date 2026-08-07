from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def edit(path, replacements):
    p = ROOT / path
    text = p.read_text(encoding="utf-8")
    for old, new in replacements:
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected one anchor, got {count}: {old[:140]!r}")
        text = text.replace(old, new, 1)
    p.write_text(text, encoding="utf-8", newline="\n")

edit("src/ui/window/map_viewer.rs", [
    ("mod player_panel;\nmod player_workspace;", "mod player_item_menu;\nmod player_panel;\nmod player_workspace;")
])

edit("src/ui/window/map_viewer/player_workspace.rs", [
    ("        let item_id = workspace_input(window, cx, \"minecraft:diamond_sword\");\n        let count = workspace_input(window, cx, \"1\");\n        let damage = workspace_input(window, cx, \"0\");",
     "        let item_id = workspace_input(window, cx, \"物品 ID，例如 minecraft:gold_ingot\");\n        let count = workspace_input(window, cx, \"数量\");\n        let damage = workspace_input(window, cx, \"Damage\");"),
    ("        let can_place_on = workspace_input(window, cx, \"minecraft:stone, minecraft:dirt\");\n        let can_destroy = workspace_input(window, cx, \"minecraft:stone, minecraft:dirt\");",
     "        let can_place_on = workspace_input(window, cx, \"可放置方块 ID，逗号分隔\");\n        let can_destroy = workspace_input(window, cx, \"可破坏方块 ID，逗号分隔\");"),
    ("    pub(super) pressed_item: Option<PlayerItemSelection>,\n    pub(super) press_generation: u64,\n    pub(super) open_first_after_refresh: bool,",
     "    pub(super) pressed_item: Option<PlayerItemSelection>,\n    pub(super) press_generation: u64,\n    pub(super) item_context_menu: Option<super::player_item_menu::PlayerItemContextMenuState>,\n    pub(super) item_context_copy_open: bool,\n    pub(super) open_first_after_refresh: bool,"),
    ("            pressed_item: None,\n            press_generation: 0,\n            open_first_after_refresh: false,",
     "            pressed_item: None,\n            press_generation: 0,\n            item_context_menu: None,\n            item_context_copy_open: false,\n            open_first_after_refresh: false,"),
    ('''    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
        let available = self.viewport.width.max(320.0);
        let compact = available < 620.0;
        let outer_padding = if available < 470.0 {
            8.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let panel_padding = if available < 470.0 {
            9.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let slot_gap = if compact { 3.0 } else { 4.0 };
        let usable = (available - outer_padding * 2.0 - panel_padding * 2.0)
            .min(584.0)
            .max(288.0);
        let slot_size = ((usable - slot_gap * 8.0) / 9.0).clamp(30.0, 52.0);
        let grid_width = slot_size * 9.0 + slot_gap * 8.0;
''',
'''    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
        // Do not reuse viewport.width here: opening/closing the right dock can leave it one
        // layout tick behind the actual center workspace. Compute against the current dock
        // geometry directly so backpack rows and hotbar share the same pixel grid immediately.
        let available = (self
            .center_stage_size(size(px(self.window_width), px(self.window_height)))
            .width
            / px(1.0))
            .max(320.0);
        let compact = available < 620.0;
        let outer_padding = if available < 470.0 {
            8.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let panel_padding = if available < 470.0 {
            9.0
        } else if compact {
            12.0
        } else {
            18.0
        };
        let slot_gap = if compact { 3.0 } else { 4.0 };
        let usable = (available - outer_padding * 2.0 - panel_padding * 2.0)
            .min(584.0)
            .max(288.0);
        let slot_size = ((usable - slot_gap * 8.0) / 9.0)
            .floor()
            .clamp(30.0, 52.0);
        let grid_width = (slot_size * 9.0 + slot_gap * 8.0).round();
'''),
    ('''            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.end_player_item_press(selection, cx)
                }),
            )
''',
'''            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.end_player_item_press(selection, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.open_player_item_context_menu(selection, event.position, cx);
                    cx.stop_propagation();
                }),
            )
'''),
    ('''    fn selected_workspace_entry(&self) -> Option<PlayerInventoryEntry> {
        let selection = self.player_workspace.selected_item?;
        let detail = self.players.detail.as_ref()?;
        player_inventory_entries(&detail.nbt)
            .into_iter()
            .find(|entry| {
                entry.kind == selection.kind
                    && entry.slot.unwrap_or(entry.list_index as i32) == selection.slot
            })
    }

    fn populate_player_item_visual_inputs(
        &mut self,
        selection: PlayerItemSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = self.selected_workspace_entry();
''',
'''    pub(super) fn workspace_entry_for_selection(
        &self,
        selection: PlayerItemSelection,
    ) -> Option<PlayerInventoryEntry> {
        let detail = self.players.detail.as_ref()?;
        player_inventory_entries(&detail.nbt)
            .into_iter()
            .find(|entry| {
                entry.kind == selection.kind
                    && entry.slot.unwrap_or(entry.list_index as i32) == selection.slot
            })
    }

    fn selected_workspace_entry(&self) -> Option<PlayerInventoryEntry> {
        self.workspace_entry_for_selection(self.player_workspace.selected_item?)
    }

    pub(super) fn sync_selected_player_item_visual_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        let entry = self.workspace_entry_for_selection(selection);
'''),
    ('''        for (input, value) in [
            (self.player_workspace.item_id.clone(), id),
            (self.player_workspace.count.clone(), count),
            (self.player_workspace.damage.clone(), damage),
            (self.player_workspace.custom_name.clone(), custom_name),
            (self.player_workspace.lore.clone(), lore),
            (self.player_workspace.can_place_on.clone(), can_place_on),
            (self.player_workspace.can_destroy.clone(), can_destroy),
        ] {
            input.update(cx, |input, cx| {
                input.set_value(SharedString::from(value), window, cx);
            });
        }
        let _ = selection;
    }

    fn sync_player_item_raw_editor(&mut self, cx: &mut Context<Self>) {
''',
'''        for (input, value) in [
            (self.player_workspace.item_id.clone(), id),
            (self.player_workspace.count.clone(), count),
            (self.player_workspace.damage.clone(), damage),
            (self.player_workspace.custom_name.clone(), custom_name),
            (self.player_workspace.lore.clone(), lore),
            (self.player_workspace.can_place_on.clone(), can_place_on),
            (self.player_workspace.can_destroy.clone(), can_destroy),
        ] {
            input.update(cx, |input, cx| input.set_text(SharedString::from(value), cx));
        }
    }

    pub(super) fn sync_player_item_raw_editor(&mut self, cx: &mut Context<Self>) {
'''),
    ('''        if dock_changed {
            self.update_viewport_after_dock_change(cx);
        }
        self.populate_player_item_visual_inputs(selection, window, cx);
        self.sync_player_item_raw_editor(cx);
''',
'''        self.sync_selected_player_item_visual_inputs(cx);
        self.sync_player_item_raw_editor(cx);
        if dock_changed {
            self.update_viewport_after_dock_change(cx);
        }
        let _ = window;
'''),
    ('''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_player_item_raw_editor(cx);
                        this.status =
                            SharedString::from("批量物品移动完成 · 未覆盖其他槽位 · 可从历史撤销");
''',
'''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_selected_player_item_visual_inputs(cx);
                        this.sync_player_item_raw_editor(cx);
                        this.status =
                            SharedString::from("批量物品移动完成 · 未覆盖其他槽位 · 可从历史撤销");
'''),
    ('''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "物品拖拽已写入 · 目标有物品时自动交换 · 可从历史撤销",
                        );
''',
'''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_selected_player_item_visual_inputs(cx);
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "物品拖拽已写入 · 目标有物品时自动交换 · 可从历史撤销",
                        );
'''),
    ('''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "玩家物品已写入 · 未识别 NBT 字段已保留 · 可从历史撤销",
                        );
''',
'''                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_selected_player_item_visual_inputs(cx);
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "玩家物品已写入 · 未识别 NBT 字段已保留 · 可从历史撤销",
                        );
'''),
    ("    fn write_player_workspace_slot(\n", "    pub(super) fn write_player_workspace_slot(\n"),
    ("fn inventory_kind_capacity(kind: PlayerInventoryKind) -> i32 {", "pub(super) fn inventory_kind_capacity(kind: PlayerInventoryKind) -> i32 {"),
    ("fn set_workspace_item_slot(item: &mut NbtTag, slot: i32) -> Result<(), String> {", "pub(super) fn set_workspace_item_slot(item: &mut NbtTag, slot: i32) -> Result<(), String> {"),
    ("fn parse_workspace_item_import(text: &str, slot: i32) -> Result<NbtTag, String> {", "pub(super) fn parse_workspace_item_import(text: &str, slot: i32) -> Result<NbtTag, String> {"),
    ('''    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(object) = value.as_object() {
            return simplified_json_item(object, slot);
        }
    }
''',
'''    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(item) = value.get("item") {
            let mut tag = serde_json::from_value::<NbtTag>(item.clone())
                .map_err(|error| format!("BMCBL 物品配置中的 item 无法解析: {error}"))?;
            if !matches!(tag, NbtTag::Compound(_)) {
                return Err("BMCBL 物品配置中的 item 必须是 Compound".to_string());
            }
            set_workspace_item_slot(&mut tag, slot)?;
            return Ok(tag);
        }
        if let Some(object) = value.as_object() {
            return simplified_json_item(object, slot);
        }
    }
''')
])

edit("src/ui/window/map_viewer/panels.rs", [
    ('''        MapMenuOverlaySnapshot {
            open: self.ui_state.top_more_open || self.context_menu.is_some(),
        }
''',
'''        MapMenuOverlaySnapshot {
            open: self.ui_state.top_more_open
                || self.context_menu.is_some()
                || self.player_workspace.item_context_menu.is_some(),
        }
'''),
    ('''        let has_menu = self.ui_state.top_more_open || self.context_menu.is_some();
''',
'''        let has_menu = self.ui_state.top_more_open
            || self.context_menu.is_some()
            || self.player_workspace.item_context_menu.is_some();
'''),
    ('''                    .when_some(self.context_menu, |this, menu| {
                        this.child(self.render_context_menu(colors, menu, cx))
                    }),
''',
'''                    .when_some(self.context_menu, |this, menu| {
                        this.child(self.render_context_menu(colors, menu, cx))
                    })
                    .when_some(self.player_workspace.item_context_menu, |this, menu| {
                        this.child(self.render_player_item_context_menu(colors, menu, cx))
                    }),
''')
])

edit("src/ui/window/map_viewer/interactions.rs", [
    ('''    pub(super) fn toggle_top_more(&mut self, cx: &mut Context<Self>) {
        self.ui_state.top_more_open = !self.ui_state.top_more_open;
        self.context_menu = None;
        cx.notify();
    }
''',
'''    pub(super) fn toggle_top_more(&mut self, cx: &mut Context<Self>) {
        self.ui_state.top_more_open = !self.ui_state.top_more_open;
        self.context_menu = None;
        self.player_workspace.item_context_menu = None;
        self.player_workspace.item_context_copy_open = false;
        cx.notify();
    }
'''),
    ('''        let changed = self.context_menu.take().is_some()
            || self.players.context_target.take().is_some()
            || self.ui_state.top_more_open
''',
'''        let changed = self.context_menu.take().is_some()
            || self.player_workspace.item_context_menu.take().is_some()
            || self.players.context_target.take().is_some()
            || self.ui_state.top_more_open
'''),
    ('''        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;
''',
'''        self.ui_state.top_more_open = false;
        self.player_workspace.item_context_copy_open = false;
        self.ui_state.context_more_open = false;
''')
])

menu = r'''use super::model::*;
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

    fn player_item_config_json(
        &self,
        selection: PlayerItemSelection,
    ) -> Result<String, String> {
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

    fn copy_player_item_nbt(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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

    fn copy_player_item_id(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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

    fn cut_player_item(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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

    fn import_player_item_file(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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

    fn export_player_item_file(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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

    fn clear_context_item(
        &mut self,
        selection: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
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
        let entry = self.workspace_entry_for_selection(menu.selection);
        let has_item = entry.is_some();
        let item_name = entry
            .as_ref()
            .and_then(|entry| entry.item.name.as_deref())
            .unwrap_or("空槽位");
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
            ContextMenuItem::new("复制物品配置")
                .description("完整 BMCBL JSON；可直接粘贴到其他槽位")
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| entity.update(cx, |this, cx| { this.copy_player_item_config(copy_selection, cx); })
                }),
            ContextMenuItem::new("复制物品 ID")
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| entity.update(cx, |this, cx| this.copy_player_item_id(copy_selection, cx))
                }),
            ContextMenuItem::new("复制原始 NBT JSON")
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| entity.update(cx, |this, cx| this.copy_player_item_nbt(copy_selection, cx))
                }),
            ContextMenuItem::new("导出物品配置…")
                .disabled(!has_item)
                .on_click({
                    let entity = entity.clone();
                    move |cx| entity.update(cx, |this, cx| this.export_player_item_file(copy_selection, cx))
                }),
        ];
        let toggle_entity = cx.entity();
        let copy_entry = ContextMenuEntry::submenu(
            if self.player_workspace.item_context_copy_open {
                "复制 / 导出（收起）"
            } else {
                "复制 / 导出…"
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
                "槽位操作",
                vec![
                    ContextMenuEntry::item(
                        ContextMenuItem::new("剪切物品")
                            .description("复制完整配置后清空当前槽位")
                            .disabled(!has_item)
                            .on_click({
                                let entity = entity.clone();
                                move |cx| entity.update(cx, |this, cx| this.cut_player_item(selection, cx))
                            }),
                    ),
                    ContextMenuEntry::item(ContextMenuItem::new("粘贴到此槽位").on_click({
                        let entity = entity.clone();
                        move |cx| entity.update(cx, |this, cx| this.paste_player_item_context(selection, cx))
                    })),
                    ContextMenuEntry::item(ContextMenuItem::new("从配置文件导入…").on_click({
                        let entity = entity.clone();
                        move |cx| entity.update(cx, |this, cx| this.import_player_item_file(selection, cx))
                    })),
                    ContextMenuEntry::item(
                        ContextMenuItem::new("复制到下一个空槽")
                            .disabled(!has_item)
                            .on_click({
                                let entity = entity.clone();
                                move |cx| entity.update(cx, |this, cx| this.duplicate_player_item_to_free_slot(selection, cx))
                            }),
                    ),
                    ContextMenuEntry::item(
                        ContextMenuItem::new(if selected_in_multi { "从多选移除" } else { "加入多选" })
                            .disabled(!has_item)
                            .on_click({
                                let entity = entity.clone();
                                move |cx| entity.update(cx, |this, cx| this.toggle_context_multi_select(selection, cx))
                            }),
                    ),
                ],
            ),
            ContextMenuGroup::titled(
                "配置",
                vec![ContextMenuEntry::item(
                    ContextMenuItem::new("导出整个玩家背包配置…")
                        .description("背包、快捷栏、末影箱、护甲与副手")
                        .on_click({
                            let entity = entity.clone();
                            move |cx| entity.update(cx, |this, cx| this.export_player_inventory_config(cx))
                        }),
                )],
            ),
        ];
        if has_item {
            groups.push(ContextMenuGroup::titled(
                "危险操作",
                vec![ContextMenuEntry::item(
                    ContextMenuItem::new("清空槽位")
                        .danger(true)
                        .on_click({
                            let entity = entity.clone();
                            move |cx| entity.update(cx, |this, cx| this.clear_context_item(selection, cx))
                        }),
                )],
            ));
        }

        div().child(
            ContextMenu::new(colors, groups)
                .header(format!("{} · {} · 槽位 {}", item_name, selection.kind.label(), selection.slot))
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
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect::<String>();
    while output.contains("__") {
        output = output.replace("__", "_");
    }
    output.trim_matches('_').chars().take(80).collect::<String>()
}
'''
(ROOT / "src/ui/window/map_viewer/player_item_menu.rs").write_text(menu, encoding="utf-8", newline="\n")

use super::editor::*;
use super::map_history::MapHistoryCapture;
use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::viewport::viewport_screen_for_block;
use std::collections::HashMap as StdHashMap;
use std::fs;

const PLAYER_MAIN_INVENTORY_SIZE: i32 = 36;
const PLAYER_ITEM_CATALOG_LIMIT: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlayerRecordHealth {
    Complete,
    Partial,
    Stub,
    Invalid,
}

impl PlayerRecordHealth {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Complete => "完整",
            Self::Partial => "部分",
            Self::Stub => "残留",
            Self::Invalid => "无效",
        }
    }

    pub(super) fn localized_label(self, i18n: &I18n) -> SharedString {
        match self {
            Self::Complete => t!("MapViewer.health_complete"),
            Self::Partial => t!("MapViewer.health_partial"),
            Self::Stub => t!("MapViewer.health_stub"),
            Self::Invalid => t!("MapViewer.health_invalid"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PlayerRecordQuality {
    pub(super) health: PlayerRecordHealth,
    pub(super) score: i16,
    pub(super) trusted_server: bool,
    pub(super) has_unique_id: bool,
    pub(super) has_position: bool,
    pub(super) has_dimension: bool,
    pub(super) has_inventory: bool,
    pub(super) item_count: usize,
    pub(super) ender_item_count: usize,
}

impl PlayerRecordQuality {
    fn invalid() -> Self {
        Self {
            health: PlayerRecordHealth::Invalid,
            score: -32767,
            trusted_server: false,
            has_unique_id: false,
            has_position: false,
            has_dimension: false,
            has_inventory: false,
            item_count: 0,
            ender_item_count: 0,
        }
    }

    fn sort_bucket(&self, id: &PlayerId) -> u8 {
        if self.health == PlayerRecordHealth::Invalid {
            return 7;
        }
        if matches!(id, PlayerId::Local) {
            return 0;
        }
        if self.trusted_server {
            return 1;
        }
        match self.health {
            PlayerRecordHealth::Complete => 2,
            PlayerRecordHealth::Partial if is_server_like_player_id(id) => 3,
            PlayerRecordHealth::Partial => 4,
            PlayerRecordHealth::Stub if is_server_like_player_id(id) => 5,
            PlayerRecordHealth::Stub => 6,
            PlayerRecordHealth::Invalid => 7,
        }
    }

    fn marker_candidate(&self, id: &PlayerId) -> bool {
        self.has_position
            && self.has_dimension
            && (matches!(id, PlayerId::Local)
                || self.trusted_server
                || self.health == PlayerRecordHealth::Complete)
    }

    pub(super) fn search_text(&self) -> String {
        format!(
            "{} score:{} uid:{} pos:{} dim:{} inventory:{} items:{} ender:{}",
            self.health.label(),
            self.score,
            self.has_unique_id,
            self.has_position,
            self.has_dimension,
            self.has_inventory,
            self.item_count,
            self.ender_item_count
        )
        .to_ascii_lowercase()
    }
}

#[derive(Clone, Debug)]
struct PlayerProbe {
    position: Option<[f64; 3]>,
    dimension_id: Option<i32>,
    quality: PlayerRecordQuality,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlayerInventoryKind {
    Inventory,
    Armor,
    Offhand,
    EnderChest,
}

impl PlayerInventoryKind {
    pub(super) const fn nbt_key(self) -> &'static str {
        match self {
            Self::Inventory => "Inventory",
            Self::Armor => "Armor",
            Self::Offhand => "Offhand",
            Self::EnderChest => "EnderChestInventory",
        }
    }

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Inventory => "背包",
            Self::Armor => "护甲",
            Self::Offhand => "副手",
            Self::EnderChest => "末影箱",
        }
    }

    pub(super) fn localized_label(self, i18n: &I18n) -> SharedString {
        match self {
            Self::Inventory => t!("MapViewer.inventory"),
            Self::Armor => t!("MapViewer.armor"),
            Self::Offhand => t!("MapViewer.offhand"),
            Self::EnderChest => t!("MapViewer.ender_chest"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PlayerInventoryEntry {
    pub(super) kind: PlayerInventoryKind,
    pub(super) list_index: usize,
    pub(super) slot: Option<i32>,
    pub(super) item: bedrock_world::ItemStack,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerEnchantEntry {
    pub(super) list_index: usize,
    pub(super) id: i16,
    pub(super) level: i16,
}

#[derive(Clone, Debug)]
pub(super) struct PlayerItemTexture {
    pub(super) id: SharedString,
    pub(super) label: SharedString,
    pub(super) path: Arc<Path>,
}

#[derive(Clone, Debug)]
pub(super) enum PlayerItemMutation {
    AddItem {
        kind: PlayerInventoryKind,
        name: String,
    },
    DeleteItem {
        kind: PlayerInventoryKind,
        list_index: usize,
    },
    DuplicateItem {
        kind: PlayerInventoryKind,
        list_index: usize,
    },
    AdjustCount {
        kind: PlayerInventoryKind,
        list_index: usize,
        delta: i32,
    },
    SetCount {
        kind: PlayerInventoryKind,
        list_index: usize,
        value: i8,
    },
    AdjustDamage {
        kind: PlayerInventoryKind,
        list_index: usize,
        delta: i32,
    },
    SetDamage {
        kind: PlayerInventoryKind,
        list_index: usize,
        value: i16,
    },
    SetCustomName {
        kind: PlayerInventoryKind,
        list_index: usize,
        value: String,
    },
    SetLore {
        kind: PlayerInventoryKind,
        list_index: usize,
        lines: Vec<String>,
    },
    AddEnchant {
        kind: PlayerInventoryKind,
        list_index: usize,
        id: i16,
        level: i16,
    },
    AdjustEnchant {
        kind: PlayerInventoryKind,
        list_index: usize,
        enchant_index: usize,
        delta: i16,
    },
    RemoveEnchant {
        kind: PlayerInventoryKind,
        list_index: usize,
        enchant_index: usize,
    },
}

impl PlayerItemMutation {
    fn history_label(&self) -> String {
        match self {
            Self::AddItem { .. } => "玩家物品：添加物品".to_string(),
            Self::DeleteItem { .. } => "玩家物品：删除物品".to_string(),
            Self::DuplicateItem { .. } => "玩家物品：复制物品".to_string(),
            Self::AdjustCount { .. } | Self::SetCount { .. } => "玩家物品：修改数量".to_string(),
            Self::AdjustDamage { .. } | Self::SetDamage { .. } => {
                "玩家物品：修改 Damage".to_string()
            }
            Self::SetCustomName { .. } => "玩家物品：修改自定义名称".to_string(),
            Self::SetLore { .. } => "玩家物品：修改 Lore".to_string(),
            Self::AddEnchant { .. } | Self::AdjustEnchant { .. } | Self::RemoveEnchant { .. } => {
                "玩家物品：修改附魔".to_string()
            }
        }
    }
}

#[derive(Clone, Debug)]
struct PlayerRefreshMarker {
    id: PlayerId,
    label: SharedString,
    dimension: Dimension,
    x: i32,
    z: i32,
}

#[derive(Clone, Debug)]
struct PlayerRefreshResult {
    players: Vec<PlayerSummary>,
    markers: BTreeMap<Dimension, Vec<Marker>>,
}

impl MapViewerWindowView {
    pub(super) fn refresh_players(&mut self, cx: &mut Context<Self>) {
        self.players.generation = self.players.generation.saturating_add(1);
        self.players.loading = true;
        self.players.error = None;
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        self.status = SharedString::from("正在读取、校验并排序玩家记录...");
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::BedrockWorldOpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let ids = world
                        .list_players_blocking()
                        .map_err(|error| error.to_string())?;

                    let mut rows = Vec::with_capacity(ids.len());
                    let mut marker_records = Vec::new();
                    for id in ids {
                        let raw_label = player_id_label(&id);
                        let probe = world
                            .get_player_blocking(&id)
                            .map_err(|error| error.to_string())
                            .and_then(|data| {
                                let data = data.ok_or_else(|| "玩家记录不存在".to_string())?;
                                player_probe(&id, &data)
                            });

                        match probe {
                            Ok(probe) => {
                                let label = SharedString::from(player_friendly_label(&id, true));
                                let bucket = probe.quality.sort_bucket(&id);
                                let score = probe.quality.score;
                                if probe.quality.marker_candidate(&id) {
                                    if let (Some(position), Some(dimension_id)) =
                                        (probe.position, probe.dimension_id)
                                    {
                                        if position[0].is_finite() && position[2].is_finite() {
                                            marker_records.push(PlayerRefreshMarker {
                                                id: id.clone(),
                                                label: label.clone(),
                                                dimension: Dimension::from_id(dimension_id),
                                                x: position[0]
                                                    .floor()
                                                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                                                    as i32,
                                                z: position[2]
                                                    .floor()
                                                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                                                    as i32,
                                            });
                                        }
                                    }
                                }
                                rows.push((
                                    bucket,
                                    -score,
                                    raw_label,
                                    PlayerSummary {
                                        id,
                                        label,
                                        quality: probe.quality,
                                    },
                                ));
                            }
                            Err(_) => {
                                let quality = PlayerRecordQuality::invalid();
                                rows.push((
                                    quality.sort_bucket(&id),
                                    -quality.score,
                                    raw_label.clone(),
                                    PlayerSummary {
                                        id,
                                        label: SharedString::from(format!(
                                            "无效记录 · {raw_label}"
                                        )),
                                        quality,
                                    },
                                ));
                            }
                        }
                    }

                    rows.sort_by(|a, b| {
                        a.0.cmp(&b.0)
                            .then_with(|| a.1.cmp(&b.1))
                            .then_with(|| a.2.cmp(&b.2))
                    });
                    let players = rows.into_iter().map(|(_, _, _, player)| player).collect();
                    let mut markers: BTreeMap<Dimension, Vec<Marker>> = BTreeMap::new();
                    for marker in marker_records {
                        markers.entry(marker.dimension).or_default().push(Marker {
                            x: marker.x,
                            z: marker.z,
                            label: marker.label,
                            player_id: Some(marker.id),
                        });
                    }
                    for values in markers.values_mut() {
                        values.sort_by(|a, b| {
                            a.label
                                .as_ref()
                                .cmp(b.label.as_ref())
                                .then_with(|| a.x.cmp(&b.x))
                                .then_with(|| a.z.cmp(&b.z))
                        });
                    }
                    Ok::<_, String>(PlayerRefreshResult { players, markers })
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.players.generation != generation {
                    return;
                }
                this.players.loading = false;
                match result {
                    Ok(result) => {
                        let selected_still_exists =
                            this.players.selected.as_ref().is_some_and(|id| {
                                result.players.iter().any(|player| &player.id == id)
                            });
                        this.players.players = result.players;
                        if !selected_still_exists {
                            this.players.selected = preferred_player_id(&this.players.players);
                        }
                        this.markers = result.markers;
                        this.markers_generation = this.markers_generation.saturating_add(1);
                        this.last_synced_canvas_snapshot_key = None;

                        let visible_marker_count =
                            this.markers.get(&this.dimension).map_or(0, Vec::len);
                        this.status = SharedString::from(format!(
                            "玩家列表已加载 · {} 条记录 · 当前维度 {} 个地图标记",
                            this.players.players.len(),
                            visible_marker_count
                        ));
                        let colors = this.theme_colors(cx);
                        this.sync_canvas_snapshot(colors, cx);
                        if this.player_workspace.open_first_after_refresh
                            && this.ui_state.active_left_panel == MapViewerLeftPanel::Players
                            && this.ui_state.left_panel_open
                        {
                            this.player_workspace.open_first_after_refresh = false;
                            if let Some(id) = preferred_player_id(&this.players.players) {
                                this.open_player_workspace_for_player(
                                    id,
                                    PlayerWorkspaceCenter::Inventory,
                                    cx,
                                );
                            }
                        } else if this.player_workspace_active() {
                            if let Some(id) = this.players.selected.clone() {
                                this.load_player_detail(id, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.players.error = Some(SharedString::from(error.clone()));
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn load_player_detail(&mut self, id: PlayerId, cx: &mut Context<Self>) {
        self.players.generation = self.players.generation.saturating_add(1);
        self.players.selected = Some(id.clone());
        self.players.loading = true;
        self.players.error = None;
        self.players.pending_save_confirmation = None;
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        self.status = SharedString::from(format!("正在读取玩家 {}...", player_id_label(&id)));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        bedrock_world::BedrockWorldOpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    let data = world
                        .get_player_blocking(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "玩家记录不存在".to_string())?;
                    player_detail_from_data(data).map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.players.generation != generation {
                    return;
                }
                this.players.loading = false;
                match result {
                    Ok(detail) => {
                        this.players.detail = Some(detail);
                        this.status = SharedString::from("玩家记录已加载");
                    }
                    Err(error) => {
                        this.players.detail = None;
                        this.players.error = Some(SharedString::from(error.clone()));
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn open_selected_player_in_editor(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = self.players.detail.clone() else {
            self.status = SharedString::from("请先选择玩家记录");
            cx.notify();
            return;
        };
        self.set_professional_detail(Some(player_editor_detail(detail)), cx);
        self.open_right_nbt_panel(cx);
        self.status = SharedString::from(
            "已打开高级 NBT JSON 编辑器 · 保存会校验并直接写回玩家 NBT，历史面板可撤销",
        );
        cx.notify();
    }

    pub(super) fn add_player_item_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let text = clipboard_text(cx);
        let Some(name) = parse_item_id(&text) else {
            self.status =
                SharedString::from("剪贴板中没有有效物品 ID；示例：minecraft:diamond_sword");
            cx.notify();
            return;
        };
        self.run_player_item_mutation(
            PlayerItemMutation::AddItem {
                kind: PlayerInventoryKind::Inventory,
                name,
            },
            cx,
        );
    }

    pub(super) fn set_player_item_name_from_clipboard(
        &mut self,
        kind: PlayerInventoryKind,
        list_index: usize,
        cx: &mut Context<Self>,
    ) {
        let value = clipboard_text(cx).trim().to_string();
        if value.is_empty() {
            self.status = SharedString::from("剪贴板名称为空");
            cx.notify();
            return;
        }
        self.run_player_item_mutation(
            PlayerItemMutation::SetCustomName {
                kind,
                list_index,
                value,
            },
            cx,
        );
    }

    pub(super) fn set_player_item_lore_from_clipboard(
        &mut self,
        kind: PlayerInventoryKind,
        list_index: usize,
        cx: &mut Context<Self>,
    ) {
        let text = clipboard_text(cx);
        let lines = text
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            self.status = SharedString::from("剪贴板 Lore 为空；每行会写为一条 Lore");
            cx.notify();
            return;
        }
        self.run_player_item_mutation(
            PlayerItemMutation::SetLore {
                kind,
                list_index,
                lines,
            },
            cx,
        );
    }

    pub(super) fn add_player_item_enchant_from_clipboard(
        &mut self,
        kind: PlayerInventoryKind,
        list_index: usize,
        cx: &mut Context<Self>,
    ) {
        let text = clipboard_text(cx);
        let Some((id, level)) = parse_enchant_spec(&text) else {
            self.status =
                SharedString::from("剪贴板附魔格式无效；使用 `id:等级`，例如 `9:32767`（锋利）");
            cx.notify();
            return;
        };
        self.run_player_item_mutation(
            PlayerItemMutation::AddEnchant {
                kind,
                list_index,
                id,
                level,
            },
            cx,
        );
    }

    pub(super) fn run_player_item_mutation(
        &mut self,
        mutation: PlayerItemMutation,
        cx: &mut Context<Self>,
    ) {
        let i18n = cx.global::<I18n>().clone();
        let player_item_written = t!("MapViewer.player_item_written");
        let Some(id) = self.players.selected.clone() else {
            self.status = t!("MapViewer.select_player_record");
            cx.notify();
            return;
        };
        if self.players.saving {
            self.status = t!("MapViewer.player_write_pending");
            cx.notify();
            return;
        }

        let label = mutation.history_label();
        self.players.pending_save_confirmation = None;
        self.players.saving = true;
        self.players.generation = self.players.generation.saturating_add(1);
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        self.status = t!("MapViewer.performing_edit", action = &label);
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut options = bedrock_world::BedrockWorldOpenOptions::default();
                    options.read_only = false;
                    let world = BedrockWorld::open_blocking(&world_path, options)
                        .map_err(|error| error.to_string())?;
                    let history_capture =
                        capture_player_history(&world_path, &id, label.clone());

                    let mut data = world
                        .get_player_blocking(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "玩家记录不存在".to_string())?;
                    apply_player_item_mutation(&mut data.nbt, &mutation)?;
                    data = PlayerData::from_nbt(id.clone(), data.nbt)
                        .map_err(|error| error.to_string())?;
                    world
                        .put_player_blocking(&data)
                        .map_err(|error| error.to_string())?;
                    let detail = player_detail_from_data(data).map_err(|error| error.to_string());

                    match (history_capture, detail) {
                        (Ok(capture), Ok(detail)) => {
                            complete_after(capture, label.clone())?;
                            Ok(detail)
                        }
                        (Ok(capture), Err(error)) => {
                            let _ = complete_failed(capture, error.clone());
                            Err(error)
                        }
                        (Err(error), Ok(detail)) => {
                            tracing::warn!(%error, "map history capture failed after player item edit");
                            Ok(detail)
                        }
                        (Err(history_error), Err(write_error)) => {
                            Err(format!("{write_error}；历史捕获失败: {history_error}"))
                        }
                    }
                })
                .await;

            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.players.generation != generation {
                    return;
                }
                this.players.saving = false;
                match result {
                    Ok(detail) => {
                        this.players.detail = Some(detail);
                        this.status = player_item_written.clone();
                    }
                    Err(error) => {
                        this.players.error = Some(SharedString::from(error.clone()));
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn run_player_quick_edit(&mut self, edit: PlayerQuickEdit, cx: &mut Context<Self>) {
        let i18n = cx.global::<I18n>().clone();
        let edit_label = edit.localized_label(&i18n);
        let player_record_written = t!("MapViewer.player_record_written");
        let Some(id) = self.players.selected.clone() else {
            self.status = t!("MapViewer.select_player_record");
            cx.notify();
            return;
        };
        if self
            .players
            .pending_save_confirmation
            .as_ref()
            .is_none_or(|pending| pending != &edit)
        {
            self.players.pending_save_confirmation = Some(edit.clone());
            self.status = t!("MapViewer.confirm_edit", action = edit_label.clone());
            cx.notify();
            return;
        }
        self.players.pending_save_confirmation = None;
        self.players.saving = true;
        self.players.generation = self.players.generation.saturating_add(1);
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        let center_block = self.viewport.center_block(self.active_layout);
        let dimension = self.dimension;
        self.status = t!("MapViewer.performing_edit", action = edit_label);
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut options = bedrock_world::BedrockWorldOpenOptions::default();
                    options.read_only = false;
                    let world = BedrockWorld::open_blocking(&world_path, options)
                        .map_err(|error| error.to_string())?;
                    let history_capture = capture_player_history(&world_path, &id, edit.label());
                    let mut data = world
                        .get_player_blocking(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "玩家记录不存在".to_string())?;
                    apply_player_quick_edit(&mut data.nbt, &edit, center_block, dimension)?;
                    data = PlayerData::from_nbt(id.clone(), data.nbt)
                        .map_err(|error| error.to_string())?;
                    world
                        .put_player_blocking(&data)
                        .map_err(|error| error.to_string())?;
                    let detail = player_detail_from_data(data).map_err(|error| error.to_string());
                    match (history_capture, detail) {
                        (Ok(capture), Ok(detail)) => {
                            complete_after(capture, "玩家记录已写入")?;
                            Ok(detail)
                        }
                        (Ok(capture), Err(error)) => {
                            let _ = complete_failed(capture, error.clone());
                            Err(error)
                        }
                        (Err(error), Ok(detail)) => {
                            tracing::warn!(%error, "map history capture failed after player edit");
                            Ok(detail)
                        }
                        (Err(history_error), Err(write_error)) => {
                            Err(format!("{write_error}；历史捕获失败: {history_error}"))
                        }
                    }
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.players.generation != generation {
                    return;
                }
                this.players.saving = false;
                match result {
                    Ok(detail) => {
                        this.sync_player_marker_from_detail(&detail);
                        this.players.detail = Some(detail);
                        this.status = player_record_written.clone();
                        let colors = this.theme_colors(cx);
                        this.sync_canvas_snapshot(colors, cx);
                    }
                    Err(error) => {
                        this.players.error = Some(SharedString::from(error.clone()));
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn sync_player_marker_from_detail(&mut self, detail: &PlayerDetail) {
        let label = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_friendly_label(&detail.id, true)));

        for markers in self.markers.values_mut() {
            markers.retain(|marker| marker.label != label);
        }
        if let (Some(position), Some(dimension_id)) = (detail.position, detail.dimension_id) {
            if position[0].is_finite() && position[2].is_finite() {
                self.markers
                    .entry(Dimension::from_id(dimension_id))
                    .or_default()
                    .push(Marker {
                        x: position[0]
                            .floor()
                            .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                            as i32,
                        z: position[2]
                            .floor()
                            .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                            as i32,
                        label,
                        player_id: Some(detail.id.clone()),
                    });
            }
        }
        self.markers.retain(|_, values| !values.is_empty());
        self.markers_generation = self.markers_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
    }

    pub(super) fn player_item_catalog(&self) -> Arc<Vec<PlayerItemTexture>> {
        cached_item_catalog(&PathBuf::from(self.version.path.as_ref()))
    }

    pub(super) fn player_quick_item_catalog(&self) -> Vec<PlayerItemTexture> {
        let catalog = self.player_item_catalog();
        let common = [
            "minecraft:stone",
            "minecraft:dirt",
            "minecraft:torch",
            "minecraft:diamond",
            "minecraft:iron_ingot",
            "minecraft:gold_ingot",
            "minecraft:diamond_sword",
            "minecraft:diamond_pickaxe",
            "minecraft:bow",
            "minecraft:arrow",
            "minecraft:shield",
            "minecraft:elytra",
            "minecraft:firework_rocket",
            "minecraft:totem",
            "minecraft:enchanted_golden_apple",
            "minecraft:ender_pearl",
            "minecraft:shulker_box",
            "minecraft:water_bucket",
            "minecraft:lava_bucket",
            "minecraft:barrier",
            "minecraft:command_block",
            "minecraft:structure_block",
        ];

        let mut output = Vec::with_capacity(PLAYER_ITEM_CATALOG_LIMIT);
        let mut used = BTreeSet::new();
        for id in common {
            if let Some(entry) = catalog.iter().find(|entry| entry.id.as_ref() == id) {
                used.insert(entry.id.to_string());
                output.push(entry.clone());
            }
        }
        for entry in catalog.iter() {
            if output.len() >= PLAYER_ITEM_CATALOG_LIMIT {
                break;
            }
            if used.insert(entry.id.to_string()) {
                output.push(entry.clone());
            }
        }
        output
    }

    pub(super) fn player_item_texture(&self, item_name: Option<&str>) -> Option<Arc<Path>> {
        let name = item_name?.trim();
        if name.is_empty() {
            return None;
        }
        let normalized = normalize_item_id(name);
        let short = normalized
            .strip_prefix("minecraft:")
            .unwrap_or(normalized.as_str());
        self.player_item_catalog()
            .iter()
            .find(|entry| {
                entry.id.as_ref().eq_ignore_ascii_case(&normalized)
                    || entry
                        .path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .is_some_and(|stem| stem.eq_ignore_ascii_case(short))
            })
            .map(|entry| entry.path.clone())
    }
}

pub(super) fn capture_player_history(
    world_path: &Path,
    id: &PlayerId,
    label: impl Into<String>,
) -> Result<MapHistoryCapture, String> {
    let mut raw_keys = BTreeSet::new();
    let mut include_level_dat = false;
    if let Some(key) = id.storage_key() {
        raw_keys.insert(key.as_ref().to_vec());
    } else {
        include_level_dat = true;
    }
    capture_before(MapHistoryCaptureSpec {
        kind: MapHistoryEntryKind::PlayerEdit,
        label: label.into(),
        world_path: world_path.to_path_buf(),
        chunks: BTreeSet::new(),
        raw_keys,
        include_level_dat,
    })
    .map_err(|error| error.to_string())
}

fn is_server_like_player_id(id: &PlayerId) -> bool {
    match id {
        PlayerId::Xuid(_) => true,
        PlayerId::Unknown(value) => {
            let value = value.to_ascii_lowercase();
            value.starts_with("player_")
                || value.starts_with("server_")
                || value.contains("_server_")
        }
        PlayerId::Local | PlayerId::LegacyLevelDat => false,
    }
}

pub(super) fn player_friendly_label(id: &PlayerId, valid: bool) -> String {
    let raw = player_id_label(id);
    if !valid {
        return format!("无效记录 · {raw}");
    }
    match id {
        PlayerId::Local => "本地玩家 · ~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("服务器玩家 · {xuid}"),
        PlayerId::LegacyLevelDat => "旧版玩家 · level.dat".to_string(),
        PlayerId::Unknown(_) if is_server_like_player_id(id) => format!("服务器记录 · {raw}"),
        PlayerId::Unknown(_) => format!("其他玩家 · {raw}"),
    }
}

pub(super) fn preferred_player_id(players: &[PlayerSummary]) -> Option<PlayerId> {
    players
        .iter()
        .find(|player| matches!(&player.id, PlayerId::Local))
        .or_else(|| players.first())
        .map(|player| player.id.clone())
}

pub(super) fn player_id_label(id: &PlayerId) -> String {
    match id {
        PlayerId::Local => "~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("player_{xuid}"),
        PlayerId::LegacyLevelDat => "level.dat legacy player".to_string(),
        PlayerId::Unknown(value) => value.clone(),
    }
}

fn player_probe(id: &PlayerId, data: &PlayerData) -> Result<PlayerProbe, String> {
    let root = match &data.nbt {
        NbtTag::Compound(root) => root,
        _ => return Err("玩家 NBT 根节点不是 Compound".to_string()),
    };
    let position = nbt_vec3_f64(root.get("Pos"));
    let position = position.filter(|value| value.iter().all(|value| value.is_finite()));
    let dimension_id = nbt_i32_any(root.get("DimensionId"));
    let has_unique_id = nbt_i64(root.get("UniqueID")).is_some();
    let has_position = position.is_some();
    let has_dimension = dimension_id.is_some();
    let has_inventory = matches!(root.get("Inventory"), Some(NbtTag::List(_)));
    let entries = player_inventory_entries(&data.nbt);
    let item_count = entries.len();
    let ender_item_count = entries
        .iter()
        .filter(|entry| entry.kind == PlayerInventoryKind::EnderChest)
        .count();
    let container_count = ["Inventory", "Armor", "Offhand", "EnderChestInventory"]
        .into_iter()
        .filter(|key| matches!(root.get(*key), Some(NbtTag::List(_))))
        .count() as i16;
    let server_like = is_server_like_player_id(id);
    let trusted_server =
        server_like && has_unique_id && has_position && has_dimension && has_inventory;
    let completeness = (if has_unique_id { 1 } else { 0 })
        + (if has_position { 1 } else { 0 })
        + (if has_dimension { 1 } else { 0 })
        + (if has_inventory { 1 } else { 0 });
    let health =
        if (matches!(id, PlayerId::Local) && has_position && has_dimension && has_inventory)
            || trusted_server
            || (has_unique_id && has_position && has_dimension && has_inventory)
        {
            PlayerRecordHealth::Complete
        } else if completeness >= 2 || (has_inventory && item_count > 0) {
            PlayerRecordHealth::Partial
        } else {
            PlayerRecordHealth::Stub
        };
    let score = (if has_unique_id { 30 } else { 0 })
        + (if has_position { 25 } else { 0 })
        + (if has_dimension { 15 } else { 0 })
        + (if has_inventory { 25 } else { 0 })
        + container_count * 2
        + i16::try_from(item_count.min(10)).unwrap_or(10)
        + if ender_item_count > 0 { 5 } else { 0 };
    Ok(PlayerProbe {
        position,
        dimension_id,
        quality: PlayerRecordQuality {
            health,
            score,
            trusted_server,
            has_unique_id,
            has_position,
            has_dimension,
            has_inventory,
            item_count,
            ender_item_count,
        },
    })
}

pub(super) fn player_detail_from_data(data: PlayerData) -> Result<PlayerDetail, String> {
    let root = match &data.nbt {
        NbtTag::Compound(root) => root,
        _ => {
            return Err("玩家 NBT 根节点不是 Compound".to_string());
        }
    };
    let items = collect_inventory_items(&data.nbt);
    let json = pretty_json(
        serde_json::to_value(&data.nbt)
            .map_err(|error| format!("玩家 NBT 转 JSON 失败: {error}"))?,
    );
    Ok(PlayerDetail {
        id: data.id,
        unique_id: nbt_i64(root.get("UniqueID")),
        position: nbt_vec3_f64(root.get("Pos")),
        dimension_id: nbt_i32_any(root.get("DimensionId")),
        item_count: items.len(),
        items,
        nbt: data.nbt,
        json,
    })
}

pub(super) fn player_inventory_entries(tag: &NbtTag) -> Vec<PlayerInventoryEntry> {
    let Some(root) = nbt_compound(tag) else {
        return Vec::new();
    };
    let mut output = Vec::new();
    for kind in [
        PlayerInventoryKind::Inventory,
        PlayerInventoryKind::Armor,
        PlayerInventoryKind::Offhand,
        PlayerInventoryKind::EnderChest,
    ] {
        let Some(NbtTag::List(items)) = root.get(kind.nbt_key()) else {
            continue;
        };
        for (list_index, item) in items.iter().enumerate() {
            let Some(compound) = nbt_compound(item) else {
                continue;
            };
            let name = nbt_string_any(compound.get("Name"));
            if name.as_deref().is_none_or(|name| name.trim().is_empty()) {
                continue;
            }
            output.push(PlayerInventoryEntry {
                kind,
                list_index,
                slot: nbt_i32_any(compound.get("Slot")).or_else(|| {
                    (kind == PlayerInventoryKind::Inventory).then_some(list_index as i32)
                }),
                item: item_stack_from_compound(compound, item),
            });
        }
    }
    output.sort_by(|a, b| {
        inventory_kind_rank(a.kind)
            .cmp(&inventory_kind_rank(b.kind))
            .then_with(|| a.slot.unwrap_or(i32::MAX).cmp(&b.slot.unwrap_or(i32::MAX)))
            .then_with(|| a.list_index.cmp(&b.list_index))
    });
    output
}

fn inventory_kind_rank(kind: PlayerInventoryKind) -> u8 {
    match kind {
        PlayerInventoryKind::Inventory => 0,
        PlayerInventoryKind::Armor => 1,
        PlayerInventoryKind::Offhand => 2,
        PlayerInventoryKind::EnderChest => 3,
    }
}

pub(super) fn collect_inventory_items(tag: &NbtTag) -> Vec<bedrock_world::ItemStack> {
    player_inventory_entries(tag)
        .into_iter()
        .map(|entry| entry.item)
        .collect()
}

fn item_stack_from_compound(
    compound: &indexmap::IndexMap<String, NbtTag>,
    item: &NbtTag,
) -> bedrock_world::ItemStack {
    bedrock_world::ItemStack {
        name: nbt_string_any(compound.get("Name")),
        count: nbt_i32_any(compound.get("Count")),
        damage: nbt_i32_any(compound.get("Damage")),
        was_picked_up: nbt_bool_any(compound.get("WasPickedUp")),
        has_block: compound.contains_key("Block"),
        has_tag: compound.contains_key("tag"),
        nbt: item.clone(),
    }
}

pub(super) fn player_item_enchantments(tag: &NbtTag) -> Vec<PlayerEnchantEntry> {
    let Some(item) = nbt_compound(tag) else {
        return Vec::new();
    };
    let Some(NbtTag::Compound(user_tag)) = item.get("tag") else {
        return Vec::new();
    };
    let Some(NbtTag::List(enchantments)) = user_tag.get("ench") else {
        return Vec::new();
    };
    enchantments
        .iter()
        .enumerate()
        .filter_map(|(list_index, value)| {
            let compound = nbt_compound(value)?;
            let id = i16::try_from(nbt_i32_any(compound.get("id"))?).ok()?;
            let level = i16::try_from(nbt_i32_any(compound.get("lvl"))?).ok()?;
            Some(PlayerEnchantEntry {
                list_index,
                id,
                level,
            })
        })
        .collect()
}

pub(super) fn player_item_custom_name(tag: &NbtTag) -> Option<String> {
    let item = nbt_compound(tag)?;
    let NbtTag::Compound(user_tag) = item.get("tag")? else {
        return None;
    };
    let NbtTag::Compound(display) = user_tag.get("display")? else {
        return None;
    };
    nbt_string_any(display.get("Name"))
}

pub(super) fn player_item_lore_count(tag: &NbtTag) -> usize {
    let Some(item) = nbt_compound(tag) else {
        return 0;
    };
    let Some(NbtTag::Compound(user_tag)) = item.get("tag") else {
        return 0;
    };
    let Some(NbtTag::Compound(display)) = user_tag.get("display") else {
        return 0;
    };
    let Some(NbtTag::List(lines)) = display.get("Lore") else {
        return 0;
    };
    lines.len()
}

pub(super) fn localized_enchant_name(i18n: &I18n, id: i16) -> SharedString {
    let key = match id {
        0 => "MapViewer.enchant_protection",
        1 => "MapViewer.enchant_fire_protection",
        2 => "MapViewer.enchant_feather_falling",
        3 => "MapViewer.enchant_blast_protection",
        4 => "MapViewer.enchant_projectile_protection",
        5 => "MapViewer.enchant_thorns",
        6 => "MapViewer.enchant_respiration",
        7 => "MapViewer.enchant_depth_strider",
        8 => "MapViewer.enchant_aqua_affinity",
        9 => "MapViewer.enchant_sharpness",
        10 => "MapViewer.enchant_smite",
        11 => "MapViewer.enchant_bane_of_arthropods",
        12 => "MapViewer.enchant_knockback",
        13 => "MapViewer.enchant_fire_aspect",
        14 => "MapViewer.enchant_looting",
        15 => "MapViewer.enchant_efficiency",
        16 => "MapViewer.enchant_silk_touch",
        17 => "MapViewer.enchant_unbreaking",
        18 => "MapViewer.enchant_fortune",
        19 => "MapViewer.enchant_power",
        20 => "MapViewer.enchant_punch",
        21 => "MapViewer.enchant_flame",
        22 => "MapViewer.enchant_infinity",
        23 => "MapViewer.enchant_luck_of_the_sea",
        24 => "MapViewer.enchant_lure",
        25 => "MapViewer.enchant_frost_walker",
        26 => "MapViewer.enchant_mending",
        27 => "MapViewer.enchant_binding_curse",
        28 => "MapViewer.enchant_vanishing_curse",
        29 => "MapViewer.enchant_impaling",
        30 => "MapViewer.enchant_riptide",
        31 => "MapViewer.enchant_loyalty",
        32 => "MapViewer.enchant_channeling",
        33 => "MapViewer.enchant_multishot",
        34 => "MapViewer.enchant_piercing",
        35 => "MapViewer.enchant_quick_charge",
        36 => "MapViewer.enchant_soul_speed",
        37 => "MapViewer.enchant_swift_sneak",
        _ => return SharedString::from(format!("Unknown enchantment {id}")),
    };
    i18n.lookup(key)
        .unwrap_or_else(|| SharedString::from(format!("Unknown enchantment {id}")))
}

pub(super) fn localized_player_friendly_label(
    i18n: &I18n,
    id: &PlayerId,
    valid: bool,
) -> SharedString {
    if !valid {
        return t!("MapViewer.invalid_record", id = player_id_label(id));
    }
    match id {
        PlayerId::Local => t!("MapViewer.local_player"),
        PlayerId::Xuid(xuid) => {
            t!("MapViewer.server_player", id = xuid)
        }
        PlayerId::LegacyLevelDat => t!("MapViewer.legacy_player"),
        PlayerId::Unknown(_) if is_server_like_player_id(id) => {
            t!("MapViewer.server_record", id = player_id_label(id))
        }
        PlayerId::Unknown(_) => {
            t!("MapViewer.other_player", id = player_id_label(id))
        }
    }
}

pub(super) fn apply_player_quick_edit(
    tag: &mut NbtTag,
    edit: &PlayerQuickEdit,
    center_block: (i32, i32),
    dimension: Dimension,
) -> Result<(), String> {
    let root = player_root_mut(tag)?;
    match edit {
        PlayerQuickEdit::MoveToMapCenter => {
            root.insert(
                "Pos".to_string(),
                NbtTag::List(vec![
                    NbtTag::Double(f64::from(center_block.0) + 0.5),
                    NbtTag::Double(80.0),
                    NbtTag::Double(f64::from(center_block.1) + 0.5),
                ]),
            );
        }
        PlayerQuickEdit::SetDimension(target_dimension) => {
            root.insert(
                "DimensionId".to_string(),
                NbtTag::Int(target_dimension.id()),
            );
        }
        PlayerQuickEdit::ClearInventory => {
            if let Some(NbtTag::List(items)) = root.get_mut("Inventory") {
                for (index, item) in items.iter_mut().enumerate() {
                    let slot = nbt_compound(item)
                        .and_then(|compound| nbt_i32_any(compound.get("Slot")))
                        .unwrap_or(index as i32);
                    *item = empty_item_for_slot(slot);
                }
            } else {
                root.insert("Inventory".to_string(), NbtTag::List(Vec::new()));
            }
        }
    }
    if !matches!(edit, PlayerQuickEdit::SetDimension(_)) {
        root.entry("DimensionId".to_string())
            .or_insert_with(|| NbtTag::Int(dimension.id()));
    }
    Ok(())
}

pub(super) fn apply_player_item_mutation(
    tag: &mut NbtTag,
    mutation: &PlayerItemMutation,
) -> Result<(), String> {
    let root = player_root_mut(tag)?;
    match mutation {
        PlayerItemMutation::AddItem { kind, name } => {
            let list = inventory_list_mut(root, *kind)?;
            let slot = if *kind == PlayerInventoryKind::Inventory {
                next_free_slot(list, PLAYER_MAIN_INVENTORY_SIZE)
                    .ok_or_else(|| "主背包没有空槽位".to_string())?
            } else {
                next_free_slot(list, i32::from(i8::MAX))
                    .unwrap_or(i32::try_from(list.len()).unwrap_or(i32::from(i8::MAX)))
            };
            let item = new_item(name, slot)?;
            replace_empty_slot_or_push(list, slot, item);
        }
        PlayerItemMutation::DeleteItem { kind, list_index } => {
            let list = inventory_list_mut(root, *kind)?;
            let item = list
                .get(*list_index)
                .ok_or_else(|| "物品索引已失效，请刷新玩家数据".to_string())?;
            let slot = nbt_compound(item)
                .and_then(|compound| nbt_i32_any(compound.get("Slot")))
                .unwrap_or(*list_index as i32);
            list[*list_index] = empty_item_for_slot(slot);
        }
        PlayerItemMutation::DuplicateItem { kind, list_index } => {
            let list = inventory_list_mut(root, *kind)?;
            let mut cloned = list
                .get(*list_index)
                .cloned()
                .ok_or_else(|| "物品索引已失效，请刷新玩家数据".to_string())?;
            let slot = if *kind == PlayerInventoryKind::Inventory {
                next_free_slot(list, PLAYER_MAIN_INVENTORY_SIZE)
                    .ok_or_else(|| "主背包没有空槽位".to_string())?
            } else {
                next_free_slot(list, i32::from(i8::MAX))
                    .unwrap_or(i32::try_from(list.len()).unwrap_or(i32::from(i8::MAX)))
            };
            set_item_slot(&mut cloned, slot)?;
            replace_empty_slot_or_push(list, slot, cloned);
        }
        PlayerItemMutation::AdjustCount {
            kind,
            list_index,
            delta,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let current = nbt_i32_any(compound.get("Count")).unwrap_or(1);
            let value = current.saturating_add(*delta).clamp(1, i32::from(i8::MAX)) as i8;
            compound.insert("Count".to_string(), NbtTag::Byte(value));
        }
        PlayerItemMutation::SetCount {
            kind,
            list_index,
            value,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            item_compound_mut(item)?.insert("Count".to_string(), NbtTag::Byte((*value).max(1)));
        }
        PlayerItemMutation::AdjustDamage {
            kind,
            list_index,
            delta,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let current = nbt_i32_any(compound.get("Damage")).unwrap_or(0);
            let value = current
                .saturating_add(*delta)
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            compound.insert("Damage".to_string(), NbtTag::Short(value));
        }
        PlayerItemMutation::SetDamage {
            kind,
            list_index,
            value,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            item_compound_mut(item)?.insert("Damage".to_string(), NbtTag::Short(*value));
        }
        PlayerItemMutation::SetCustomName {
            kind,
            list_index,
            value,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let user_tag = nested_compound_mut(compound, "tag");
            let display = nested_compound_mut(user_tag, "display");
            display.insert("Name".to_string(), NbtTag::String(value.clone()));
        }
        PlayerItemMutation::SetLore {
            kind,
            list_index,
            lines,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let user_tag = nested_compound_mut(compound, "tag");
            let display = nested_compound_mut(user_tag, "display");
            display.insert(
                "Lore".to_string(),
                NbtTag::List(lines.iter().cloned().map(NbtTag::String).collect()),
            );
        }
        PlayerItemMutation::AddEnchant {
            kind,
            list_index,
            id,
            level,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let user_tag = nested_compound_mut(compound, "tag");
            let enchantments = nested_list_mut(user_tag, "ench");
            if let Some(existing) = enchantments.iter_mut().find(|value| {
                nbt_compound(value).and_then(|compound| nbt_i32_any(compound.get("id")))
                    == Some(i32::from(*id))
            }) {
                let existing = item_compound_mut(existing)?;
                existing.insert("lvl".to_string(), NbtTag::Short(*level));
            } else {
                enchantments.push(enchant_tag(*id, *level));
            }
        }
        PlayerItemMutation::AdjustEnchant {
            kind,
            list_index,
            enchant_index,
            delta,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let user_tag = nested_compound_mut(compound, "tag");
            let enchantments = nested_list_mut(user_tag, "ench");
            let enchantment = enchantments
                .get_mut(*enchant_index)
                .ok_or_else(|| "附魔索引已失效，请刷新玩家数据".to_string())?;
            let enchantment = item_compound_mut(enchantment)?;
            let current = nbt_i32_any(enchantment.get("lvl")).unwrap_or(1);
            let value = current
                .saturating_add(i32::from(*delta))
                .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            enchantment.insert("lvl".to_string(), NbtTag::Short(value));
        }
        PlayerItemMutation::RemoveEnchant {
            kind,
            list_index,
            enchant_index,
        } => {
            let item = inventory_item_mut(root, *kind, *list_index)?;
            let compound = item_compound_mut(item)?;
            let user_tag = nested_compound_mut(compound, "tag");
            let enchantments = nested_list_mut(user_tag, "ench");
            if *enchant_index >= enchantments.len() {
                return Err("附魔索引已失效，请刷新玩家数据".to_string());
            }
            enchantments.remove(*enchant_index);
        }
    }
    Ok(())
}

fn player_root_mut(tag: &mut NbtTag) -> Result<&mut indexmap::IndexMap<String, NbtTag>, String> {
    match tag {
        NbtTag::Compound(root) => Ok(root),
        _ => Err("玩家 NBT 根节点不是 Compound".to_string()),
    }
}

fn inventory_list_mut(
    root: &mut indexmap::IndexMap<String, NbtTag>,
    kind: PlayerInventoryKind,
) -> Result<&mut Vec<NbtTag>, String> {
    let key = kind.nbt_key().to_string();
    if !root.contains_key(&key) {
        root.insert(key.clone(), NbtTag::List(Vec::new()));
    }
    match root.get_mut(&key) {
        Some(NbtTag::List(items)) => Ok(items),
        Some(_) => Err(format!("玩家 `{}` 不是 NBT List", kind.nbt_key())),
        None => Err(format!("无法创建玩家 `{}`", kind.nbt_key())),
    }
}

fn inventory_item_mut(
    root: &mut indexmap::IndexMap<String, NbtTag>,
    kind: PlayerInventoryKind,
    list_index: usize,
) -> Result<&mut NbtTag, String> {
    inventory_list_mut(root, kind)?
        .get_mut(list_index)
        .ok_or_else(|| "物品索引已失效，请刷新玩家数据".to_string())
}

fn item_compound_mut(tag: &mut NbtTag) -> Result<&mut indexmap::IndexMap<String, NbtTag>, String> {
    match tag {
        NbtTag::Compound(compound) => Ok(compound),
        _ => Err("物品 NBT 不是 Compound".to_string()),
    }
}

fn nested_compound_mut<'a>(
    parent: &'a mut indexmap::IndexMap<String, NbtTag>,
    key: &str,
) -> &'a mut indexmap::IndexMap<String, NbtTag> {
    if !matches!(parent.get(key), Some(NbtTag::Compound(_))) {
        parent.insert(key.to_string(), NbtTag::Compound(indexmap::IndexMap::new()));
    }
    match parent.get_mut(key) {
        Some(NbtTag::Compound(compound)) => compound,
        _ => unreachable!("compound was inserted above"),
    }
}

fn nested_list_mut<'a>(
    parent: &'a mut indexmap::IndexMap<String, NbtTag>,
    key: &str,
) -> &'a mut Vec<NbtTag> {
    if !matches!(parent.get(key), Some(NbtTag::List(_))) {
        parent.insert(key.to_string(), NbtTag::List(Vec::new()));
    }
    match parent.get_mut(key) {
        Some(NbtTag::List(items)) => items,
        _ => unreachable!("list was inserted above"),
    }
}

fn next_free_slot(items: &[NbtTag], max_slots: i32) -> Option<i32> {
    let mut occupied = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let Some(compound) = nbt_compound(item) else {
            continue;
        };
        let Some(name) = nbt_string_any(compound.get("Name")) else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let slot = nbt_i32_any(compound.get("Slot")).unwrap_or(index as i32);
        occupied.insert(slot);
    }
    (0..max_slots).find(|slot| !occupied.contains(slot))
}

fn replace_empty_slot_or_push(items: &mut Vec<NbtTag>, slot: i32, item: NbtTag) {
    if let Some(index) = items.iter().position(|value| {
        let Some(compound) = nbt_compound(value) else {
            return false;
        };
        let value_slot = nbt_i32_any(compound.get("Slot"));
        let empty = nbt_string_any(compound.get("Name")).is_none_or(|name| name.trim().is_empty());
        empty && value_slot == Some(slot)
    }) {
        items[index] = item;
    } else {
        items.push(item);
    }
}

fn new_item(name: &str, slot: i32) -> Result<NbtTag, String> {
    let slot = i8::try_from(slot).map_err(|_| "物品槽位超出 NBT Byte 范围".to_string())?;
    let mut compound = indexmap::IndexMap::new();
    compound.insert("Name".to_string(), NbtTag::String(normalize_item_id(name)));
    compound.insert("Count".to_string(), NbtTag::Byte(1));
    compound.insert("Damage".to_string(), NbtTag::Short(0));
    compound.insert("Slot".to_string(), NbtTag::Byte(slot));
    compound.insert("WasPickedUp".to_string(), NbtTag::Byte(0));
    Ok(NbtTag::Compound(compound))
}

fn empty_item_for_slot(slot: i32) -> NbtTag {
    let mut compound = indexmap::IndexMap::new();
    compound.insert("Name".to_string(), NbtTag::String(String::new()));
    compound.insert("Count".to_string(), NbtTag::Byte(0));
    compound.insert("Damage".to_string(), NbtTag::Short(0));
    if let Ok(slot) = i8::try_from(slot) {
        compound.insert("Slot".to_string(), NbtTag::Byte(slot));
    }
    compound.insert("WasPickedUp".to_string(), NbtTag::Byte(0));
    NbtTag::Compound(compound)
}

fn set_item_slot(item: &mut NbtTag, slot: i32) -> Result<(), String> {
    let slot = i8::try_from(slot).map_err(|_| "物品槽位超出 NBT Byte 范围".to_string())?;
    item_compound_mut(item)?.insert("Slot".to_string(), NbtTag::Byte(slot));
    Ok(())
}

fn enchant_tag(id: i16, level: i16) -> NbtTag {
    let mut compound = indexmap::IndexMap::new();
    compound.insert("id".to_string(), NbtTag::Short(id));
    compound.insert("lvl".to_string(), NbtTag::Short(level));
    NbtTag::Compound(compound)
}

fn clipboard_text(cx: &mut Context<MapViewerWindowView>) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

fn parse_item_id(text: &str) -> Option<String> {
    let token = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .split_whitespace()
        .next()?
        .trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '`' | ',' | ';' | '[' | ']' | '{' | '}'
            )
        });
    if token.is_empty()
        || !token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_:-./".contains(character))
    {
        return None;
    }
    Some(normalize_item_id(token))
}

fn parse_enchant_spec(text: &str) -> Option<(i16, i16)> {
    let normalized = text.trim().replace('=', ":").replace(',', ":");
    let mut parts = normalized
        .split(|character: char| character == ':' || character.is_whitespace())
        .filter(|part| !part.is_empty());
    let id = parts.next()?.parse::<i16>().ok()?;
    let level = parts.next().unwrap_or("1").parse::<i16>().ok()?;
    Some((id, level))
}

fn normalize_item_id(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.contains(':') {
        value
    } else {
        format!("minecraft:{value}")
    }
}

fn cached_item_catalog(instance_root: &Path) -> Arc<Vec<PlayerItemTexture>> {
    static CACHE: OnceLock<Mutex<StdHashMap<PathBuf, Arc<Vec<PlayerItemTexture>>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(StdHashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(cached) = cache.get(instance_root)
    {
        return cached.clone();
    }

    let loaded = Arc::new(load_item_catalog(instance_root));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(instance_root.to_path_buf(), loaded.clone());
    }
    loaded
}

fn player_resource_pack_roots(instance_root: &Path) -> Vec<PathBuf> {
    let root = instance_root.join("data").join("resource_packs");
    let mut out = Vec::new();
    for name in ["vanilla", "chemistry"] {
        let p = root.join(name);
        if p.is_dir() {
            out.push(p);
        }
    }
    let mut versioned = Vec::<(String, PathBuf)>::new();
    if let Ok(entries) = fs::read_dir(&root) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|v| v.to_str()) else {
                continue;
            };
            if name.starts_with("vanilla_") || name.starts_with("chemistry_") {
                versioned.push((name.to_ascii_lowercase(), p));
            }
        }
    }
    versioned.sort_by(|a, b| {
        let af = if a.0.starts_with("vanilla_") { 0 } else { 1 };
        let bf = if b.0.starts_with("vanilla_") { 0 } else { 1 };
        af.cmp(&bf).then_with(|| b.0.cmp(&a.0))
    });
    out.extend(versioned.into_iter().map(|(_, p)| p));
    out
}
fn add_texture(
    by_id: &mut BTreeMap<String, PlayerItemTexture>,
    pack: &Path,
    key: &str,
    texture: &str,
) {
    let path = pack.join(format!("{texture}.png"));
    if !path.is_file() {
        return;
    }
    let id = normalize_item_id(key);
    by_id
        .entry(id.clone())
        .or_insert_with(|| PlayerItemTexture {
            id: SharedString::from(id),
            label: SharedString::from(key.replace('_', " ")),
            path: Arc::<Path>::from(path.into_boxed_path()),
        });
}
fn scan_flat_textures(by_id: &mut BTreeMap<String, PlayerItemTexture>, dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if !path
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("png"))
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|v| v.to_str()).map(str::to_owned) else {
            continue;
        };
        if stem.ends_with("_0") || stem.ends_with("_1") {
            continue;
        }
        let id = normalize_item_id(&stem);
        by_id
            .entry(id.clone())
            .or_insert_with(|| PlayerItemTexture {
                id: SharedString::from(id),
                label: SharedString::from(stem.replace('_', " ")),
                path: Arc::<Path>::from(path.into_boxed_path()),
            });
    }
}
fn load_item_catalog(instance_root: &Path) -> Vec<PlayerItemTexture> {
    let mut by_id = BTreeMap::<String, PlayerItemTexture>::new();
    for pack in player_resource_pack_roots(instance_root) {
        let textures = pack.join("textures");
        for atlas in ["item_texture.json", "terrain_texture.json"] {
            let file = textures.join(atlas);
            if let Ok(bytes) = fs::read(file)
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && let Some(data) = value.get("texture_data").and_then(|v| v.as_object())
            {
                for (key, entry) in data {
                    if let Some(texture) = texture_reference(entry) {
                        add_texture(&mut by_id, &pack, key, texture);
                    }
                }
            }
        }
        scan_flat_textures(&mut by_id, &textures.join("items"));
        scan_flat_textures(&mut by_id, &textures.join("blocks"));
    }
    by_id.into_values().collect()
}

fn texture_reference(value: &serde_json::Value) -> Option<&str> {
    if let Some(value) = value.as_str() {
        return Some(value);
    }
    let textures = value.get("textures")?;
    if let Some(value) = textures.as_str() {
        return Some(value);
    }
    textures
        .as_array()?
        .first()
        .and_then(serde_json::Value::as_str)
}

pub(super) fn nbt_compound(tag: &NbtTag) -> Option<&indexmap::IndexMap<String, NbtTag>> {
    match tag {
        NbtTag::Compound(values) => Some(values),
        _ => None,
    }
}

pub(super) fn nbt_i64(tag: Option<&NbtTag>) -> Option<i64> {
    match tag? {
        NbtTag::Byte(value) => Some(i64::from(*value)),
        NbtTag::Short(value) => Some(i64::from(*value)),
        NbtTag::Int(value) => Some(i64::from(*value)),
        NbtTag::Long(value) => Some(*value),
        _ => None,
    }
}

pub(super) fn nbt_i32_any(tag: Option<&NbtTag>) -> Option<i32> {
    match tag? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

pub(super) fn nbt_bool_any(tag: Option<&NbtTag>) -> Option<bool> {
    match tag? {
        NbtTag::Byte(value) => Some(*value != 0),
        NbtTag::Int(value) => Some(*value != 0),
        _ => None,
    }
}

pub(super) fn nbt_string_any(tag: Option<&NbtTag>) -> Option<String> {
    match tag? {
        NbtTag::String(value) => Some(value.clone()),
        _ => None,
    }
}

pub(super) fn nbt_vec3_f64(tag: Option<&NbtTag>) -> Option<[f64; 3]> {
    let NbtTag::List(values) = tag? else {
        return None;
    };
    if values.len() < 3 {
        return None;
    }
    Some([
        nbt_number_f64(&values[0])?,
        nbt_number_f64(&values[1])?,
        nbt_number_f64(&values[2])?,
    ])
}

pub(super) fn nbt_number_f64(tag: &NbtTag) -> Option<f64> {
    match tag {
        NbtTag::Byte(value) => Some(f64::from(*value)),
        NbtTag::Short(value) => Some(f64::from(*value)),
        NbtTag::Int(value) => Some(f64::from(*value)),
        NbtTag::Long(value) => Some(*value as f64),
        NbtTag::Float(value) => Some(f64::from(*value)),
        NbtTag::Double(value) => Some(*value),
        _ => None,
    }
}

pub(super) fn player_detail_grid(colors: &ThemeColors, detail: &PlayerDetail) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(6.0))
        .child(status_badge(colors, player_id_label(&detail.id)))
        .child(status_badge(
            colors,
            detail
                .unique_id
                .map_or_else(|| "UID unknown".to_string(), |value| format!("UID {value}")),
        ))
        .child(status_badge(
            colors,
            detail.position.map_or_else(
                || "Pos unknown".to_string(),
                |position| {
                    format!(
                        "Pos {:.1}, {:.1}, {:.1}",
                        position[0], position[1], position[2]
                    )
                },
            ),
        ))
        .child(status_badge(
            colors,
            detail
                .dimension_id
                .map_or_else(|| "Dim unknown".to_string(), |value| format!("Dim {value}")),
        ))
}

pub(super) fn player_inventory_summary(detail: &PlayerDetail) -> String {
    let entries = player_inventory_entries(&detail.nbt);
    let mut main = 0usize;
    let mut armor = 0usize;
    let mut offhand = 0usize;
    let mut ender = 0usize;
    for entry in entries {
        match entry.kind {
            PlayerInventoryKind::Inventory => main += 1,
            PlayerInventoryKind::Armor => armor += 1,
            PlayerInventoryKind::Offhand => offhand += 1,
            PlayerInventoryKind::EnderChest => ender += 1,
        }
    }
    format!("背包 {main} · 护甲 {armor} · 副手 {offhand} · 末影箱 {ender}")
}

pub(super) fn render_player_item_row(
    colors: &ThemeColors,
    index: usize,
    item: &bedrock_world::ItemStack,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.20,
            ..colors.surface_hover
        })
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .text_size(px(12.0))
                .text_color(colors.text_primary)
                .child(format!(
                    "#{} {}",
                    index,
                    item.name.as_deref().unwrap_or("unknown")
                )),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(colors.text_muted)
                .child(format!(
                    "x{} dmg {}{}{}{}",
                    item.count
                        .map_or_else(|| "?".to_string(), |value| value.to_string()),
                    item.damage
                        .map_or_else(|| "?".to_string(), |value| value.to_string()),
                    if item.was_picked_up == Some(true) {
                        " picked"
                    } else {
                        ""
                    },
                    if item.has_block { " block" } else { "" },
                    if item.has_tag { " tag" } else { "" },
                )),
        )
}

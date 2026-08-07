use super::editor::*;
use super::map_history::MapHistoryCapture;
use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::viewport::viewport_screen_for_block;
use std::collections::HashMap as StdHashMap;
use std::fs;

const PLAYER_CLICK_HIT_RADIUS_PX: f32 = 22.0;
const PLAYER_CLICK_DRAG_THRESHOLD_PX: f32 = 4.0;
const PLAYER_MAIN_INVENTORY_SIZE: i32 = 36;
const PLAYER_ITEM_CATALOG_LIMIT: usize = 96;

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
        if self.players.generation == 0 {
            self.install_player_canvas_click_hook(cx);
        }

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
                        bedrock_world::OpenOptions::default(),
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
                                player_probe(&data)
                            });

                        match probe {
                            Ok((position, dimension_id)) => {
                                let label = SharedString::from(player_friendly_label(&id, true));
                                let rank = player_sort_rank(&id, true);
                                if let (Some(position), Some(dimension_id)) =
                                    (position, dimension_id)
                                {
                                    if position[0].is_finite() && position[2].is_finite() {
                                        marker_records.push(PlayerRefreshMarker {
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
                                rows.push((rank, raw_label, PlayerSummary { id, label }));
                            }
                            Err(_) => {
                                rows.push((
                                    player_sort_rank(&id, false),
                                    raw_label.clone(),
                                    PlayerSummary {
                                        id,
                                        label: SharedString::from(format!(
                                            "无效记录 · {raw_label}"
                                        )),
                                    },
                                ));
                            }
                        }
                    }

                    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                    let players = rows.into_iter().map(|(_, _, player)| player).collect();
                    let mut markers: BTreeMap<Dimension, Vec<Marker>> = BTreeMap::new();
                    for marker in marker_records {
                        markers.entry(marker.dimension).or_default().push(Marker {
                            x: marker.x,
                            z: marker.z,
                            label: marker.label,
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
                            this.players.selected =
                                this.players.players.first().map(|player| player.id.clone());
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
                        if let Some(id) = this.players.selected.clone() {
                            this.load_player_detail(id, cx);
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

    fn install_player_canvas_click_hook(&mut self, cx: &mut Context<Self>) {
        let canvas = self.canvas_view.clone();
        let mut click_start: Option<Point<Pixels>> = None;
        let mut players_were_active = false;
        let subscription = cx.subscribe(
            &canvas,
            move |this, _canvas, action: &MapCanvasAction, cx| match *action {
                MapCanvasAction::BeginDrag(position) => {
                    click_start = Some(position);
                    players_were_active = this.ui_state.bottom_panel_open
                        && this.ui_state.active_bottom_tab == MapViewerBottomTab::Players;
                }
                MapCanvasAction::EndDrag(position) => {
                    let Some(start) = click_start.take() else {
                        players_were_active = false;
                        return;
                    };
                    let was_players = players_were_active;
                    players_were_active = false;
                    let dx = (position.x - start.x) / px(1.0);
                    let dy = (position.y - start.y) / px(1.0);
                    if !was_players || dx.hypot(dy) > PLAYER_CLICK_DRAG_THRESHOLD_PX {
                        return;
                    }
                    let Some(id) = this.player_at_canvas_position(position) else {
                        return;
                    };

                    this.ui_state.active_bottom_tab = MapViewerBottomTab::Players;
                    this.ui_state.bottom_panel_open = true;
                    this.load_player_detail(id, cx);
                }
                _ => {}
            },
        );
        self._subscriptions.push(subscription);
    }

    fn player_at_canvas_position(&self, position: Point<Pixels>) -> Option<PlayerId> {
        let markers = self.markers.get(&self.dimension)?;
        let x = position.x / px(1.0);
        let y = position.y / px(1.0);
        let radius2 = PLAYER_CLICK_HIT_RADIUS_PX * PLAYER_CLICK_HIT_RADIUS_PX;
        let mut best: Option<(f32, &Marker)> = None;
        for marker in markers {
            let Some((screen_x, screen_y)) =
                viewport_screen_for_block(self.viewport, self.active_layout, marker.x, marker.z)
            else {
                continue;
            };
            let dx = screen_x - x;
            let dy = screen_y - y;
            let distance2 = dx * dx + dy * dy;
            if distance2 > radius2 {
                continue;
            }
            if best.is_none_or(|(best_distance, _)| distance2 < best_distance) {
                best = Some((distance2, marker));
            }
        }

        let (_, marker) = best?;
        self.players
            .players
            .iter()
            .find(|player| player.label == marker.label)
            .map(|player| player.id.clone())
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
                        bedrock_world::OpenOptions::default(),
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
        let Some(id) = self.players.selected.clone() else {
            self.status = SharedString::from("请先选择玩家记录");
            cx.notify();
            return;
        };
        if self.players.saving {
            self.status = SharedString::from("上一项玩家写入尚未完成");
            cx.notify();
            return;
        }

        let label = mutation.history_label();
        self.players.pending_save_confirmation = None;
        self.players.saving = true;
        self.players.generation = self.players.generation.saturating_add(1);
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        self.status = SharedString::from(format!("正在{label}..."));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut options = bedrock_world::OpenOptions::default();
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
                        this.status = SharedString::from("玩家物品已写入 · 可在历史面板撤销");
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
        let Some(id) = self.players.selected.clone() else {
            self.status = SharedString::from("请先选择玩家记录");
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
            self.status = SharedString::from(format!("再次点击以确认{}", edit.label()));
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
        self.status = SharedString::from(format!("正在{}...", edit.label()));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut options = bedrock_world::OpenOptions::default();
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
                        this.status = SharedString::from("玩家记录已写入");
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

fn capture_player_history(
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

fn player_sort_rank(id: &PlayerId, valid: bool) -> u8 {
    if !valid {
        return 3;
    }
    match id {
        PlayerId::Local => 0,
        PlayerId::Xuid(_) => 1,
        PlayerId::LegacyLevelDat | PlayerId::Unknown(_) => 2,
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
        PlayerId::Unknown(_) => format!("其他玩家 · {raw}"),
    }
}

pub(super) fn player_id_label(id: &PlayerId) -> String {
    match id {
        PlayerId::Local => "~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("player_{xuid}"),
        PlayerId::LegacyLevelDat => "level.dat legacy player".to_string(),
        PlayerId::Unknown(value) => value.clone(),
    }
}

fn player_probe(data: &PlayerData) -> Result<(Option<[f64; 3]>, Option<i32>), String> {
    let root = match &data.nbt {
        NbtTag::Compound(root) => root,
        _ => return Err("玩家 NBT 根节点不是 Compound".to_string()),
    };
    Ok((
        nbt_vec3_f64(root.get("Pos")),
        nbt_i32_any(root.get("DimensionId")),
    ))
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

pub(super) fn enchant_name(id: i16) -> &'static str {
    match id {
        0 => "保护",
        1 => "火焰保护",
        2 => "摔落保护",
        3 => "爆炸保护",
        4 => "弹射物保护",
        5 => "荆棘",
        6 => "水下呼吸",
        7 => "深海探索者",
        8 => "水下速掘",
        9 => "锋利",
        10 => "亡灵杀手",
        11 => "节肢杀手",
        12 => "击退",
        13 => "火焰附加",
        14 => "抢夺",
        15 => "效率",
        16 => "精准采集",
        17 => "耐久",
        18 => "时运",
        19 => "力量",
        20 => "冲击",
        21 => "火矢",
        22 => "无限",
        23 => "海之眷顾",
        24 => "饵钓",
        25 => "冰霜行者",
        26 => "经验修补",
        27 => "绑定诅咒",
        28 => "消失诅咒",
        29 => "穿刺",
        30 => "激流",
        31 => "忠诚",
        32 => "引雷",
        33 => "多重射击",
        34 => "穿透",
        35 => "快速装填",
        36 => "灵魂疾行",
        37 => "迅捷潜行",
        _ => "自定义/新版附魔",
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

fn load_item_catalog(instance_root: &Path) -> Vec<PlayerItemTexture> {
    let vanilla_root = instance_root
        .join("data")
        .join("resource_packs")
        .join("vanilla");
    let item_dir = vanilla_root.join("textures").join("items");
    let mut by_id = BTreeMap::<String, PlayerItemTexture>::new();

    let item_texture_json = vanilla_root.join("textures").join("item_texture.json");
    if let Ok(bytes) = fs::read(&item_texture_json)
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(texture_data) = value
            .get("texture_data")
            .and_then(|value| value.as_object())
    {
        for (key, entry) in texture_data {
            let Some(texture) = texture_reference(entry) else {
                continue;
            };
            let path = vanilla_root.join(format!("{texture}.png"));
            if !path.is_file() {
                continue;
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
    }

    if let Ok(entries) = fs::read_dir(&item_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_png = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("png"));
            if !is_png {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let stem = stem.to_owned();
            if stem.ends_with("_0") || stem.ends_with("_1") {
                continue;
            }
            let id = normalize_item_id(stem);
            by_id
                .entry(id.clone())
                .or_insert_with(|| PlayerItemTexture {
                    id: SharedString::from(id),
                    label: SharedString::from(stem.replace('_', " ")),
                    path: Arc::<Path>::from(path.into_boxed_path()),
                });
        }
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

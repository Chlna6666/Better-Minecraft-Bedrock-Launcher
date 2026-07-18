use super::editor::*;
use super::model::*;
use super::panels::*;
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn refresh_players(&mut self, cx: &mut Context<Self>) {
        self.players.generation = self.players.generation.saturating_add(1);
        self.players.loading = true;
        self.players.error = None;
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        self.status = SharedString::from("正在读取玩家列表...");
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
                    world
                        .list_players_blocking()
                        .map_err(|error| error.to_string())
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
                    Ok(players) => {
                        this.players.players = players
                            .into_iter()
                            .map(|id| PlayerSummary {
                                label: SharedString::from(player_id_label(&id)),
                                id,
                            })
                            .collect();
                        if this.players.selected.is_none() {
                            this.players.selected =
                                this.players.players.first().map(|player| player.id.clone());
                        }
                        this.status = SharedString::from(format!(
                            "玩家列表已加载 · {} 条记录",
                            this.players.players.len()
                        ));
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
        cx.notify();
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
                    let mut raw_keys = BTreeSet::new();
                    let mut include_level_dat = false;
                    if let Some(key) = id.storage_key() {
                        raw_keys.insert(key.as_ref().to_vec());
                    } else {
                        include_level_dat = true;
                    }
                    let history_capture = capture_before(MapHistoryCaptureSpec {
                        kind: MapHistoryEntryKind::PlayerEdit,
                        label: edit.label(),
                        world_path: world_path.clone(),
                        chunks: BTreeSet::new(),
                        raw_keys,
                        include_level_dat,
                    });
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
                        this.players.detail = Some(detail);
                        this.status = SharedString::from("玩家记录已写入");
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
}

pub(super) fn player_id_label(id: &PlayerId) -> String {
    match id {
        PlayerId::Local => "~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("player_{xuid}"),
        PlayerId::LegacyLevelDat => "level.dat legacy player".to_string(),
        PlayerId::Unknown(value) => value.clone(),
    }
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
    let detail = PlayerDetail {
        id: data.id,
        unique_id: nbt_i64(root.get("UniqueID")),
        position: nbt_vec3_f64(root.get("Pos")),
        dimension_id: nbt_i32_any(root.get("DimensionId")),
        item_count: items.len(),
        items,
        nbt: data.nbt,
        json,
    };
    Ok(detail)
}

pub(super) fn collect_inventory_items(tag: &NbtTag) -> Vec<bedrock_world::ItemStack> {
    let Some(root) = nbt_compound(tag) else {
        return Vec::new();
    };
    let Some(NbtTag::List(items)) = root.get("Inventory") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let compound = nbt_compound(item)?;
            Some(bedrock_world::ItemStack {
                name: nbt_string_any(compound.get("Name")),
                count: nbt_i32_any(compound.get("Count")),
                damage: nbt_i32_any(compound.get("Damage")),
                was_picked_up: nbt_bool_any(compound.get("WasPickedUp")),
                has_block: compound.contains_key("Block"),
                has_tag: compound.contains_key("tag"),
                nbt: item.clone(),
            })
        })
        .collect()
}

pub(super) fn apply_player_quick_edit(
    tag: &mut NbtTag,
    edit: &PlayerQuickEdit,
    center_block: (i32, i32),
    dimension: Dimension,
) -> Result<(), String> {
    let root = match tag {
        NbtTag::Compound(root) => root,
        _ => return Err("玩家 NBT 根节点不是 Compound".to_string()),
    };
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
            root.insert("Inventory".to_string(), NbtTag::List(Vec::new()));
        }
    }
    if !matches!(edit, PlayerQuickEdit::SetDimension(_)) {
        root.entry("DimensionId".to_string())
            .or_insert_with(|| NbtTag::Int(dimension.id()));
    }
    Ok(())
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
        .rounded(px(6.0))
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

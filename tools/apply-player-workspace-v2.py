from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def write(path, text):
    (ROOT / path).write_text(text, encoding="utf-8", newline="\n")


def replace_once(path, old, new):
    text = read(path)
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, got {count}: {old[:120]!r}")
    write(path, text.replace(old, new, 1))


# Module and shared imports.
replace_once(
    "src/ui/window/map_viewer.rs",
    "mod player_panel;\nmod players;",
    "mod player_panel;\nmod player_workspace;\nmod players;",
)
replace_once(
    "src/ui/window/map_viewer/prelude.rs",
    "pub(super) use super::model::ChunkTransferProgress;",
    "pub(super) use super::model::ChunkTransferProgress;\n"
    "pub(super) use super::player_workspace::{\n"
    "    PlayerInspectorMode, PlayerItemSelection, PlayerWorkspaceCenter, PlayerWorkspaceState,\n"
    "    player_workspace_subscriptions,\n"
    "};",
)
replace_once(
    "src/ui/window/map_viewer/prelude.rs",
    "    EditorDocument, MIN_CENTER_HEIGHT, MIN_CENTER_WIDTH, MapViewerBottomTab, MapViewerRightPanel,\n"
    "    MapViewerUiState, chunk_tree_nodes_for_tile, clamp_bottom_panel_height,",
    "    EditorDocument, MIN_CENTER_HEIGHT, MIN_CENTER_WIDTH, MapViewerBottomTab, MapViewerLeftPanel,\n"
    "    MapViewerRightPanel, MapViewerUiState, chunk_tree_nodes_for_tile, clamp_bottom_panel_height,",
)

# IDE state: contextual left dock, no bottom Players tab, player inspector right dock.
state = read("src/ui/window/map_viewer/state.rs")
state = state.replace("pub const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 420.0;", "pub const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 460.0;")
state = state.replace("pub const RIGHT_PANEL_MIN_WIDTH: f32 = 300.0;", "pub const RIGHT_PANEL_MIN_WIDTH: f32 = 340.0;")
old = '''#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapViewerBottomTab {
    ChunkTree,
    Players,
    Details,
    Diagnostics,
    History,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MapViewerRightPanel {
    #[default]
    Nbt,
    Preview3d,
}'''
new = '''#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MapViewerLeftPanel {
    #[default]
    Tools,
    Players,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapViewerBottomTab {
    ChunkTree,
    Details,
    Diagnostics,
    History,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MapViewerRightPanel {
    #[default]
    Nbt,
    Player,
    Preview3d,
}'''
if old not in state:
    raise SystemExit("state enums anchor missing")
state = state.replace(old, new, 1)
state = state.replace(
    "    pub active_bottom_tab: MapViewerBottomTab,\n    pub active_right_panel: MapViewerRightPanel,",
    "    pub active_left_panel: MapViewerLeftPanel,\n    pub active_bottom_tab: MapViewerBottomTab,\n    pub active_right_panel: MapViewerRightPanel,",
    1,
)
state = state.replace(
    "            active_bottom_tab: MapViewerBottomTab::ChunkTree,\n            active_right_panel: MapViewerRightPanel::Nbt,",
    "            active_left_panel: MapViewerLeftPanel::Tools,\n            active_bottom_tab: MapViewerBottomTab::ChunkTree,\n            active_right_panel: MapViewerRightPanel::Nbt,",
    1,
)
write("src/ui/window/map_viewer/state.rs", state)

# Actions can choose which contextual left dock is registered/active.
replace_once(
    "src/ui/window/map_viewer/actions.rs",
    "use super::state::{MapViewerBottomTab, MapViewerRightPanel};",
    "use super::state::{MapViewerBottomTab, MapViewerLeftPanel, MapViewerRightPanel};",
)
replace_once(
    "src/ui/window/map_viewer/actions.rs",
    "    ToggleLeftPanel,\n    ToggleBottomTab(MapViewerBottomTab),",
    "    ToggleLeftPanel,\n    ToggleLeftPanelKind(MapViewerLeftPanel),\n    ToggleBottomTab(MapViewerBottomTab),",
)

# Public function stripe keeps its role, but Tools/Players select registered left-dock content.
tool = read("src/ui/window/map_viewer/tool_stripe.rs")
tool = tool.replace(
    "use super::state::{MapViewerBottomTab, MapViewerRightPanel};",
    "use super::state::{MapViewerBottomTab, MapViewerLeftPanel, MapViewerRightPanel};",
    1,
)
tool = tool.replace(
    "    pub bottom_panel_open: bool,\n    pub active_bottom_tab: MapViewerBottomTab,",
    "    pub bottom_panel_open: bool,\n    pub active_left_panel: MapViewerLeftPanel,\n    pub active_bottom_tab: MapViewerBottomTab,",
    1,
)
tool = tool.replace(
    "            bottom_panel_open: false,\n            active_bottom_tab: MapViewerBottomTab::ChunkTree,",
    "            bottom_panel_open: false,\n            active_left_panel: MapViewerLeftPanel::Tools,\n            active_bottom_tab: MapViewerBottomTab::ChunkTree,",
    1,
)
tool = tool.replace(
    '''                snapshot.left_panel_open,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleLeftPanel);
                }),''',
    '''                snapshot.left_panel_open
                    && snapshot.active_left_panel == MapViewerLeftPanel::Tools,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleLeftPanelKind(MapViewerLeftPanel::Tools));
                }),''',
    1,
)
old_players = '''                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::Players,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::Players,
                    ));
                }),'''
new_players = '''                snapshot.left_panel_open
                    && snapshot.active_left_panel == MapViewerLeftPanel::Players,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleLeftPanelKind(MapViewerLeftPanel::Players));
                }),'''
if old_players not in tool:
    raise SystemExit("tool stripe players anchor missing")
tool = tool.replace(old_players, new_players, 1)
write("src/ui/window/map_viewer/tool_stripe.rs", tool)

# Model: summary carries trust/completeness, window owns player workspace UI state.
replace_once(
    "src/ui/window/map_viewer/model.rs",
    "use super::panels::*;\nuse super::players::*;",
    "use super::panels::*;\nuse super::player_workspace::*;\nuse super::players::*;",
)
replace_once(
    "src/ui/window/map_viewer/model.rs",
    '''pub(super) struct PlayerSummary {
    pub(super) id: PlayerId,
    pub(super) label: SharedString,
}''',
    '''pub(super) struct PlayerSummary {
    pub(super) id: PlayerId,
    pub(super) label: SharedString,
    pub(super) quality: PlayerRecordQuality,
}''',
)
replace_once(
    "src/ui/window/map_viewer/model.rs",
    "    pub(super) history: MapHistoryState,\n    pub(super) players: PlayerPanelState,\n    pub(super) preview_3d: Preview3dState,",
    "    pub(super) history: MapHistoryState,\n    pub(super) players: PlayerPanelState,\n    pub(super) player_workspace: PlayerWorkspaceState,\n    pub(super) preview_3d: Preview3dState,",
)

# Lifecycle constructs/subscribes player workspace without touching retained tile state.
life = read("src/ui/window/map_viewer/lifecycle.rs")
life = life.replace(
    '''        let editor_state = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor
        });
        let mut subscriptions = vec![cx.observe_window_bounds(window, |this, window, cx| {''',
    '''        let editor_state = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor
        });
        let player_workspace = PlayerWorkspaceState::new(window, cx);
        let mut subscriptions = vec![cx.observe_window_bounds(window, |this, window, cx| {''',
    1,
)
life = life.replace(
    "        subscriptions.extend(map_input_subscriptions(&input_fields, cx));",
    "        subscriptions.extend(map_input_subscriptions(&input_fields, cx));\n        subscriptions.extend(player_workspace_subscriptions(&player_workspace, cx));",
    1,
)
life = life.replace(
    "            history: MapHistoryState::default(),\n            players: PlayerPanelState::default(),\n            preview_3d: Preview3dState::default(),",
    "            history: MapHistoryState::default(),\n            players: PlayerPanelState::default(),\n            player_workspace,\n            preview_3d: Preview3dState::default(),",
    1,
)
write("src/ui/window/map_viewer/lifecycle.rs", life)

# Workspace render: contextual left dock and player inventory can temporarily occupy center canvas
# while retained map tiles remain alive in RegionManager/MapCanvasView.
panels = read("src/ui/window/map_viewer/panels.rs")
panels = panels.replace(
    "            bottom_panel_open: self.ui_state.bottom_panel_open,\n            active_bottom_tab: self.ui_state.active_bottom_tab,",
    "            bottom_panel_open: self.ui_state.bottom_panel_open,\n            active_left_panel: self.ui_state.active_left_panel,\n            active_bottom_tab: self.ui_state.active_bottom_tab,",
    1,
)
panels = panels.replace(
    "                    .flex()\n                    .flex_col()\n                    .child(self.canvas_view.clone()),",
    '''                    .flex()
                    .flex_col()
                    .child(if self.player_workspace_active()
                        && self.player_workspace.center != PlayerWorkspaceCenter::Map
                    {
                        self.render_player_center_workspace(colors, cx).into_any_element()
                    } else {
                        self.canvas_view.clone().into_any_element()
                    }),''',
    1,
)
old_sig = '''    pub(super) fn render_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()'''
new_sig = '''    pub(super) fn render_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.ui_state.active_left_panel {
            MapViewerLeftPanel::Tools => self.render_tools_left_dock(colors, cx).into_any_element(),
            MapViewerLeftPanel::Players => self.render_player_left_dock(colors, cx).into_any_element(),
        }
    }

    fn render_tools_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()'''
if old_sig not in panels:
    raise SystemExit("panels left dock anchor missing")
panels = panels.replace(old_sig, new_sig, 1)
write("src/ui/window/map_viewer/panels.rs", panels)

# Bottom dock no longer owns Players; keep data/diagnostics/history there.
bottom = read("src/ui/window/map_viewer/bottom_panel.rs")
bottom = bottom.replace(
    '''                MapViewerBottomTab::Players => {
                    self.render_players_panel(colors, cx).into_any_element()
                }
''',
    "",
    1,
)
bottom = bottom.replace(
    "        let tabs: [(&'static str, &'static str, MapViewerBottomTab); 5] = [",
    "        let tabs: [(&'static str, &'static str, MapViewerBottomTab); 4] = [",
    1,
)
bottom = bottom.replace(
    '''            (
                lucide_icons::icon_users(),
                "玩家",
                MapViewerBottomTab::Players,
            ),
''',
    "",
    1,
)
write("src/ui/window/map_viewer/bottom_panel.rs", bottom)

# Right dock adds dedicated player/item inspector while global NBT remains independent.
right = read("src/ui/window/map_viewer/right_panel.rs")
right = right.replace(
    '''            .child(match self.ui_state.active_right_panel {
                MapViewerRightPanel::Nbt => self.render_nbt_right_panel(colors, cx),
                MapViewerRightPanel::Preview3d => {
                    self.render_preview_3d_panel(colors, cx).into_any_element()
                }
            })''',
    '''            .child(match self.ui_state.active_right_panel {
                MapViewerRightPanel::Nbt => self.render_nbt_right_panel(colors, cx),
                MapViewerRightPanel::Player => self.render_player_right_panel(colors, cx),
                MapViewerRightPanel::Preview3d => {
                    self.render_preview_3d_panel(colors, cx).into_any_element()
                }
            })''',
    1,
)
write("src/ui/window/map_viewer/right_panel.rs", right)

# Interactions wire contextual Players dock and player right inspector.
inter = read("src/ui/window/map_viewer/interactions.rs")
inter = inter.replace(
    "            MapViewerAction::ToggleLeftPanel => self.toggle_left_panel(cx),\n            MapViewerAction::ToggleBottomTab(tab) => self.toggle_bottom_tab(tab, cx),",
    "            MapViewerAction::ToggleLeftPanel => self.toggle_left_panel(cx),\n            MapViewerAction::ToggleLeftPanelKind(panel) => self.toggle_left_panel_kind(panel, cx),\n            MapViewerAction::ToggleBottomTab(tab) => self.toggle_bottom_tab(tab, cx),",
    1,
)
anchor = '''    pub(super) fn toggle_left_panel(&mut self, cx: &mut Context<Self>) {
        self.ui_state.left_panel_open = !self.ui_state.left_panel_open;
        let size = size(px(self.window_width), px(self.window_height));
        if self.viewport.set_size(self.center_stage_size(size)) {
            self.invalidate_professional_overlay_for_viewport_change();
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches(cx);
            self.refresh_professional_overlays(cx);
        }
        cx.notify();
    }
'''
insert = anchor + '''
    pub(super) fn toggle_left_panel_kind(
        &mut self,
        panel: MapViewerLeftPanel,
        cx: &mut Context<Self>,
    ) {
        if self.ui_state.left_panel_open && self.ui_state.active_left_panel == panel {
            self.toggle_left_panel(cx);
            return;
        }
        self.ui_state.active_left_panel = panel;
        self.ui_state.left_panel_open = true;
        if panel == MapViewerLeftPanel::Players {
            if self.players.players.is_empty() {
                self.refresh_players(cx);
            }
            if self.players.selected.is_some() {
                self.ui_state.active_right_panel = MapViewerRightPanel::Player;
                self.ui_state.set_right_panel_open(true);
            }
        } else if self.ui_state.active_right_panel == MapViewerRightPanel::Player {
            self.ui_state.set_right_panel_open(false);
        }
        self.update_viewport_after_dock_change(cx);
        cx.notify();
    }
'''
if inter.count(anchor) != 1:
    raise SystemExit("toggle_left_panel anchor missing")
inter = inter.replace(anchor, insert, 1)
inter = inter.replace(
    '''        if tab == MapViewerBottomTab::Players && self.players.players.is_empty() {
            self.refresh_players(cx);
        }
''',
    "",
    1,
)
inter = inter.replace(
    '''    pub(super) fn open_right_preview_3d_panel(&mut self, cx: &mut Context<Self>) {
        self.show_right_preview_3d_panel(cx);
        cx.notify();
    }
''',
    '''    pub(super) fn open_right_player_panel(&mut self, cx: &mut Context<Self>) {
        if self.ui_state.active_right_panel == MapViewerRightPanel::Preview3d {
            self.clear_preview_3d_resources(false);
        }
        self.ui_state.active_right_panel = MapViewerRightPanel::Player;
        self.ui_state.set_right_panel_open(true);
        self.update_viewport_after_dock_change(cx);
        cx.notify();
    }

    pub(super) fn open_right_preview_3d_panel(&mut self, cx: &mut Context<Self>) {
        self.show_right_preview_3d_panel(cx);
        cx.notify();
    }
''',
    1,
)
inter = inter.replace(
    '''        match panel {
            MapViewerRightPanel::Nbt => self.open_right_nbt_panel(cx),
            MapViewerRightPanel::Preview3d => self.open_right_preview_3d_panel(cx),
        }''',
    '''        match panel {
            MapViewerRightPanel::Nbt => self.open_right_nbt_panel(cx),
            MapViewerRightPanel::Player => self.open_right_player_panel(cx),
            MapViewerRightPanel::Preview3d => self.open_right_preview_3d_panel(cx),
        }''',
    1,
)
write("src/ui/window/map_viewer/interactions.rs", inter)

# Player trust/completeness scoring and marker gating.
players = read("src/ui/window/map_viewer/players.rs")
insert_after = '''const PLAYER_MAIN_INVENTORY_SIZE: i32 = 36;
const PLAYER_ITEM_CATALOG_LIMIT: usize = 96;
'''
quality = '''const PLAYER_MAIN_INVENTORY_SIZE: i32 = 36;
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
            score: i16::MIN,
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
'''
if players.count(insert_after) != 1:
    raise SystemExit("players constants anchor missing")
players = players.replace(insert_after, quality, 1)
# Replace refresh probe and row logic.
old = '''                        let probe = world
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
                        }'''
new = '''                        let probe = world
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
                                        label: SharedString::from(format!("无效记录 · {raw_label}")),
                                        quality,
                                    },
                                ));
                            }
                        }'''
if old not in players:
    raise SystemExit("players refresh anchor missing")
players = players.replace(old, new, 1)
players = players.replace(
    "                    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));\n                    let players = rows.into_iter().map(|(_, _, player)| player).collect();",
    "                    rows.sort_by(|a, b| {\n                        a.0.cmp(&b.0)\n                            .then_with(|| a.1.cmp(&b.1))\n                            .then_with(|| a.2.cmp(&b.2))\n                    });\n                    let players = rows.into_iter().map(|(_, _, _, player)| player).collect();",
    1,
)
# Canvas click hook now keys off contextual Players dock, not bottom panel.
players = players.replace(
    '''                    players_were_active = this.ui_state.bottom_panel_open
                        && this.ui_state.active_bottom_tab == MapViewerBottomTab::Players;''',
    '''                    players_were_active = this.ui_state.left_panel_open
                        && this.ui_state.active_left_panel == MapViewerLeftPanel::Players;''',
    1,
)
players = players.replace(
    '''                    this.ui_state.active_bottom_tab = MapViewerBottomTab::Players;
                    this.ui_state.bottom_panel_open = true;
                    this.load_player_detail(id, cx);''',
    '''                    this.ui_state.active_left_panel = MapViewerLeftPanel::Players;
                    this.ui_state.left_panel_open = true;
                    this.player_workspace.center = PlayerWorkspaceCenter::Map;
                    this.ui_state.active_right_panel = MapViewerRightPanel::Player;
                    this.ui_state.set_right_panel_open(true);
                    this.update_viewport_after_dock_change(cx);
                    this.load_player_detail(id, cx);''',
    1,
)
# Old simple sorter becomes trust helpers + richer probe.
start = players.find("fn player_sort_rank(id: &PlayerId, valid: bool) -> u8 {")
end = players.find("pub(super) fn player_friendly_label", start)
if start < 0 or end < 0:
    raise SystemExit("player_sort_rank region missing")
players = players[:start] + '''fn is_server_like_player_id(id: &PlayerId) -> bool {
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

''' + players[end:]
# Friendly labels recognize server-like unknown IDs.
old_friendly = '''    match id {
        PlayerId::Local => "本地玩家 · ~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("服务器玩家 · {xuid}"),
        PlayerId::LegacyLevelDat => "旧版玩家 · level.dat".to_string(),
        PlayerId::Unknown(_) => format!("其他玩家 · {raw}"),
    }'''
new_friendly = '''    match id {
        PlayerId::Local => "本地玩家 · ~local_player".to_string(),
        PlayerId::Xuid(xuid) => format!("服务器玩家 · {xuid}"),
        PlayerId::LegacyLevelDat => "旧版玩家 · level.dat".to_string(),
        PlayerId::Unknown(_) if is_server_like_player_id(id) => format!("服务器记录 · {raw}"),
        PlayerId::Unknown(_) => format!("其他玩家 · {raw}"),
    }'''
if old_friendly not in players:
    raise SystemExit("friendly label anchor missing")
players = players.replace(old_friendly, new_friendly, 1)
old_probe = '''fn player_probe(data: &PlayerData) -> Result<(Option<[f64; 3]>, Option<i32>), String> {
    let root = match &data.nbt {
        NbtTag::Compound(root) => root,
        _ => return Err("玩家 NBT 根节点不是 Compound".to_string()),
    };
    Ok((
        nbt_vec3_f64(root.get("Pos")),
        nbt_i32_any(root.get("DimensionId")),
    ))
}'''
new_probe = '''fn player_probe(id: &PlayerId, data: &PlayerData) -> Result<PlayerProbe, String> {
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
    let trusted_server = server_like
        && has_unique_id
        && has_position
        && has_dimension
        && has_inventory;
    let completeness = i16::from(has_unique_id)
        + i16::from(has_position)
        + i16::from(has_dimension)
        + i16::from(has_inventory);
    let health = if (matches!(id, PlayerId::Local)
        && has_position
        && has_dimension
        && has_inventory)
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
}'''
if old_probe not in players:
    raise SystemExit("player_probe anchor missing")
players = players.replace(old_probe, new_probe, 1)
players = players.replace("fn capture_player_history(\n", "pub(super) fn capture_player_history(\n", 1)
write("src/ui/window/map_viewer/players.rs", players)

# Window defaults: player IDE needs substantially more working area.
view = read("src/ui/window/map_viewer/view.rs")
view = view.replace("const MAP_VIEWER_DEFAULT_WINDOW_WIDTH: f32 = 1120.0;", "const MAP_VIEWER_DEFAULT_WINDOW_WIDTH: f32 = 1500.0;")
view = view.replace("const MAP_VIEWER_DEFAULT_WINDOW_HEIGHT: f32 = 720.0;", "const MAP_VIEWER_DEFAULT_WINDOW_HEIGHT: f32 = 860.0;")
view = view.replace("const MAP_VIEWER_MIN_WINDOW_WIDTH: f32 = 920.0;", "const MAP_VIEWER_MIN_WINDOW_WIDTH: f32 = 1040.0;")
view = view.replace("const MAP_VIEWER_MIN_WINDOW_HEIGHT: f32 = 620.0;", "const MAP_VIEWER_MIN_WINDOW_HEIGHT: f32 = 680.0;")
view = view.replace("const MAP_VIEWER_MAX_DISPLAY_RATIO: f32 = 0.9;", "const MAP_VIEWER_MAX_DISPLAY_RATIO: f32 = 0.96;")
write("src/ui/window/map_viewer/view.rs", view)

# Wider contextual dock for searchable player records / map tools.
replace_once(
    "src/ui/window/map_viewer/layout.rs",
    "pub const IDE_LEFT_DOCK_WIDTH: f32 = 276.0;",
    "pub const IDE_LEFT_DOCK_WIDTH: f32 = 300.0;",
)

# Compile fixes in the new workspace authored before integration.
workspace = read("src/ui/window/map_viewer/player_workspace.rs")
workspace = workspace.replace("colors.warning", "colors.stat_orange_text")
workspace = workspace.replace(
    ".and_then(|tag| serde_json::to_value(tag).map_err(serde_json::Error::io))",
    ".and_then(serde_json::to_value)",
)
write("src/ui/window/map_viewer/player_workspace.rs", workspace)

print("player workspace v2 integration applied")

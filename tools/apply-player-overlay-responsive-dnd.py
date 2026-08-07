from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def write(path, text):
    (ROOT / path).write_text(text, encoding="utf-8", newline="\n")


def replace_once(path, old, new):
    text = read(path)
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, got {count}: {old[:160]!r}")
    write(path, text.replace(old, new, 1))


def regex_once(path, pattern, replacement):
    text = read(path)
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise SystemExit(f"{path}: regex expected one match, got {count}: {pattern[:160]!r}")
    write(path, updated)


# 1. The public stripe is a function switcher. Player visualization moves under Tools/Data overlay.
tool = read("src/ui/window/map_viewer/tool_stripe.rs")
player_button = '''            .child(stripe_button(
                "stripe-players",
                &colors,
                lucide_icons::icon_users(),
                "玩家",
                snapshot.left_panel_open
                    && snapshot.active_left_panel == MapViewerLeftPanel::Players,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleLeftPanelKind(
                        MapViewerLeftPanel::Players,
                    ));
                }),
            ))
'''
if tool.count(player_button) != 1:
    raise SystemExit("tool_stripe.rs: player stripe button anchor missing")
tool = tool.replace(player_button, "", 1)
write("src/ui/window/map_viewer/tool_stripe.rs", tool)

# 2. Stable player-marker identity and an explicit Players overlay switch.
model = read("src/ui/window/map_viewer/model.rs")
model = model.replace(
    '''pub(super) struct Marker {
    pub(super) x: i32,
    pub(super) z: i32,
    pub(super) label: SharedString,
}''',
    '''pub(super) struct Marker {
    pub(super) x: i32,
    pub(super) z: i32,
    pub(super) label: SharedString,
    pub(super) player_id: Option<PlayerId>,
}''',
    1,
)
model = model.replace(
    "    pub(super) ruler: bool,\n    pub(super) slime_chunks: bool,",
    "    pub(super) ruler: bool,\n    pub(super) players: bool,\n    pub(super) slime_chunks: bool,",
    1,
)
model = model.replace(
    "            ruler: true,\n            slime_chunks: false,",
    "            ruler: true,\n            players: false,\n            slime_chunks: false,",
    1,
)
model = model.replace(
    "    pub(super) pending_save_confirmation: Option<PlayerQuickEdit>,\n}",
    "    pub(super) pending_save_confirmation: Option<PlayerQuickEdit>,\n    pub(super) context_target: Option<PlayerId>,\n}",
    1,
)
write("src/ui/window/map_viewer/model.rs", model)

# 3. Tools -> Data overlay owns player display.
panels = read("src/ui/window/map_viewer/panels.rs")
anchor = '''                    .child(
                        mode_button(colors, "生物实体", self.overlay_options.entities)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_entity_overlay(cx)
                                }),
                            ),
                    )
'''
insert = '''                    .child(
                        mode_button(colors, "玩家显示", self.overlay_options.players)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_player_overlay(cx)
                                }),
                            ),
                    )
''' + anchor
if panels.count(anchor) != 1:
    raise SystemExit("panels.rs: entity overlay button anchor missing")
panels = panels.replace(anchor, insert, 1)
write("src/ui/window/map_viewer/panels.rs", panels)

# 4. Slightly denser docks; player workspace itself scales slots based on live center width.
replace_once(
    "src/ui/window/map_viewer/layout.rs",
    "pub const IDE_LEFT_DOCK_WIDTH: f32 = 300.0;",
    "pub const IDE_LEFT_DOCK_WIDTH: f32 = 280.0;",
)
state = read("src/ui/window/map_viewer/state.rs")
state = state.replace("pub const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 460.0;", "pub const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 420.0;", 1)
state = state.replace("pub const RIGHT_PANEL_MIN_WIDTH: f32 = 340.0;", "pub const RIGHT_PANEL_MIN_WIDTH: f32 = 300.0;", 1)
write("src/ui/window/map_viewer/state.rs", state)

# 5. Player refresh no longer installs the old left-click map hit-test. Markers carry PlayerId.
players = read("src/ui/window/map_viewer/players.rs")
players = players.replace("const PLAYER_CLICK_HIT_RADIUS_PX: f32 = 22.0;\nconst PLAYER_CLICK_DRAG_THRESHOLD_PX: f32 = 4.0;\n", "", 1)
players = players.replace(
    '''struct PlayerRefreshMarker {
    label: SharedString,
    dimension: Dimension,
    x: i32,
    z: i32,
}''',
    '''struct PlayerRefreshMarker {
    id: PlayerId,
    label: SharedString,
    dimension: Dimension,
    x: i32,
    z: i32,
}''',
    1,
)
players = players.replace(
    '''        if self.players.generation == 0 {
            self.install_player_canvas_click_hook(cx);
        }

''',
    "",
    1,
)
players = players.replace(
    '''                                            marker_records.push(PlayerRefreshMarker {
                                                label: label.clone(),''',
    '''                                            marker_records.push(PlayerRefreshMarker {
                                                id: id.clone(),
                                                label: label.clone(),''',
    1,
)
players = players.replace(
    '''                        markers.entry(marker.dimension).or_default().push(Marker {
                            x: marker.x,
                            z: marker.z,
                            label: marker.label,
                        });''',
    '''                        markers.entry(marker.dimension).or_default().push(Marker {
                            x: marker.x,
                            z: marker.z,
                            label: marker.label,
                            player_id: Some(marker.id),
                        });''',
    1,
)
players = players.replace(
    '''                        if let Some(id) = this.players.selected.clone() {
                            this.load_player_detail(id, cx);
                        }''',
    '''                        if this.player_workspace_active() {
                            if let Some(id) = this.players.selected.clone() {
                                this.load_player_detail(id, cx);
                            }
                        }''',
    1,
)
players, removed = re.subn(
    r'''\n    fn install_player_canvas_click_hook\(&mut self, cx: &mut Context<Self>\) \{.*?\n    pub\(super\) fn load_player_detail''',
    "\n    pub(super) fn load_player_detail",
    players,
    count=1,
    flags=re.S,
)
if removed != 1:
    raise SystemExit("players.rs: old canvas click hook block not found")
write("src/ui/window/map_viewer/players.rs", players)

# 6. Canvas marker layer becomes interactive and displays player avatars only when Tools/玩家显示 is on.
canvas = read("src/ui/window/map_viewer/canvas.rs")
canvas = canvas.replace(
    '''    PointerMoved {
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
    },''',
    '''    OpenPlayerMarkerContext {
        marker_index: usize,
        position: Point<Pixels>,
    },
    PointerMoved {
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
    },''',
    1,
)
old_new = '''    pub(super) fn new(map_focus_handle: FocusHandle, cx: &mut Context<Self>) -> Self {
        let paste_controls_layer = cx.new(|_cx| MapPasteControlsView::default());
        let subscriptions = vec![cx.subscribe(
            &paste_controls_layer,
            |_this, _controls, action: &MapCanvasAction, cx| {
                cx.emit(*action);
            },
        )];
        Self {
            tile_layer: cx.new(|_cx| MapTileLayerView::default()),
            overlay_layer: cx.new(|_cx| MapOverlayLayerView::default()),
            marker_layer: cx.new(|_cx| MapMarkerLayerView::default()),'''
new_new = '''    pub(super) fn new(map_focus_handle: FocusHandle, cx: &mut Context<Self>) -> Self {
        let paste_controls_layer = cx.new(|_cx| MapPasteControlsView::default());
        let marker_layer = cx.new(|_cx| MapMarkerLayerView::default());
        let subscriptions = vec![
            cx.subscribe(
                &paste_controls_layer,
                |_this, _controls, action: &MapCanvasAction, cx| {
                    cx.emit(*action);
                },
            ),
            cx.subscribe(
                &marker_layer,
                |_this, _markers, action: &MapCanvasAction, cx| {
                    cx.emit(*action);
                },
            ),
        ];
        Self {
            tile_layer: cx.new(|_cx| MapTileLayerView::default()),
            overlay_layer: cx.new(|_cx| MapOverlayLayerView::default()),
            marker_layer,'''
if canvas.count(old_new) != 1:
    raise SystemExit("canvas.rs: MapCanvasView::new anchor missing")
canvas = canvas.replace(old_new, new_new, 1)
canvas = canvas.replace(
    '''            .child(cached_absolute_layer(&self.overlay_layer, frame_revision))
            .child(cached_absolute_layer(&self.marker_layer, frame_revision))
            .child(cached_absolute_layer(&self.hud_layer, frame_revision))
            .child(render_interaction_layer(
                &self.map_focus_handle,
                self.interaction_cursor,
                cx,
            ))
            .child(cached_absolute_layer(
                &self.paste_controls_layer,
                frame_revision,
            ))''',
    '''            .child(cached_absolute_layer(&self.overlay_layer, frame_revision))
            .child(cached_absolute_layer(&self.hud_layer, frame_revision))
            .child(render_interaction_layer(
                &self.map_focus_handle,
                self.interaction_cursor,
                cx,
            ))
            .child(cached_absolute_layer(&self.marker_layer, frame_revision))
            .child(cached_absolute_layer(
                &self.paste_controls_layer,
                frame_revision,
            ))''',
    1,
)
canvas = canvas.replace(
    '''struct MarkerLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    markers: Arc<Vec<Marker>>,
    markers_generation: u64,
}''',
    '''struct MarkerLayerSnapshot {
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    overlays: OverlayOptions,
    markers: Arc<Vec<Marker>>,
    markers_generation: u64,
}''',
    1,
)
canvas = canvas.replace(
    '''            colors: snapshot.colors,
            markers: snapshot.markers.clone(),''',
    '''            colors: snapshot.colors,
            overlays: snapshot.overlays,
            markers: snapshot.markers.clone(),''',
    1,
)
canvas = canvas.replace(
    '''            && self.colors == other.colors
            && self.markers_generation == other.markers_generation''',
    '''            && self.colors == other.colors
            && self.overlays == other.overlays
            && self.markers_generation == other.markers_generation''',
    1,
)
canvas = canvas.replace(
    '''impl Render for MapMarkerLayerView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(render_markers)
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}''',
    '''impl EventEmitter<MapCanvasAction> for MapMarkerLayerView {}

impl Render for MapMarkerLayerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.snapshot
            .as_ref()
            .map(|snapshot| render_markers(snapshot, cx))
            .unwrap_or_else(|| div().absolute().inset_0())
    }
}''',
    1,
)
old_markers = '''fn render_markers(snapshot: &MarkerLayerSnapshot) -> Div {
    let mut layer = div().absolute().inset_0();
    for marker in snapshot.markers.iter() {
        let Some((screen_x, screen_y)) =
            viewport_screen_for_block(snapshot.viewport, snapshot.layout, marker.x, marker.z)
        else {
            continue;
        };
        let left = px(screen_x);
        let top = px(screen_y);
        layer = layer.child(
            div()
                .absolute()
                .left(left - px(7.0))
                .top(top - px(7.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .w(px(14.0))
                        .h(px(14.0))
                        .rounded_full()
                        .border_2()
                        .border_color(rgb(0xffffff))
                        .bg(snapshot.colors.danger),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.78,
                            ..snapshot.colors.surface
                        })
                        .text_size(px(11.0))
                        .text_color(snapshot.colors.text_primary)
                        .child(marker.label.clone()),
                ),
        );
    }
    layer
}'''
new_markers = '''fn render_markers(snapshot: &MarkerLayerSnapshot, cx: &mut Context<MapMarkerLayerView>) -> Div {
    let mut layer = div().absolute().inset_0();
    for (marker_index, marker) in snapshot.markers.iter().enumerate() {
        let is_player = marker.player_id.is_some();
        if is_player && !snapshot.overlays.players {
            continue;
        }
        let Some((screen_x, screen_y)) =
            viewport_screen_for_block(snapshot.viewport, snapshot.layout, marker.x, marker.z)
        else {
            continue;
        };
        let left = px(screen_x);
        let top = px(screen_y);
        if is_player {
            let show_label = snapshot.viewport.scale >= 0.75;
            layer = layer.child(
                div()
                    .absolute()
                    .left(left - px(14.0))
                    .top(top - px(14.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id(("player-map-marker", marker_index))
                            .w(px(28.0))
                            .h(px(28.0))
                            .flex_none()
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .border_2()
                            .border_color(rgb(0xffffff))
                            .bg(Hsla {
                                a: 0.90,
                                ..snapshot.colors.surface
                            })
                            .cursor(CursorStyle::PointingHand)
                            .child(img("images/map/entity/player.png").w(px(28.0)).h(px(28.0)))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |_this, event: &MouseDownEvent, _window, cx| {
                                    cx.emit(MapCanvasAction::OpenPlayerMarkerContext {
                                        marker_index,
                                        position: event.position,
                                    });
                                    cx.stop_propagation();
                                }),
                            ),
                    )
                    .when(show_label, |this| {
                        this.child(
                            div()
                                .max_w(px(220.0))
                                .overflow_hidden()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(crate::ui::theme::tokens::radius::SM))
                                .bg(Hsla {
                                    a: 0.82,
                                    ..snapshot.colors.surface
                                })
                                .text_size(px(10.0))
                                .text_color(snapshot.colors.text_primary)
                                .child(marker.label.clone()),
                        )
                    }),
            );
        } else {
            layer = layer.child(
                div()
                    .absolute()
                    .left(left - px(7.0))
                    .top(top - px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(0xffffff))
                            .bg(snapshot.colors.danger),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .bg(Hsla {
                                a: 0.78,
                                ..snapshot.colors.surface
                            })
                            .text_size(px(11.0))
                            .text_color(snapshot.colors.text_primary)
                            .child(marker.label.clone()),
                    ),
            );
        }
    }
    layer
}'''
if canvas.count(old_markers) != 1:
    raise SystemExit("canvas.rs: render_markers anchor missing")
canvas = canvas.replace(old_markers, new_markers, 1)
write("src/ui/window/map_viewer/canvas.rs", canvas)

# 7. Interaction ownership: player overlay toggle + right-click marker context.
inter = read("src/ui/window/map_viewer/interactions.rs")
inter = inter.replace(
    '''    pub(super) fn close_all_menus(&mut self, cx: &mut Context<Self>) {
        let changed = self.context_menu.take().is_some()
            || self.ui_state.top_more_open''',
    '''    pub(super) fn close_all_menus(&mut self, cx: &mut Context<Self>) {
        let changed = self.context_menu.take().is_some()
            || self.players.context_target.take().is_some()
            || self.ui_state.top_more_open''',
    1,
)
inter = inter.replace(
    '''    pub(super) fn toggle_ruler(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.ruler = !self.overlay_options.ruler;
        cx.notify();
    }
''',
    '''    pub(super) fn toggle_ruler(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.ruler = !self.overlay_options.ruler;
        cx.notify();
    }

    pub(super) fn toggle_player_overlay(&mut self, cx: &mut Context<Self>) {
        self.overlay_options.players = !self.overlay_options.players;
        self.last_synced_canvas_snapshot_key = None;
        if self.overlay_options.players && self.players.players.is_empty() {
            self.refresh_players(cx);
            return;
        }
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }

    pub(super) fn open_player_marker_context(
        &mut self,
        marker_index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(marker) = self
            .markers
            .get(&self.dimension)
            .and_then(|markers| markers.get(marker_index))
            .cloned()
        else {
            return;
        };
        let Some(player_id) = marker.player_id.clone() else {
            return;
        };
        self.drag = None;
        self.right_selection_drag = None;
        self.players.context_target = Some(player_id);
        self.context_menu = Some(ContextMenuState {
            position,
            block_x: marker.x,
            block_z: marker.z,
        });
        self.ui_state.top_more_open = false;
        self.ui_state.context_more_open = false;
        self.ui_state.context_paste_open = false;
        cx.notify();
    }
''',
    1,
)
inter = inter.replace(
    '''        self.ui_state.dock_drag = None;
        self.context_menu = None;
        self.ui_state.top_more_open = false;''',
    '''        self.ui_state.dock_drag = None;
        self.context_menu = None;
        self.players.context_target = None;
        self.ui_state.top_more_open = false;''',
    1,
)
# handle_canvas_action is a match over MapCanvasAction; insert the marker action before PointerMoved.
needle = '''            MapCanvasAction::PointerMoved {
                position,
                pressed_button,
            } =>'''
replacement = '''            MapCanvasAction::OpenPlayerMarkerContext {
                marker_index,
                position,
            } => self.open_player_marker_context(marker_index, position, cx),
            MapCanvasAction::PointerMoved {
                position,
                pressed_button,
            } =>'''
if inter.count(needle) != 1:
    raise SystemExit("interactions.rs: handle_canvas_action PointerMoved anchor missing")
inter = inter.replace(needle, replacement, 1)
write("src/ui/window/map_viewer/interactions.rs", inter)

# 8. Player context menu: Edit Player is the first command when invoked from a player icon.
menus = read("src/ui/window/map_viewer/menus.rs")
anchor = '''        ];
        {
            let Some(chunk) = self.context_chunk_pos() else {'''
insert = '''        ];
        if let Some(player_id) = self.players.context_target.clone() {
            let entity = cx.entity();
            groups.insert(
                0,
                ContextMenuGroup::new(vec![ContextMenuEntry::item(
                    ContextMenuItem::new("编辑玩家")
                        .description("打开玩家背包、末影箱、装备与 NBT 编辑器")
                        .on_click(move |cx| {
                            let player_id = player_id.clone();
                            entity.update(cx, move |this, cx| {
                                this.context_menu = None;
                                this.players.context_target = None;
                                this.open_player_workspace_for_player(
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
            let Some(chunk) = self.context_chunk_pos() else {'''
if menus.count(anchor) != 1:
    raise SystemExit("menus.rs: groups/write anchor missing")
menus = menus.replace(anchor, insert, 1)
write("src/ui/window/map_viewer/menus.rs", menus)

# 9. Player workspace: deterministic labels, responsive slots/header, real DnD, Lucide fallbacks.
workspace = read("src/ui/window/map_viewer/player_workspace.rs")
workspace = workspace.replace(
    "use super::prelude::*;\n",
    "use super::prelude::*;\nuse crate::ui::components::icon::themed_icon;\nuse lucide_gpui::icons as lucide_icons;\n",
    1,
)
workspace = workspace.replace(
    '''#[derive(Clone, Debug)]
struct PlayerVisualItemPatch {
    id: String,
    count: i8,
    damage: i16,
    custom_name: String,
    lore: Vec<String>,
    can_place_on: Vec<String>,
    can_destroy: Vec<String>,
}

impl MapViewerWindowView {''',
    '''#[derive(Clone, Debug)]
struct PlayerVisualItemPatch {
    id: String,
    count: i8,
    damage: i16,
    custom_name: String,
    lore: Vec<String>,
    can_place_on: Vec<String>,
    can_destroy: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct PlayerWorkspaceMetrics {
    slot_size: f32,
    slot_gap: f32,
    panel_padding: f32,
    outer_padding: f32,
    compact: bool,
}

#[derive(Clone)]
struct PlayerItemDrag {
    source: PlayerItemSelection,
    label: SharedString,
    texture: Option<Arc<Path>>,
    count: i32,
    position: Point<Pixels>,
}

impl PlayerItemDrag {
    fn at(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for PlayerItemDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let size = 48.0;
        div()
            .pl(self.position.x - px(size * 0.5))
            .pt(self.position.y - px(size * 0.5))
            .child(
                div()
                    .relative()
                    .w(px(size))
                    .h(px(size))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(0xffffff))
                    .bg(rgb(0xf3efe8))
                    .shadow_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(self.texture.clone(), |this, texture| {
                        this.child(img(texture).w(px(36.0)).h(px(36.0)))
                    })
                    .when(self.texture.is_none(), |this| {
                        this.child(themed_icon(
                            lucide_icons::icon_package(),
                            24.0,
                            rgb(0x6f675c).into(),
                        ))
                    })
                    .when(self.count > 1, |this| {
                        this.child(
                            div()
                                .absolute()
                                .right(px(3.0))
                                .bottom(px(1.0))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x2b2620)),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(size + 6.0))
                            .top(px(12.0))
                            .max_w(px(180.0))
                            .overflow_hidden()
                            .px(px(6.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(Hsla {
                                a: 0.92,
                                ..rgb(0xf3efe8).into()
                            })
                            .text_size(px(10.0))
                            .text_color(rgb(0x2b2620))
                            .child(self.label.clone()),
                    ),
            )
    }
}

impl MapViewerWindowView {''',
    1,
)
workspace = workspace.replace(
    '''    pub(super) fn player_workspace_active(&self) -> bool {
        self.ui_state.left_panel_open
            && self.ui_state.active_left_panel == MapViewerLeftPanel::Players
    }
''',
    '''    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
        let available = self.viewport.width.max(320.0);
        let compact = available < 620.0;
        let outer_padding = if available < 470.0 { 8.0 } else if compact { 12.0 } else { 18.0 };
        let panel_padding = if available < 470.0 { 9.0 } else if compact { 12.0 } else { 18.0 };
        let slot_gap = if compact { 3.0 } else { 4.0 };
        let usable = (available - outer_padding * 2.0 - panel_padding * 2.0).min(584.0).max(288.0);
        let slot_size = ((usable - slot_gap * 8.0) / 9.0).clamp(30.0, 52.0);
        PlayerWorkspaceMetrics {
            slot_size,
            slot_gap,
            panel_padding,
            outer_padding,
            compact,
        }
    }

    pub(super) fn player_workspace_active(&self) -> bool {
        self.ui_state.active_left_panel == MapViewerLeftPanel::Players
            && (self.ui_state.left_panel_open || self.players.selected.is_some())
    }
''',
    1,
)
# Deterministic label shortening: no layout-engine ellipsis oscillation.
workspace = workspace.replace(
    '''        let raw = player_id_label(&player.id);
        let quality_label = player.quality.health.label();''',
    '''        let raw = player_id_label(&player.id);
        let stable_label = stable_middle_ellipsis(player.label.as_ref(), 30);
        let stable_raw = stable_middle_ellipsis(&raw, 34);
        let quality_label = player.quality.health.label();''',
    1,
)
workspace = workspace.replace(
    '''        div()
            .mb(px(4.0))
            .p(px(7.0))''',
    '''        div()
            .mb(px(4.0))
            .p(px(7.0))
            .overflow_hidden()''',
    1,
)
workspace = workspace.replace(
    '''                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child(player.label.clone()),''',
    '''                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child(stable_label),''',
    1,
)
workspace = workspace.replace(
    '''                                div()
                                    .truncate()
                                    .text_size(px(9.0))
                                    .text_color(colors.text_muted)
                                    .child(raw),''',
    '''                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_size(px(9.0))
                                    .text_color(colors.text_muted)
                                    .child(stable_raw),''',
    1,
)
workspace = workspace.replace(
    '''                        div()
                            .px(px(5.0))
                            .py(px(2.0))''',
    '''                        div()
                            .w(px(34.0))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .px(px(4.0))
                            .py(px(2.0))''',
    1,
)
# Opening an editor is explicit (usually from right-click player icon) and clears map context state.
workspace = workspace.replace(
    '''    ) {
        self.ui_state.active_left_panel = MapViewerLeftPanel::Players;
        self.ui_state.left_panel_open = true;''',
    '''    ) {
        self.context_menu = None;
        self.players.context_target = None;
        self.ui_state.active_left_panel = MapViewerLeftPanel::Players;
        self.ui_state.left_panel_open = true;''',
    1,
)
# Center workspace padding follows live viewport width.
workspace = workspace.replace(
    '''        let entries = player_inventory_entries(&detail.nbt);
        div()''',
    '''        let entries = player_inventory_entries(&detail.nbt);
        let metrics = self.player_workspace_metrics();
        div()''',
    1,
)
workspace = workspace.replace("                    .p(px(18.0))\n                    .child(match self.player_workspace.center {", "                    .p(px(metrics.outer_padding))\n                    .child(match self.player_workspace.center {", 1)
# Header wraps instead of clipping/deforming after resize.
workspace = workspace.replace(
    '''        div()
            .h(px(50.0))
            .flex_none()
            .px(px(14.0))''',
    '''        div()
            .min_h(px(50.0))
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))''',
    1,
)
workspace = workspace.replace(
    '''            .flex()
            .items_center()
            .gap(px(7.0))''',
    '''            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(7.0))''',
    1,
)
# Inventory/ender/equipment panels fill available center width and shrink slot geometry.
workspace = workspace.replace("        div()\n            .max_w(px(620.0))\n            .mx_auto()", "        let metrics = self.player_workspace_metrics();\n        div()\n            .w_full()\n            .max_w(px(620.0))\n            .mx_auto()", 3)
workspace = workspace.replace("            .p(px(18.0))", "            .p(px(metrics.panel_padding))", 3)
workspace = workspace.replace(
    '''                div()
                    .flex()
                    .gap(px(20.0))''',
    '''                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.0))''',
    1,
)
workspace = workspace.replace(
    '''    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.0))
            .children(slots.map(|slot| {''',
    '''    ) -> Div {
        let metrics = self.player_workspace_metrics();
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(metrics.slot_gap))
            .children(slots.map(|slot| {''',
    1,
)
# Replace slot renderer with responsive dimensions, Lucide fallback, visible drag preview, and drop support.
slot_pattern = r'''    fn render_player_inventory_slot\(\n        &self,\n        colors: &ThemeColors,\n        kind: PlayerInventoryKind,\n        slot: i32,\n        entries: &\[PlayerInventoryEntry\],\n        cx: &mut Context<Self>,\n    \) -> Div \{.*?\n    \}\n\n    fn render_workspace_quick_catalog'''
slot_replacement = '''    fn render_player_inventory_slot(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        slot: i32,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let metrics = self.player_workspace_metrics();
        let entry = entries.iter().find(|entry| {
            entry.kind == kind && entry.slot.unwrap_or(entry.list_index as i32) == slot
        });
        let list_index = entry.map(|entry| entry.list_index);
        let selection = PlayerItemSelection {
            kind,
            list_index,
            slot,
        };
        let selected = self
            .player_workspace
            .selected_item
            .is_some_and(|selected| selected.kind == kind && selected.slot == slot);
        let texture = entry.and_then(|entry| self.player_item_texture(entry.item.name.as_deref()));
        let count = entry.and_then(|entry| entry.item.count).unwrap_or(0);
        let enchanted =
            entry.is_some_and(|entry| !player_item_enchantments(&entry.item.nbt).is_empty());
        let has_custom_name = entry
            .and_then(|entry| player_item_custom_name(&entry.item.nbt))
            .is_some_and(|name| !name.trim().is_empty());
        let drag_payload = entry.map(|entry| PlayerItemDrag {
            source: selection,
            label: SharedString::from(
                entry
                    .item
                    .name
                    .clone()
                    .unwrap_or_else(|| "未知物品".to_string()),
            ),
            texture: texture.clone(),
            count,
            position: Point::default(),
        });
        let icon_size = (metrics.slot_size * 0.72).clamp(20.0, 38.0);
        div()
            .relative()
            .w(px(metrics.slot_size))
            .h(px(metrics.slot_size))
            .flex_none()
            .rounded(px(3.0))
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                Hsla {
                    a: 0.42,
                    ..colors.border
                }
            })
            .bg(if selected {
                Hsla {
                    a: 0.16,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.72,
                    ..colors.surface_hover
                }
            })
            .cursor_pointer()
            .hover(|style| {
                style.bg(Hsla {
                    a: 0.92,
                    ..colors.surface_hover
                })
            })
            .flex()
            .items_center()
            .justify_center()
            .when_some(texture.clone(), |this, texture| {
                this.child(img(texture).w(px(icon_size)).h(px(icon_size)))
            })
            .when(entry.is_some() && texture.is_none(), |this| {
                this.child(themed_icon(
                    lucide_icons::icon_package(),
                    icon_size.min(26.0),
                    colors.text_muted,
                ))
            })
            .when(entry.is_none(), |this| {
                this.child(themed_icon(
                    lucide_icons::icon_plus(),
                    (icon_size * 0.55).max(12.0),
                    Hsla {
                        a: 0.42,
                        ..colors.text_muted
                    },
                ))
            })
            .when(entry.is_some() && count > 1, |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(3.0))
                        .bottom(px(1.0))
                        .text_size(px(if metrics.compact { 9.0 } else { 10.0 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(count.to_string()),
                )
            })
            .when(enchanted, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(2.0))
                        .top(px(2.0))
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(colors.accent),
                )
            })
            .when(has_custom_name, |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(2.0))
                        .top(px(2.0))
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(colors.stat_orange_text),
                )
            })
            .when_some(drag_payload, |this, drag| {
                this.cursor_move().on_drag(
                    drag,
                    |info: &PlayerItemDrag, position, _window, cx| {
                        cx.new(|_| info.clone().at(position))
                    },
                )
            })
            .on_drop(cx.listener(move |this, drag: &PlayerItemDrag, _window, cx| {
                this.move_player_workspace_item(drag.source, selection, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, window, cx| {
                    this.select_player_workspace_item(selection, window, cx)
                }),
            )
    }

    fn render_workspace_quick_catalog'''
workspace, count = re.subn(slot_pattern, slot_replacement, workspace, count=1, flags=re.S)
if count != 1:
    raise SystemExit("player_workspace.rs: slot renderer block not found")
# Inspector icon fallback.
workspace = workspace.replace(
    '''                            .when_some(texture, |this, texture| {
                                this.child(img(texture).w(px(34.0)).h(px(34.0)))
                            }),''',
    '''                            .when_some(texture.clone(), |this, texture| {
                                this.child(img(texture).w(px(34.0)).h(px(34.0)))
                            })
                            .when(texture.is_none(), |this| {
                                this.child(themed_icon(
                                    lucide_icons::icon_package(),
                                    24.0,
                                    colors.text_muted,
                                ))
                            }),''',
    1,
)
# Count/Damage line can wrap in a narrow inspector.
workspace = workspace.replace(
    '''                div()
                    .flex()
                    .gap(px(7.0))
                    .child(
                        player_form_field(colors, "数量", self.player_workspace.count.clone())''',
    '''                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(7.0))
                    .child(
                        player_form_field(colors, "数量", self.player_workspace.count.clone())''',
    1,
)
# Atomic drag/drop swap writer + list-count synchronization.
write_anchor = '''    fn write_player_workspace_slot(
        &mut self,
        selection: PlayerItemSelection,'''
move_method = '''    fn sync_player_summary_inventory_counts(&mut self, detail: &PlayerDetail) {
        let entries = player_inventory_entries(&detail.nbt);
        let item_count = entries.len();
        let ender_item_count = entries
            .iter()
            .filter(|entry| entry.kind == PlayerInventoryKind::EnderChest)
            .count();
        let Some(summary) = self
            .players
            .players
            .iter_mut()
            .find(|summary| summary.id == detail.id)
        else {
            return;
        };
        let old_bonus = i16::try_from(summary.quality.item_count.min(10)).unwrap_or(10)
            + if summary.quality.ender_item_count > 0 { 5 } else { 0 };
        let new_bonus = i16::try_from(item_count.min(10)).unwrap_or(10)
            + if ender_item_count > 0 { 5 } else { 0 };
        summary.quality.score = summary
            .quality
            .score
            .saturating_sub(old_bonus)
            .saturating_add(new_bonus);
        summary.quality.item_count = item_count;
        summary.quality.ender_item_count = ender_item_count;
        summary.quality.has_inventory = true;
    }

    fn move_player_workspace_item(
        &mut self,
        source: PlayerItemSelection,
        target: PlayerItemSelection,
        cx: &mut Context<Self>,
    ) {
        if source.kind == target.kind && source.slot == target.slot {
            return;
        }
        let Some(id) = self.players.selected.clone() else {
            return;
        };
        if self.players.saving {
            self.status = SharedString::from("上一项玩家写入尚未完成");
            cx.notify();
            return;
        }
        self.players.saving = true;
        self.players.generation = self.players.generation.saturating_add(1);
        let generation = self.players.generation;
        let world_path = self.world_path.clone();
        self.status = SharedString::from(format!(
            "正在移动物品：{} {} → {} {}...",
            source.kind.label(),
            source.slot,
            target.kind.label(),
            target.slot
        ));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut options = bedrock_world::OpenOptions::default();
                    options.read_only = false;
                    let world = BedrockWorld::open_blocking(&world_path, options)
                        .map_err(|error| error.to_string())?;
                    let history_capture = capture_player_history(
                        &world_path,
                        &id,
                        "玩家物品：拖拽移动/交换",
                    );
                    let mut data = world
                        .get_player_blocking(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "玩家记录不存在".to_string())?;
                    let source_item = player_slot_item(&data.nbt, source)
                        .ok_or_else(|| "拖拽源物品已经不存在，请刷新玩家数据".to_string())?;
                    let target_item = player_slot_item(&data.nbt, target);
                    replace_player_slot(&mut data.nbt, source, target_item)?;
                    replace_player_slot(&mut data.nbt, target, Some(source_item))?;
                    data = PlayerData::from_nbt(id.clone(), data.nbt)
                        .map_err(|error| error.to_string())?;
                    world
                        .put_player_blocking(&data)
                        .map_err(|error| error.to_string())?;
                    let detail = player_detail_from_data(data)?;
                    if let Ok(capture) = history_capture {
                        complete_after(capture, "玩家物品：拖拽移动/交换")?;
                    }
                    Ok::<_, String>(detail)
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
                        this.sync_player_summary_inventory_counts(&detail);
                        this.players.detail = Some(detail);
                        this.player_workspace.selected_item = Some(target);
                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "物品拖拽已写入 · 目标有物品时自动交换 · 可从历史撤销",
                        );
                    }
                    Err(error) => {
                        this.player_workspace.item_editor_error =
                            Some(SharedString::from(error.clone()));
                        this.status = SharedString::from(error);
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

''' + write_anchor
if workspace.count(write_anchor) != 1:
    raise SystemExit("player_workspace.rs: write_player_workspace_slot anchor missing")
workspace = workspace.replace(write_anchor, move_method, 1)
# Existing writes also update counts shown in the left player list immediately.
workspace = workspace.replace(
    '''                    Ok(detail) => {
                        this.players.detail = Some(detail);
                        this.player_workspace.item_editor_dirty = false;''',
    '''                    Ok(detail) => {
                        this.sync_player_summary_inventory_counts(&detail);
                        this.players.detail = Some(detail);
                        this.player_workspace.item_editor_dirty = false;''',
    1,
)
# Helper for DnD fresh-world reads.
helper_anchor = '''fn replace_player_slot(
    player: &mut NbtTag,'''
helper = '''fn player_slot_item(player: &NbtTag, selection: PlayerItemSelection) -> Option<NbtTag> {
    let root = nbt_compound_ref(player)?;
    let NbtTag::List(list) = root.get(selection.kind.nbt_key())? else {
        return None;
    };
    let index = selection
        .list_index
        .filter(|index| *index < list.len())
        .or_else(|| {
            list.iter().position(|item| {
                nbt_compound_ref(item).and_then(|compound| nbt_number_i32(compound.get("Slot")))
                    == Some(selection.slot)
            })
        })?;
    let item = list.get(index)?.clone();
    let name = nbt_compound_ref(&item)
        .and_then(|compound| compound.get("Name"))
        .and_then(|tag| match tag {
            NbtTag::String(value) => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or_default();
    (!name.trim().is_empty()).then_some(item)
}

''' + helper_anchor
if workspace.count(helper_anchor) != 1:
    raise SystemExit("player_workspace.rs: replace_player_slot anchor missing")
workspace = workspace.replace(helper_anchor, helper, 1)
# Deterministic middle ellipsis helper used by the left player list.
workspace += '''\nfn stable_middle_ellipsis(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars || max_chars < 5 {
        return value.to_string();
    }
    let tail = (max_chars / 3).max(2);
    let head = max_chars.saturating_sub(tail + 1);
    let prefix = value.chars().take(head).collect::<String>();
    let suffix = value
        .chars()
        .skip(count.saturating_sub(tail))
        .collect::<String>();
    format!("{prefix}…{suffix}")
}\n'''
write("src/ui/window/map_viewer/player_workspace.rs", workspace)

# 10. Manual red-dot markers need explicit non-player identity.
inter = read("src/ui/window/map_viewer/interactions.rs")
# Add player_id only to Marker literals that do not already specify it. This file owns manual map markers.
inter = re.sub(
    r'''Marker \{\n(\s*)x: ([^\n]+),\n\s*z: ([^\n]+),\n\s*label: ([^\n]+),\n\s*\}''',
    lambda m: f"Marker {{\n{m.group(1)}x: {m.group(2)},\n{m.group(1)}z: {m.group(3)},\n{m.group(1)}label: {m.group(4)},\n{m.group(1)}player_id: None,\n{m.group(1)}}}",
    inter,
)
write("src/ui/window/map_viewer/interactions.rs", inter)

print("player overlay / responsive workspace / drag-drop patch applied")

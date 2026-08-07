use super::model::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use gpui::StatefulInteractiveElement as _;
use lucide_gpui::icons as lucide_icons;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PlayerWorkspaceCenter {
    #[default]
    Map,
    Inventory,
    EnderChest,
    Equipment,
}

impl PlayerWorkspaceCenter {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Map => "地图",
            Self::Inventory => "背包",
            Self::EnderChest => "末影箱",
            Self::Equipment => "装备",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PlayerInspectorMode {
    #[default]
    Visual,
    RawNbt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PlayerItemSelection {
    pub(super) kind: PlayerInventoryKind,
    pub(super) list_index: Option<usize>,
    pub(super) slot: i32,
}

pub(super) struct PlayerWorkspaceState {
    pub(super) center: PlayerWorkspaceCenter,
    pub(super) inspector_mode: PlayerInspectorMode,
    pub(super) selected_item: Option<PlayerItemSelection>,
    pub(super) search: Entity<InputState>,
    pub(super) item_id: Entity<InputState>,
    pub(super) count: Entity<InputState>,
    pub(super) damage: Entity<InputState>,
    pub(super) custom_name: Entity<InputState>,
    pub(super) lore: Entity<InputState>,
    pub(super) can_place_on: Entity<InputState>,
    pub(super) can_destroy: Entity<InputState>,
    pub(super) enchant_id: Entity<InputState>,
    pub(super) enchant_level: Entity<InputState>,
    pub(super) item_editor_state: Entity<CodeEditorState>,
    pub(super) item_editor_dirty: bool,
    pub(super) item_editor_error: Option<SharedString>,
}

impl PlayerWorkspaceState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MapViewerWindowView>) -> Self {
        let search = workspace_input(window, cx, "搜索玩家 / UID / XUID...");
        let item_id = workspace_input(window, cx, "minecraft:diamond_sword");
        let count = workspace_input(window, cx, "1");
        let damage = workspace_input(window, cx, "0");
        let custom_name = workspace_input(window, cx, "自定义名称（留空移除）");
        let lore = workspace_input(window, cx, "Lore，多行使用 | 分隔");
        let can_place_on = workspace_input(window, cx, "minecraft:stone, minecraft:dirt");
        let can_destroy = workspace_input(window, cx, "minecraft:stone, minecraft:dirt");
        let enchant_id = workspace_input(window, cx, "附魔 ID，例如 9");
        let enchant_level = workspace_input(window, cx, "等级，例如 32767");
        let item_editor_state = cx.new(|cx| {
            let mut editor = CodeEditorState::new(cx);
            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
            editor
        });
        Self {
            center: PlayerWorkspaceCenter::Map,
            inspector_mode: PlayerInspectorMode::Visual,
            selected_item: None,
            search,
            item_id,
            count,
            damage,
            custom_name,
            lore,
            can_place_on,
            can_destroy,
            enchant_id,
            enchant_level,
            item_editor_state,
            item_editor_dirty: false,
            item_editor_error: None,
        }
    }
}

fn workspace_input(
    window: &mut Window,
    cx: &mut Context<MapViewerWindowView>,
    placeholder: &'static str,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_placeholder(SharedString::from(placeholder), window, cx);
        input
    })
}

pub(super) fn player_workspace_subscriptions(
    state: &PlayerWorkspaceState,
    cx: &mut Context<MapViewerWindowView>,
) -> Vec<Subscription> {
    let mut subscriptions = Vec::new();
    for input in [
        state.search.clone(),
        state.item_id.clone(),
        state.count.clone(),
        state.damage.clone(),
        state.custom_name.clone(),
        state.lore.clone(),
        state.can_place_on.clone(),
        state.can_destroy.clone(),
        state.enchant_id.clone(),
        state.enchant_level.clone(),
    ] {
        subscriptions.push(
            cx.subscribe(&input, |this, _input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.player_workspace.item_editor_error = None;
                    cx.notify();
                }
            }),
        );
    }
    subscriptions.push(cx.subscribe(
        &state.item_editor_state,
        |this, editor, event: &CodeEditorEvent, cx| {
            this.handle_player_item_editor_event(editor, event, cx);
        },
    ));
    subscriptions
}

#[derive(Clone, Debug)]
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

impl MapViewerWindowView {
    fn player_workspace_metrics(&self) -> PlayerWorkspaceMetrics {
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

    pub(super) fn render_player_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let query = self
            .player_workspace
            .search
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_ascii_lowercase();
        let visible = self
            .players
            .players
            .iter()
            .filter(|player| {
                if query.is_empty() {
                    return true;
                }
                player.label.as_ref().to_ascii_lowercase().contains(&query)
                    || player_id_label(&player.id)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || player.quality.search_text().contains(&query)
            })
            .collect::<Vec<_>>();

        div()
            .w(px(IDE_LEFT_DOCK_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.surface)
            .child(
                div()
                    .flex_none()
                    .p(px(12.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(panel_title(colors, "玩家资源"))
                            .child(status_badge(
                                colors,
                                format!("{} / {}", visible.len(), self.players.players.len()),
                            ))
                            .child(div().flex_1())
                            .child(toolbar_button(colors, "刷新").on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.refresh_players(cx)),
                            )),
                    )
                    .child(
                        div()
                            .h(px(32.0))
                            .px(px(8.0))
                            .rounded(px(crate::ui::theme::tokens::radius::MD))
                            .border_1()
                            .border_color(Hsla {
                                a: CHROME_HAIRLINE_ALPHA,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: CHROME_ELEVATED_ALPHA,
                                ..colors.surface_hover
                            })
                            .child(
                                Input::new(&self.player_workspace.search)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .cleanable(true)
                                    .w_full()
                                    .h_full()
                                    .px(px(0.0))
                                    .text_size(px(12.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .line_height(px(15.0))
                            .text_color(colors.text_muted)
                            .child("可信度排序：本地玩家 → 完整服务器玩家 → 其他完整玩家 → 部分/残留记录。仅可信记录显示地图标记。"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .px(px(8.0))
                    .pb(px(10.0))
                    .when(self.players.loading, |this| {
                        this.child(
                            div()
                                .p(px(8.0))
                                .text_size(px(11.0))
                                .text_color(colors.text_muted)
                                .child("正在读取并校验玩家记录..."),
                        )
                    })
                    .when(visible.is_empty() && !self.players.loading, |this| {
                        this.child(
                            div()
                                .p(px(10.0))
                                .text_size(px(11.0))
                                .text_color(colors.text_muted)
                                .child("没有匹配的玩家记录。"),
                        )
                    })
                    .children(visible.into_iter().map(|player| {
                        self.render_player_resource_row(colors, player, cx)
                            .into_any_element()
                    })),
            )
    }

    fn render_player_resource_row(
        &self,
        colors: &ThemeColors,
        player: &PlayerSummary,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = self
            .players
            .selected
            .as_ref()
            .is_some_and(|selected| selected == &player.id);
        let id = player.id.clone();
        let raw = player_id_label(&player.id);
        let stable_label = stable_middle_ellipsis(player.label.as_ref(), 30);
        let stable_raw = stable_middle_ellipsis(&raw, 34);
        let quality_label = player.quality.health.label();
        let quality_color = match player.quality.health {
            PlayerRecordHealth::Complete => colors.accent,
            PlayerRecordHealth::Partial => colors.stat_orange_text,
            PlayerRecordHealth::Stub | PlayerRecordHealth::Invalid => colors.danger,
        };
        div()
            .mb(px(4.0))
            .p(px(7.0))
            .overflow_hidden()
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .cursor_pointer()
            .bg(if selected {
                Hsla {
                    a: 0.18,
                    ..colors.accent
                }
            } else {
                transparent_black()
            })
            .hover(|style| {
                style.bg(Hsla {
                    a: 0.56,
                    ..colors.surface_hover
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.open_player_workspace_for_player(
                        id.clone(),
                        PlayerWorkspaceCenter::Inventory,
                        cx,
                    )
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(34.0))
                            .h(px(34.0))
                            .flex_none()
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .overflow_hidden()
                            .bg(Hsla {
                                a: 0.42,
                                ..colors.surface_hover
                            })
                            .child(img("images/map/entity/player.png").w(px(34.0)).h(px(34.0))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child(stable_label),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_size(px(9.0))
                                    .text_color(colors.text_muted)
                                    .child(stable_raw),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .text_size(px(9.0))
                                    .text_color(colors.text_muted)
                                    .child(format!(
                                        "物品 {} · 末影箱 {} · 评分 {}",
                                        player.quality.item_count,
                                        player.quality.ender_item_count,
                                        player.quality.score
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w(px(34.0))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(Hsla {
                                a: 0.14,
                                ..quality_color
                            })
                            .text_size(px(9.0))
                            .text_color(quality_color)
                            .child(quality_label),
                    ),
            )
    }

    pub(super) fn open_player_workspace_for_player(
        &mut self,
        id: PlayerId,
        center: PlayerWorkspaceCenter,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        self.players.context_target = None;
        self.ui_state.active_left_panel = MapViewerLeftPanel::Players;
        self.ui_state.left_panel_open = true;
        self.player_workspace.center = center;
        self.player_workspace.selected_item = None;
        self.player_workspace.inspector_mode = PlayerInspectorMode::Visual;
        self.player_workspace.item_editor_error = None;
        self.ui_state.active_right_panel = MapViewerRightPanel::Player;
        self.ui_state.set_right_panel_open(true);
        self.update_viewport_after_dock_change(cx);
        self.load_player_detail(id, cx);
    }

    pub(super) fn set_player_workspace_center(
        &mut self,
        center: PlayerWorkspaceCenter,
        cx: &mut Context<Self>,
    ) {
        self.player_workspace.center = center;
        cx.notify();
    }

    pub(super) fn render_player_center_workspace(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(detail) = self.players.detail.as_ref() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.bg)
                .text_size(px(12.0))
                .text_color(colors.text_muted)
                .child(if self.players.loading {
                    "正在读取玩家数据..."
                } else {
                    "从左侧选择玩家。"
                });
        };
        let entries = player_inventory_entries(&detail.nbt);
        let metrics = self.player_workspace_metrics();
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.bg)
            .child(self.render_player_workspace_header(colors, detail, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(metrics.outer_padding))
                    .child(match self.player_workspace.center {
                        PlayerWorkspaceCenter::Map => div().into_any_element(),
                        PlayerWorkspaceCenter::Inventory => self
                            .render_inventory_workspace(colors, &entries, cx)
                            .into_any_element(),
                        PlayerWorkspaceCenter::EnderChest => self
                            .render_ender_chest_workspace(colors, &entries, cx)
                            .into_any_element(),
                        PlayerWorkspaceCenter::Equipment => self
                            .render_equipment_workspace(colors, &entries, cx)
                            .into_any_element(),
                    }),
            )
    }

    fn render_player_workspace_header(
        &self,
        colors: &ThemeColors,
        detail: &PlayerDetail,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_id_label(&detail.id)));
        div()
            .min_h(px(50.0))
            .flex_none()
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(Hsla {
                a: CHROME_HAIRLINE_ALPHA,
                ..colors.border
            })
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(7.0))
            .child(
                div()
                    .w(px(30.0))
                    .h(px(30.0))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .overflow_hidden()
                    .child(img("images/map/entity/player.png").w(px(30.0)).h(px(30.0))),
            )
            .child(
                div()
                    .min_w(px(120.0))
                    .max_w(px(280.0))
                    .truncate()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(title),
            )
            .children(
                [
                    PlayerWorkspaceCenter::Map,
                    PlayerWorkspaceCenter::Inventory,
                    PlayerWorkspaceCenter::EnderChest,
                    PlayerWorkspaceCenter::Equipment,
                ]
                .into_iter()
                .map(|center| {
                    workspace_tab_button(
                        colors,
                        center.label(),
                        self.player_workspace.center == center,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_player_workspace_center(center, cx)
                        }),
                    )
                    .into_any_element()
                }),
            )
            .child(div().flex_1())
            .child(status_badge(
                colors,
                format!("{} 个物品", detail.item_count),
            ))
            .child(toolbar_button(colors, "玩家 NBT").on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.open_selected_player_in_editor(cx)),
            ))
    }

    fn render_inventory_workspace(
        &self,
        colors: &ThemeColors,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let metrics = self.player_workspace_metrics();
        div()
            .w_full()
            .max_w(px(620.0))
            .mx_auto()
            .rounded(px(crate::ui::theme::tokens::radius::LG))
            .border_1()
            .border_color(Hsla {
                a: 0.28,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.72,
                ..colors.surface
            })
            .p(px(metrics.panel_padding))
            .flex()
            .flex_col()
            .gap(px(15.0))
            .child(inventory_section_title(
                colors,
                "玩家背包",
                "3 × 9 主背包 + 9 格快捷栏",
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .children((0..3).map(|row| {
                        self.render_slot_row(
                            colors,
                            PlayerInventoryKind::Inventory,
                            (9 + row * 9)..(18 + row * 9),
                            entries,
                            cx,
                        )
                        .into_any_element()
                    })),
            )
            .child(
                div()
                    .pt(px(8.0))
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.18,
                        ..colors.border
                    })
                    .child(self.render_slot_row(
                        colors,
                        PlayerInventoryKind::Inventory,
                        0..9,
                        entries,
                        cx,
                    )),
            )
            .child(self.render_workspace_quick_catalog(colors, cx))
    }

    fn render_ender_chest_workspace(
        &self,
        colors: &ThemeColors,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let metrics = self.player_workspace_metrics();
        div()
            .w_full()
            .max_w(px(620.0))
            .mx_auto()
            .rounded(px(crate::ui::theme::tokens::radius::LG))
            .border_1()
            .border_color(Hsla {
                a: 0.32,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.72,
                ..colors.surface
            })
            .p(px(metrics.panel_padding))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(inventory_section_title(colors, "末影箱", "27 个独立槽位"))
            .children((0..3).map(|row| {
                self.render_slot_row(
                    colors,
                    PlayerInventoryKind::EnderChest,
                    (row * 9)..((row + 1) * 9),
                    entries,
                    cx,
                )
                .into_any_element()
            }))
    }

    fn render_equipment_workspace(
        &self,
        colors: &ThemeColors,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let armor_labels = ["头盔", "胸甲", "护腿", "靴子"];
        let metrics = self.player_workspace_metrics();
        div()
            .w_full()
            .max_w(px(620.0))
            .mx_auto()
            .rounded(px(crate::ui::theme::tokens::radius::LG))
            .border_1()
            .border_color(Hsla {
                a: 0.28,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.72,
                ..colors.surface
            })
            .p(px(metrics.panel_padding))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(inventory_section_title(colors, "装备", "护甲与副手"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .children((0..4).map(|slot| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(self.render_player_inventory_slot(
                                        colors,
                                        PlayerInventoryKind::Armor,
                                        slot,
                                        entries,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(colors.text_muted)
                                            .child(armor_labels[slot as usize]),
                                    )
                                    .into_any_element()
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(7.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_muted)
                                    .child("副手"),
                            )
                            .child(self.render_player_inventory_slot(
                                colors,
                                PlayerInventoryKind::Offhand,
                                0,
                                entries,
                                cx,
                            )),
                    ),
            )
    }

    fn render_slot_row(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        slots: std::ops::Range<i32>,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let metrics = self.player_workspace_metrics();
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(metrics.slot_gap))
            .children(slots.map(|slot| {
                self.render_player_inventory_slot(colors, kind, slot, entries, cx)
                    .into_any_element()
            }))
    }

    fn render_player_inventory_slot(
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
            .id(("player-item-slot", kind.nbt_key(), slot))
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
                this.cursor_move()
                    .on_drag(drag, |info: &PlayerItemDrag, position, _window, cx| {
                        cx.new(|_| info.clone().at(position))
                    })
            })
            .on_drop(
                cx.listener(move |this, drag: &PlayerItemDrag, _window, cx| {
                    this.move_player_workspace_item(drag.source, selection, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, window, cx| {
                    this.select_player_workspace_item(selection, window, cx)
                }),
            )
    }

    fn render_workspace_quick_catalog(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let catalog = self.player_quick_item_catalog();
        div()
            .pt(px(10.0))
            .border_t_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child("常用物品"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child("先选中一个槽位，再点击物品替换/添加"),
                    ),
            )
            .child(div().flex().flex_wrap().gap(px(4.0)).children(
                catalog.into_iter().take(20).map(|entry| {
                    let id = entry.id.to_string();
                    div()
                        .w(px(42.0))
                        .h(px(42.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.28,
                            ..colors.border
                        })
                        .bg(Hsla {
                            a: 0.58,
                            ..colors.surface_hover
                        })
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(img(entry.path).w(px(32.0)).h(px(32.0)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.replace_selected_player_item_with_id(&id, cx)
                            }),
                        )
                        .into_any_element()
                }),
            ))
    }

    pub(super) fn select_player_workspace_item(
        &mut self,
        selection: PlayerItemSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.player_workspace.selected_item = Some(selection);
        self.player_workspace.inspector_mode = PlayerInspectorMode::Visual;
        self.player_workspace.item_editor_error = None;
        self.ui_state.active_right_panel = MapViewerRightPanel::Player;
        self.ui_state.set_right_panel_open(true);
        self.update_viewport_after_dock_change(cx);
        self.populate_player_item_visual_inputs(selection, window, cx);
        self.sync_player_item_raw_editor(cx);
        cx.notify();
    }

    fn selected_workspace_entry(&self) -> Option<PlayerInventoryEntry> {
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
        let item = entry.as_ref().map(|entry| &entry.item.nbt);
        let id = entry
            .as_ref()
            .and_then(|entry| entry.item.name.clone())
            .unwrap_or_default();
        let count = entry
            .as_ref()
            .and_then(|entry| entry.item.count)
            .unwrap_or(1)
            .to_string();
        let damage = entry
            .as_ref()
            .and_then(|entry| entry.item.damage)
            .unwrap_or(0)
            .to_string();
        let custom_name = item.and_then(player_item_custom_name).unwrap_or_default();
        let lore = item.map(item_lore_lines).unwrap_or_default().join(" | ");
        let can_place_on = item
            .map(|item| item_string_list(item, "CanPlaceOn"))
            .unwrap_or_default()
            .join(", ");
        let can_destroy = item
            .map(|item| item_string_list(item, "CanDestroy"))
            .unwrap_or_default()
            .join(", ");
        for (input, value) in [
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
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        let tag = self
            .selected_workspace_entry()
            .map(|entry| entry.item.nbt)
            .unwrap_or_else(|| empty_workspace_item(selection.slot));
        let text = serde_json::to_value(&tag)
            .map(workspace_pretty_json)
            .unwrap_or_else(|_| SharedString::from("{}"));
        self.player_workspace
            .item_editor_state
            .update(cx, |editor, cx| editor.set_value(text, cx));
        self.player_workspace.item_editor_dirty = false;
        self.player_workspace.item_editor_error = None;
    }

    pub(super) fn handle_player_item_editor_event(
        &mut self,
        editor: Entity<CodeEditorState>,
        event: &CodeEditorEvent,
        cx: &mut Context<Self>,
    ) {
        if editor.entity_id() != self.player_workspace.item_editor_state.entity_id() {
            return;
        }
        match event {
            CodeEditorEvent::Change => {
                self.player_workspace.item_editor_dirty = true;
                self.player_workspace.item_editor_error = None;
                cx.notify();
            }
            CodeEditorEvent::PointerInteractionStarted => {
                self.cancel_pointer_captures_for_panel_interaction(
                    "player item editor pointer down",
                    cx,
                );
            }
            CodeEditorEvent::PointerInteractionEnded => {}
            CodeEditorEvent::SaveRequested => self.save_selected_player_item_raw(cx),
            CodeEditorEvent::FormatRequested => self.format_selected_player_item_raw(cx),
        }
    }

    pub(super) fn render_player_right_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(detail) = self.players.detail.as_ref() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .text_color(colors.text_muted)
                .child(if self.players.loading {
                    "正在加载玩家..."
                } else {
                    "从左侧玩家列表选择记录。"
                })
                .into_any_element();
        };
        let title = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_id_label(&detail.id)));
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.0))
                    .flex_none()
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(Hsla {
                        a: CHROME_HAIRLINE_ALPHA,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(title),
                    )
                    .when(self.players.saving, |this| {
                        this.child(status_badge(colors, "正在写入"))
                    })
                    .child(dock_close_button(colors).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.close_right_panel(cx)),
                    )),
            )
            .child(div().flex_1().min_h(px(0.0)).overflow_hidden().child(
                if self.player_workspace.selected_item.is_some() {
                    self.render_selected_player_item_inspector(colors, cx)
                } else {
                    self.render_player_overview_inspector(colors, detail, cx)
                },
            ))
            .into_any_element()
    }

    fn render_player_overview_inspector(
        &self,
        colors: &ThemeColors,
        detail: &PlayerDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .size_full()
            .overflow_y_scrollbar()
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(player_detail_grid(colors, detail))
            .child(self.render_player_quick_actions(colors, cx))
            .child(
                div()
                    .p(px(10.0))
                    .rounded(px(crate::ui::theme::tokens::radius::MD))
                    .bg(Hsla {
                        a: 0.34,
                        ..colors.surface_hover
                    })
                    .text_size(px(11.0))
                    .line_height(px(17.0))
                    .text_color(colors.text_muted)
                    .child("在中间背包 / 末影箱 / 装备界面点击槽位后，右侧会切换到该物品的可视化 Inspector。高级用户可切换完整 NBT。"),
            )
            .into_any_element()
    }

    fn render_selected_player_item_inspector(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selection = self.player_workspace.selected_item.expect("checked above");
        let entry = self.selected_workspace_entry();
        let item_name = entry
            .as_ref()
            .and_then(|entry| entry.item.name.as_deref())
            .unwrap_or("空槽位");
        let texture = entry
            .as_ref()
            .and_then(|entry| self.player_item_texture(entry.item.name.as_deref()));
        let unknown = entry
            .as_ref()
            .map(|entry| unknown_item_field_count(&entry.item.nbt))
            .unwrap_or(0);
        let mode = self.player_workspace.inspector_mode;
        div()
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .p(px(10.0))
                    .border_b_1()
                    .border_color(Hsla {
                        a: CHROME_HAIRLINE_ALPHA,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex_none()
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.28,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: 0.52,
                                ..colors.surface_hover
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(texture.clone(), |this, texture| {
                                this.child(img(texture).w(px(34.0)).h(px(34.0)))
                            })
                            .when(texture.is_none(), |this| {
                                this.child(themed_icon(
                                    lucide_icons::icon_package(),
                                    24.0,
                                    colors.text_muted,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child(item_name.to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_muted)
                                    .child(format!(
                                        "{} · 槽位 {}",
                                        selection.kind.label(),
                                        selection.slot
                                    )),
                            ),
                    )
                    .when(unknown > 0, |this| {
                        this.child(status_badge(colors, format!("保留 {unknown} 个扩展字段")))
                    }),
            )
            .child(
                div()
                    .h(px(38.0))
                    .flex_none()
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        workspace_tab_button(colors, "可视化", mode == PlayerInspectorMode::Visual)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.player_workspace.inspector_mode =
                                        PlayerInspectorMode::Visual;
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        workspace_tab_button(
                            colors,
                            "原始 NBT",
                            mode == PlayerInspectorMode::RawNbt,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.player_workspace.inspector_mode = PlayerInspectorMode::RawNbt;
                                this.sync_player_item_raw_editor(cx);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(div().flex_1())
                    .child(danger_button(colors, "清空槽位").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.clear_selected_player_item_slot(cx)
                        }),
                    )),
            )
            .child(match mode {
                PlayerInspectorMode::Visual => self.render_player_item_visual_form(colors, cx),
                PlayerInspectorMode::RawNbt => self.render_player_item_raw_form(colors, cx),
            })
            .into_any_element()
    }

    fn render_player_item_visual_form(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = self.selected_workspace_entry();
        let enchantments = entry
            .as_ref()
            .map(|entry| player_item_enchantments(&entry.item.nbt))
            .unwrap_or_default();
        div()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scrollbar()
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(player_form_field(
                colors,
                "物品 ID",
                self.player_workspace.item_id.clone(),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(7.0))
                    .child(
                        player_form_field(colors, "数量", self.player_workspace.count.clone())
                            .flex_1(),
                    )
                    .child(
                        player_form_field(colors, "Damage", self.player_workspace.damage.clone())
                            .flex_1(),
                    ),
            )
            .child(player_form_field(
                colors,
                "自定义名称",
                self.player_workspace.custom_name.clone(),
            ))
            .child(player_form_field(
                colors,
                "Lore（使用 | 分隔多行）",
                self.player_workspace.lore.clone(),
            ))
            .child(player_form_field(
                colors,
                "CanPlaceOn（逗号分隔）",
                self.player_workspace.can_place_on.clone(),
            ))
            .child(player_form_field(
                colors,
                "CanDestroy（逗号分隔）",
                self.player_workspace.can_destroy.clone(),
            ))
            .child(
                div()
                    .p(px(8.0))
                    .rounded(px(crate::ui::theme::tokens::radius::MD))
                    .bg(Hsla {
                        a: 0.30,
                        ..colors.surface_hover
                    })
                    .text_size(px(10.0))
                    .line_height(px(16.0))
                    .text_color(colors.text_muted)
                    .child("可视化保存只增量修改 Name / Count / Damage / tag.display / tag.ench / CanPlaceOn / CanDestroy；Block、BlockEntityTag、新版组件与未知字段原样保留。"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(toolbar_button(colors, "保存可视化修改").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.save_selected_player_item_visual(cx)
                        }),
                    ))
                    .child(toolbar_button(colors, "从剪贴板导入").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.import_selected_player_item_from_clipboard(cx)
                        }),
                    )),
            )
            .child(
                div()
                    .pt(px(6.0))
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.18,
                        ..colors.border
                    })
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child("附魔"),
                    )
                    .children(enchantments.iter().map(|enchantment| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_secondary)
                                    .child(format!(
                                        "{} · id {} · lvl {}",
                                        enchant_name(enchantment.id),
                                        enchantment.id,
                                        enchantment.level
                                    )),
                            )
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(7.0))
                            .child(
                                player_form_field(
                                    colors,
                                    "附魔 ID",
                                    self.player_workspace.enchant_id.clone(),
                                )
                                .flex_1(),
                            )
                            .child(
                                player_form_field(
                                    colors,
                                    "等级",
                                    self.player_workspace.enchant_level.clone(),
                                )
                                .flex_1(),
                            ),
                    )
                    .child(toolbar_button(colors, "添加 / 覆盖附魔").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.add_selected_player_item_enchant_from_inputs(cx)
                        }),
                    )),
            )
            .when_some(self.player_workspace.item_editor_error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(colors.danger)
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn render_player_item_raw_form(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(38.0))
                    .flex_none()
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(status_badge(
                        colors,
                        if self.player_workspace.item_editor_dirty {
                            "已修改"
                        } else {
                            "同步"
                        },
                    ))
                    .child(div().flex_1())
                    .child(toolbar_button(colors, "格式化").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.format_selected_player_item_raw(cx)
                        }),
                    ))
                    .child(toolbar_button(colors, "保存 NBT").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.save_selected_player_item_raw(cx)
                        }),
                    )),
            )
            .child(
                div().flex_1().min_h(px(0.0)).overflow_hidden().child(
                    CodeEditor::new(&self.player_workspace.item_editor_state, colors)
                        .size_full()
                        .min_w(px(0.0))
                        .min_h(px(0.0)),
                ),
            )
            .when_some(
                self.player_workspace.item_editor_error.clone(),
                |this, error| {
                    this.child(
                        div()
                            .flex_none()
                            .px(px(10.0))
                            .py(px(6.0))
                            .text_size(px(10.0))
                            .text_color(colors.danger)
                            .child(error),
                    )
                },
            )
            .into_any_element()
    }

    pub(super) fn save_selected_player_item_visual(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        let id = self
            .player_workspace
            .item_id
            .read(cx)
            .value()
            .trim()
            .to_string();
        if id.is_empty() {
            self.player_workspace.item_editor_error = Some(SharedString::from(
                "物品 ID 不能为空；清空槽位请使用“清空槽位”。",
            ));
            cx.notify();
            return;
        }
        let count = match self
            .player_workspace
            .count
            .read(cx)
            .value()
            .trim()
            .parse::<i16>()
        {
            Ok(value) if (1..=i16::from(i8::MAX)).contains(&value) => value as i8,
            _ => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from("数量必须在 1..=127（NBT Byte）范围内"));
                cx.notify();
                return;
            }
        };
        let damage = match self
            .player_workspace
            .damage
            .read(cx)
            .value()
            .trim()
            .parse::<i16>()
        {
            Ok(value) => value,
            Err(_) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from("Damage 必须是 -32768..=32767 的整数"));
                cx.notify();
                return;
            }
        };
        let patch = PlayerVisualItemPatch {
            id: normalize_workspace_item_id(&id),
            count,
            damage,
            custom_name: self
                .player_workspace
                .custom_name
                .read(cx)
                .value()
                .trim()
                .to_string(),
            lore: split_lore(self.player_workspace.lore.read(cx).value().as_ref()),
            can_place_on: split_csv(self.player_workspace.can_place_on.read(cx).value().as_ref()),
            can_destroy: split_csv(self.player_workspace.can_destroy.read(cx).value().as_ref()),
        };
        let mut tag = self
            .selected_workspace_entry()
            .map(|entry| entry.item.nbt)
            .unwrap_or_else(|| empty_workspace_item(selection.slot));
        if let Err(error) = apply_visual_item_patch(&mut tag, selection.slot, &patch) {
            self.player_workspace.item_editor_error = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：可视化编辑", cx);
    }

    pub(super) fn save_selected_player_item_raw(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        let text = self
            .player_workspace
            .item_editor_state
            .read(cx)
            .value()
            .to_string();
        let mut tag = match serde_json::from_str::<NbtTag>(&text) {
            Ok(NbtTag::Compound(compound)) => NbtTag::Compound(compound),
            Ok(_) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from("物品 NBT 根节点必须是 Compound"));
                cx.notify();
                return;
            }
            Err(error) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from(format!("NBT JSON 解析失败: {error}")));
                cx.notify();
                return;
            }
        };
        if let Err(error) = set_workspace_item_slot(&mut tag, selection.slot) {
            self.player_workspace.item_editor_error = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：原始 NBT 编辑", cx);
    }

    pub(super) fn format_selected_player_item_raw(&mut self, cx: &mut Context<Self>) {
        let text = self
            .player_workspace
            .item_editor_state
            .read(cx)
            .value()
            .to_string();
        match serde_json::from_str::<NbtTag>(&text).and_then(serde_json::to_value) {
            Ok(value) => {
                let formatted = workspace_pretty_json(value);
                self.player_workspace
                    .item_editor_state
                    .update(cx, |editor, cx| editor.set_value(formatted, cx));
                self.player_workspace.item_editor_dirty = true;
                self.player_workspace.item_editor_error = None;
            }
            Err(error) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from(format!("NBT JSON 格式化失败: {error}")));
            }
        }
        cx.notify();
    }

    pub(super) fn import_selected_player_item_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            self.status = SharedString::from("请先选择一个背包/末影箱/装备槽位");
            cx.notify();
            return;
        };
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
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
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：剪贴板导入", cx);
    }

    pub(super) fn replace_selected_player_item_with_id(
        &mut self,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = self.player_workspace.selected_item else {
            self.status = SharedString::from("请先点击一个空槽位或已有物品槽位");
            cx.notify();
            return;
        };
        let mut tag = simple_workspace_item(id, selection.slot);
        let _ = set_workspace_item_slot(&mut tag, selection.slot);
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：从原版图标库添加", cx);
    }

    pub(super) fn clear_selected_player_item_slot(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        self.write_player_workspace_slot(selection, None, "玩家物品：清空槽位", cx);
    }

    pub(super) fn add_selected_player_item_enchant_from_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.player_workspace.selected_item else {
            return;
        };
        let Some(entry) = self.selected_workspace_entry() else {
            self.player_workspace.item_editor_error =
                Some(SharedString::from("空槽位需要先添加物品，再设置附魔"));
            cx.notify();
            return;
        };
        let id = match self
            .player_workspace
            .enchant_id
            .read(cx)
            .value()
            .trim()
            .parse::<i16>()
        {
            Ok(value) => value,
            Err(_) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from("附魔 ID 必须是 NBT Short 整数"));
                cx.notify();
                return;
            }
        };
        let level = match self
            .player_workspace
            .enchant_level
            .read(cx)
            .value()
            .trim()
            .parse::<i16>()
        {
            Ok(value) => value,
            Err(_) => {
                self.player_workspace.item_editor_error =
                    Some(SharedString::from("附魔等级必须是 NBT Short 整数"));
                cx.notify();
                return;
            }
        };
        let mut tag = entry.item.nbt;
        if let Err(error) = upsert_workspace_enchant(&mut tag, id, level) {
            self.player_workspace.item_editor_error = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        self.write_player_workspace_slot(selection, Some(tag), "玩家物品：修改附魔", cx);
    }

    fn sync_player_summary_inventory_counts(&mut self, detail: &PlayerDetail) {
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
            + if summary.quality.ender_item_count > 0 {
                5
            } else {
                0
            };
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
                    let history_capture =
                        capture_player_history(&world_path, &id, "玩家物品：拖拽移动/交换");
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

    fn write_player_workspace_slot(
        &mut self,
        selection: PlayerItemSelection,
        replacement: Option<NbtTag>,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
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
        let label_string = label.to_string();
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
                        capture_player_history(&world_path, &id, label_string.clone());
                    let mut data = world
                        .get_player_blocking(&id)
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "玩家记录不存在".to_string())?;
                    replace_player_slot(&mut data.nbt, selection, replacement)?;
                    data = PlayerData::from_nbt(id.clone(), data.nbt)
                        .map_err(|error| error.to_string())?;
                    world
                        .put_player_blocking(&data)
                        .map_err(|error| error.to_string())?;
                    let detail = player_detail_from_data(data)?;
                    if let Ok(capture) = history_capture {
                        complete_after(capture, label_string)?;
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
                        this.player_workspace.item_editor_dirty = false;
                        this.player_workspace.item_editor_error = None;
                        this.sync_player_item_raw_editor(cx);
                        this.status = SharedString::from(
                            "玩家物品已写入 · 未识别 NBT 字段已保留 · 可从历史撤销",
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
}

fn workspace_tab_button(colors: &ThemeColors, label: &'static str, active: bool) -> Div {
    div()
        .px(px(9.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .cursor_pointer()
        .bg(if active {
            Hsla {
                a: 0.18,
                ..colors.accent
            }
        } else {
            transparent_black()
        })
        .hover(|style| {
            style.bg(Hsla {
                a: 0.55,
                ..colors.surface_hover
            })
        })
        .text_size(px(11.0))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(if active {
            colors.accent
        } else {
            colors.text_secondary
        })
        .child(label)
}

fn inventory_section_title(colors: &ThemeColors, title: &'static str, detail: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(colors.text_muted)
                .child(detail),
        )
}

fn player_form_field(colors: &ThemeColors, label: &'static str, input: Entity<InputState>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(10.0))
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .h(px(32.0))
                .px(px(7.0))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla {
                    a: CHROME_HAIRLINE_ALPHA,
                    ..colors.border
                })
                .bg(Hsla {
                    a: CHROME_ELEVATED_ALPHA,
                    ..colors.surface_hover
                })
                .child(
                    Input::new(&input)
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .cleanable(false)
                        .w_full()
                        .h_full()
                        .px(px(0.0))
                        .text_size(px(11.0)),
                ),
        )
}

fn normalize_workspace_item_id(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.contains(':') {
        value
    } else {
        format!("minecraft:{value}")
    }
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_workspace_item_id)
        .collect()
}

fn split_lore(value: &str) -> Vec<String> {
    value
        .split(['|', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn nbt_compound_ref(tag: &NbtTag) -> Option<&indexmap::IndexMap<String, NbtTag>> {
    match tag {
        NbtTag::Compound(compound) => Some(compound),
        _ => None,
    }
}

fn nbt_compound_mut_ref(
    tag: &mut NbtTag,
) -> Result<&mut indexmap::IndexMap<String, NbtTag>, String> {
    match tag {
        NbtTag::Compound(compound) => Ok(compound),
        _ => Err("物品 NBT 根节点必须是 Compound".to_string()),
    }
}

fn ensure_nested_compound<'a>(
    parent: &'a mut indexmap::IndexMap<String, NbtTag>,
    key: &str,
) -> &'a mut indexmap::IndexMap<String, NbtTag> {
    if !matches!(parent.get(key), Some(NbtTag::Compound(_))) {
        parent.insert(key.to_string(), NbtTag::Compound(indexmap::IndexMap::new()));
    }
    match parent.get_mut(key) {
        Some(NbtTag::Compound(compound)) => compound,
        _ => unreachable!(),
    }
}

fn item_lore_lines(item: &NbtTag) -> Vec<String> {
    let Some(item) = nbt_compound_ref(item) else {
        return Vec::new();
    };
    let Some(NbtTag::Compound(tag)) = item.get("tag") else {
        return Vec::new();
    };
    let Some(NbtTag::Compound(display)) = tag.get("display") else {
        return Vec::new();
    };
    let Some(NbtTag::List(lines)) = display.get("Lore") else {
        return Vec::new();
    };
    lines
        .iter()
        .filter_map(|line| match line {
            NbtTag::String(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn item_string_list(item: &NbtTag, key: &str) -> Vec<String> {
    let Some(item) = nbt_compound_ref(item) else {
        return Vec::new();
    };
    let value = item.get(key).or_else(|| {
        item.get("tag")
            .and_then(nbt_compound_ref)
            .and_then(|tag| tag.get(key))
    });
    let Some(NbtTag::List(values)) = value else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match value {
            NbtTag::String(value) => Some(value.clone()),
            _ => None,
        })
        .collect()
}

fn apply_visual_item_patch(
    item: &mut NbtTag,
    slot: i32,
    patch: &PlayerVisualItemPatch,
) -> Result<(), String> {
    let compound = nbt_compound_mut_ref(item)?;
    compound.insert("Name".to_string(), NbtTag::String(patch.id.clone()));
    compound.insert("Count".to_string(), NbtTag::Byte(patch.count));
    compound.insert("Damage".to_string(), NbtTag::Short(patch.damage));
    if let Ok(slot) = i8::try_from(slot) {
        compound.insert("Slot".to_string(), NbtTag::Byte(slot));
    }
    compound
        .entry("WasPickedUp".to_string())
        .or_insert(NbtTag::Byte(0));

    let tag = ensure_nested_compound(compound, "tag");
    let display = ensure_nested_compound(tag, "display");
    if patch.custom_name.is_empty() {
        display.shift_remove("Name");
    } else {
        display.insert(
            "Name".to_string(),
            NbtTag::String(patch.custom_name.clone()),
        );
    }
    if patch.lore.is_empty() {
        display.shift_remove("Lore");
    } else {
        display.insert(
            "Lore".to_string(),
            NbtTag::List(patch.lore.iter().cloned().map(NbtTag::String).collect()),
        );
    }
    patch_compat_string_list(compound, "CanPlaceOn", &patch.can_place_on);
    patch_compat_string_list(compound, "CanDestroy", &patch.can_destroy);
    Ok(())
}

fn patch_compat_string_list(
    compound: &mut indexmap::IndexMap<String, NbtTag>,
    key: &str,
    values: &[String],
) {
    let use_root = compound.contains_key(key);
    if use_root {
        if values.is_empty() {
            compound.shift_remove(key);
        } else {
            compound.insert(
                key.to_string(),
                NbtTag::List(values.iter().cloned().map(NbtTag::String).collect()),
            );
        }
        return;
    }
    let tag = ensure_nested_compound(compound, "tag");
    if values.is_empty() {
        tag.shift_remove(key);
    } else {
        tag.insert(
            key.to_string(),
            NbtTag::List(values.iter().cloned().map(NbtTag::String).collect()),
        );
    }
}

fn empty_workspace_item(slot: i32) -> NbtTag {
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

fn simple_workspace_item(id: &str, slot: i32) -> NbtTag {
    let mut item = empty_workspace_item(slot);
    if let NbtTag::Compound(compound) = &mut item {
        compound.insert(
            "Name".to_string(),
            NbtTag::String(normalize_workspace_item_id(id)),
        );
        compound.insert("Count".to_string(), NbtTag::Byte(1));
    }
    item
}

fn set_workspace_item_slot(item: &mut NbtTag, slot: i32) -> Result<(), String> {
    let slot = i8::try_from(slot).map_err(|_| "物品槽位超出 NBT Byte 范围".to_string())?;
    nbt_compound_mut_ref(item)?.insert("Slot".to_string(), NbtTag::Byte(slot));
    Ok(())
}

fn player_slot_item(player: &NbtTag, selection: PlayerItemSelection) -> Option<NbtTag> {
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

fn replace_player_slot(
    player: &mut NbtTag,
    selection: PlayerItemSelection,
    replacement: Option<NbtTag>,
) -> Result<(), String> {
    let root = nbt_compound_mut_ref(player)?;
    let key = selection.kind.nbt_key().to_string();
    if !root.contains_key(&key) {
        root.insert(key.clone(), NbtTag::List(Vec::new()));
    }
    let list = match root.get_mut(&key) {
        Some(NbtTag::List(list)) => list,
        _ => return Err(format!("玩家 `{}` 不是 NBT List", selection.kind.nbt_key())),
    };
    let index = selection
        .list_index
        .filter(|index| *index < list.len())
        .or_else(|| {
            list.iter().position(|item| {
                nbt_compound_ref(item).and_then(|compound| nbt_number_i32(compound.get("Slot")))
                    == Some(selection.slot)
            })
        });
    match replacement {
        Some(mut item) => {
            set_workspace_item_slot(&mut item, selection.slot)?;
            if let Some(index) = index {
                list[index] = item;
            } else {
                list.push(item);
            }
        }
        None => {
            if let Some(index) = index {
                list[index] = empty_workspace_item(selection.slot);
            }
        }
    }
    Ok(())
}

fn nbt_number_i32(tag: Option<&NbtTag>) -> Option<i32> {
    match tag? {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn upsert_workspace_enchant(item: &mut NbtTag, id: i16, level: i16) -> Result<(), String> {
    let item = nbt_compound_mut_ref(item)?;
    let tag = ensure_nested_compound(item, "tag");
    if !matches!(tag.get("ench"), Some(NbtTag::List(_))) {
        tag.insert("ench".to_string(), NbtTag::List(Vec::new()));
    }
    let Some(NbtTag::List(ench)) = tag.get_mut("ench") else {
        unreachable!();
    };
    if let Some(existing) = ench.iter_mut().find(|entry| {
        nbt_compound_ref(entry).and_then(|compound| nbt_number_i32(compound.get("id")))
            == Some(i32::from(id))
    }) {
        let compound = nbt_compound_mut_ref(existing)?;
        compound.insert("lvl".to_string(), NbtTag::Short(level));
    } else {
        let mut compound = indexmap::IndexMap::new();
        compound.insert("id".to_string(), NbtTag::Short(id));
        compound.insert("lvl".to_string(), NbtTag::Short(level));
        ench.push(NbtTag::Compound(compound));
    }
    Ok(())
}

fn unknown_item_field_count(item: &NbtTag) -> usize {
    let Some(compound) = nbt_compound_ref(item) else {
        return 0;
    };
    let known_root = [
        "Name",
        "Count",
        "Damage",
        "Slot",
        "WasPickedUp",
        "tag",
        "Block",
    ];
    let mut count = compound
        .keys()
        .filter(|key| !known_root.contains(&key.as_str()))
        .count();
    if let Some(NbtTag::Compound(tag)) = compound.get("tag") {
        let known_tag = ["display", "ench", "CanPlaceOn", "CanDestroy"];
        count += tag
            .keys()
            .filter(|key| !known_tag.contains(&key.as_str()))
            .count();
        if let Some(NbtTag::Compound(display)) = tag.get("display") {
            count += display
                .keys()
                .filter(|key| !["Name", "Lore"].contains(&key.as_str()))
                .count();
        }
    }
    count
}

fn parse_workspace_item_import(text: &str, slot: i32) -> Result<NbtTag, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("剪贴板为空".to_string());
    }
    if let Ok(mut tag) = serde_json::from_str::<NbtTag>(text) {
        if !matches!(tag, NbtTag::Compound(_)) {
            return Err("NBT 导入根节点必须是 Compound".to_string());
        }
        set_workspace_item_slot(&mut tag, slot)?;
        return Ok(tag);
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(object) = value.as_object() {
            return simplified_json_item(object, slot);
        }
    }
    let id = find_item_id_in_text(text).ok_or_else(|| {
        "无法识别剪贴板内容。支持完整 NbtTag JSON、简化物品 JSON、/give 或 namespace:item_id"
            .to_string()
    })?;
    Ok(simple_workspace_item(&id, slot))
}

fn simplified_json_item(
    object: &serde_json::Map<String, serde_json::Value>,
    slot: i32,
) -> Result<NbtTag, String> {
    let id = object
        .get("id")
        .or_else(|| object.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "简化物品 JSON 缺少 `id` / `name`".to_string())?;
    let mut item = simple_workspace_item(id, slot);
    let compound = nbt_compound_mut_ref(&mut item)?;
    if let Some(count) = object.get("count").and_then(serde_json::Value::as_i64) {
        let count = count.clamp(1, i64::from(i8::MAX)) as i8;
        compound.insert("Count".to_string(), NbtTag::Byte(count));
    }
    if let Some(damage) = object.get("damage").and_then(serde_json::Value::as_i64) {
        let damage = damage.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        compound.insert("Damage".to_string(), NbtTag::Short(damage));
    }
    let tag = ensure_nested_compound(compound, "tag");
    if let Some(name) = object
        .get("custom_name")
        .or_else(|| object.get("display_name"))
        .and_then(serde_json::Value::as_str)
    {
        ensure_nested_compound(tag, "display")
            .insert("Name".to_string(), NbtTag::String(name.to_string()));
    }
    if let Some(lore) = object.get("lore").and_then(serde_json::Value::as_array) {
        let lines = lore
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(|value| NbtTag::String(value.to_string()))
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            ensure_nested_compound(tag, "display").insert("Lore".to_string(), NbtTag::List(lines));
        }
    }
    if let Some(values) = json_string_array(object.get("can_place_on")) {
        tag.insert(
            "CanPlaceOn".to_string(),
            NbtTag::List(values.into_iter().map(NbtTag::String).collect()),
        );
    }
    if let Some(values) = json_string_array(object.get("can_destroy")) {
        tag.insert(
            "CanDestroy".to_string(),
            NbtTag::List(values.into_iter().map(NbtTag::String).collect()),
        );
    }
    if let Some(enchantments) = object.get("ench").and_then(serde_json::Value::as_array) {
        let mut values = Vec::new();
        for enchantment in enchantments {
            let Some(enchantment) = enchantment.as_object() else {
                continue;
            };
            let Some(id) = enchantment.get("id").and_then(serde_json::Value::as_i64) else {
                continue;
            };
            let level = enchantment
                .get("lvl")
                .or_else(|| enchantment.get("level"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(1);
            let mut value = indexmap::IndexMap::new();
            value.insert(
                "id".to_string(),
                NbtTag::Short(id.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16),
            );
            value.insert(
                "lvl".to_string(),
                NbtTag::Short(level.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16),
            );
            values.push(NbtTag::Compound(value));
        }
        if !values.is_empty() {
            tag.insert("ench".to_string(), NbtTag::List(values));
        }
    }
    Ok(item)
}

fn json_string_array(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(normalize_workspace_item_id)
            .collect(),
    )
}

fn find_item_id_in_text(text: &str) -> Option<String> {
    let mut fallback = None;
    for token in text.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | '`' | ',' | ';' | '[' | ']' | '{' | '}' | ':'
            )
        });
        if token.starts_with("minecraft:") || token.contains(':') && !token.starts_with('@') {
            return Some(normalize_workspace_item_id(token));
        }
        if fallback.is_none()
            && token
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-./".contains(character))
            && !token.starts_with('/')
            && !token.starts_with('@')
            && token.parse::<i64>().is_err()
        {
            fallback = Some(normalize_workspace_item_id(token));
        }
    }
    fallback
}

fn workspace_pretty_json(value: serde_json::Value) -> SharedString {
    SharedString::from(serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()))
}

fn stable_middle_ellipsis(value: &str, max_chars: usize) -> String {
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
}

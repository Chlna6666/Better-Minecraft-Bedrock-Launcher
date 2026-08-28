use super::model::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn render_players_panel(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(10.0))
            .p(px(10.0))
            .child(self.render_player_list_panel(colors, cx))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.38,
                        ..colors.surface_hover
                    })
                    .p(px(10.0))
                    .overflow_y_scrollbar()
                    .child(self.render_player_detail(colors, cx)),
            )
    }

    fn render_player_list_panel(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        div()
            .w(px(320.0))
            .flex_none()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .border_1()
            .border_color(Hsla {
                a: 0.24,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.42,
                ..colors.surface_hover
            })
            .p(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(panel_title(colors, t!("MapViewer.players")))
                    .child(status_badge(
                        colors,
                        t!("MapViewer.player_count", count = self.players.players.len()),
                    ))
                    .child(div().flex_1())
                    .child(toolbar_button(colors, t!("common.refresh")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.refresh_players(cx)),
                    )),
            )
            .child(
                div()
                    .px(px(4.0))
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.player_sort_hint")),
            )
            .child(
                div()
                    .px(px(4.0))
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.player_map_hint")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .when(self.players.players.is_empty(), |this| {
                        this.child(
                            div()
                                .p(px(10.0))
                                .text_size(px(12.0))
                                .line_height(px(18.0))
                                .text_color(colors.text_muted)
                                .child(if self.players.loading {
                                    t!("MapViewer.loading_player_list")
                                } else {
                                    t!("MapViewer.no_player_records")
                                }),
                        )
                    })
                    .children(self.players.players.iter().map(|player| {
                        self.render_player_row(colors, player, cx)
                            .into_any_element()
                    })),
            )
    }

    pub(super) fn render_player_row(
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
        let raw_id = player_id_label(&player.id);
        let i18n = cx.global::<I18n>().clone();
        let kind = match &player.id {
            PlayerId::Local => t!("MapViewer.local").to_string(),
            PlayerId::Xuid(_) => t!("MapViewer.server").to_string(),
            PlayerId::LegacyLevelDat => t!("MapViewer.legacy").to_string(),
            PlayerId::Unknown(_) => t!("MapViewer.other").to_string(),
        };
        let invalid = player.quality.health == PlayerRecordHealth::Invalid;
        let label = localized_player_friendly_label(&i18n, &player.id, !invalid);

        div()
            .mb(px(3.0))
            .px(px(7.0))
            .py(px(7.0))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .cursor(CursorStyle::PointingHand)
            .text_color(if selected {
                colors.text_primary
            } else {
                colors.text_secondary
            })
            .bg(if selected {
                Hsla {
                    a: 0.22,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.surface
                }
            })
            .hover(|style| {
                style.bg(Hsla {
                    a: 0.62,
                    ..colors.surface_hover
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.load_player_detail(id.clone(), cx)
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
                                a: 0.48,
                                ..colors.surface_hover
                            })
                            .child(img("images/map/entity/player.png").w(px(34.0)).h(px(34.0))),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_muted)
                                    .truncate()
                                    .child(raw_id),
                            ),
                    )
                    .child(
                        div()
                            .px(px(5.0))
                            .py(px(2.0))
                            .rounded_full()
                            .bg(if invalid {
                                Hsla {
                                    a: 0.14,
                                    ..colors.danger
                                }
                            } else {
                                Hsla {
                                    a: 0.14,
                                    ..colors.accent
                                }
                            })
                            .text_size(px(9.0))
                            .text_color(if invalid {
                                colors.danger
                            } else {
                                colors.text_secondary
                            })
                            .child(if invalid {
                                t!("MapViewer.invalid")
                            } else {
                                SharedString::from(kind)
                            }),
                    ),
            )
    }

    pub(super) fn render_player_detail(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let Some(detail) = self.players.detail.as_ref() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .text_color(colors.text_muted)
                .child(
                    self.players
                        .error
                        .clone()
                        .unwrap_or_else(|| t!("MapViewer.select_player_editable")),
                );
        };

        let friendly_name = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| {
                localized_player_friendly_label(
                    &i18n,
                    &player.id,
                    player.quality.health != PlayerRecordHealth::Invalid,
                )
            })
            .unwrap_or_else(|| localized_player_friendly_label(&i18n, &detail.id, true));
        let entries = player_inventory_entries(&detail.nbt);
        let catalog = self.player_quick_item_catalog();

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w(px(42.0))
                            .h(px(42.0))
                            .flex_none()
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .overflow_hidden()
                            .bg(Hsla {
                                a: 0.52,
                                ..colors.surface_hover
                            })
                            .child(img("images/map/entity/player.png").w(px(42.0)).h(px(42.0))),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(panel_title(colors, friendly_name))
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_muted)
                                    .child(player_inventory_summary(detail)),
                            ),
                    )
                    .child(div().flex_1())
                    .when(self.players.saving, |this| {
                        this.child(status_badge(colors, t!("MapViewer.writing")))
                    })
                    .child(
                        toolbar_button(colors, t!("MapViewer.advanced_nbt")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.open_selected_player_in_editor(cx)
                            }),
                        ),
                    ),
            )
            .child(self.render_player_quick_actions(colors, &i18n, cx))
            .child(player_detail_grid(colors, detail))
            .child(self.render_player_inventory_section(colors, &entries, cx))
            .child(self.render_player_item_add_section(colors, &catalog, &i18n, cx))
    }

    fn render_player_inventory_section(
        &self,
        colors: &ThemeColors,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let i18n = cx.global::<I18n>().clone();
        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child(t!("MapViewer.items")),
                    )
                    .child(status_badge(
                        colors,
                        t!("MapViewer.valid_item_count", count = entries.len()),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child(t!("MapViewer.empty_slot_nbt_hint")),
                    ),
            )
            .when(entries.is_empty(), |this| {
                this.child(
                    div()
                        .p(px(12.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.24,
                            ..colors.surface_hover
                        })
                        .text_size(px(12.0))
                        .text_color(colors.text_muted)
                        .child(t!("MapViewer.no_player_items")),
                )
            })
            .children(entries.iter().map(|entry| {
                self.render_player_inventory_entry(colors, entry, cx)
                    .into_any_element()
            }))
    }

    fn render_player_inventory_entry(
        &self,
        colors: &ThemeColors,
        entry: &PlayerInventoryEntry,
        cx: &mut Context<Self>,
    ) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let kind = entry.kind;
        let list_index = entry.list_index;
        let name = entry
            .item
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("unknown");
        let custom_name = player_item_custom_name(&entry.item.nbt);
        let lore_count = player_item_lore_count(&entry.item.nbt);
        let enchantments = player_item_enchantments(&entry.item.nbt);
        let texture = self.player_item_texture(Some(name));
        let has_texture = texture.is_some();
        let count = entry
            .item
            .count
            .map_or_else(|| "?".to_string(), |value| value.to_string());
        let damage = entry
            .item
            .damage
            .map_or_else(|| "?".to_string(), |value| value.to_string());
        let slot_label = entry.slot.map_or_else(
            || format!("#{list_index}"),
            |slot| t!("MapViewer.slot_label", slot = slot).to_string(),
        );

        div()
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.22,
                ..colors.surface_hover
            })
            .p(px(8.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.0))
                    .child(
                        div()
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex_none()
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.16,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: 0.44,
                                ..colors.surface
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when_some(texture, |this, path| {
                                this.child(img(path).w(px(34.0)).h(px(34.0)))
                            })
                            .when(!has_texture, |this| {
                                this.child(
                                    div()
                                        .text_size(px(9.0))
                                        .text_color(colors.text_muted)
                                        .child("NBT"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .truncate()
                                    .child(
                                        custom_name
                                            .as_deref()
                                            .filter(|name| !name.trim().is_empty())
                                            .unwrap_or(name)
                                            .to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(colors.text_muted)
                                    .truncate()
                                    .child(name.to_string()),
                            ),
                    )
                    .child(status_badge(
                        colors,
                        format!("{} · {}", kind.label(), slot_label),
                    ))
                    .child(status_badge(colors, format!("x{count} · dmg {damage}"))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        player_item_action_button(colors, t!("MapViewer.decrease_count"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AdjustCount {
                                            kind,
                                            list_index,
                                            delta: -1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.increase_count"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AdjustCount {
                                            kind,
                                            list_index,
                                            delta: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.set_count_64"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::SetCount {
                                            kind,
                                            list_index,
                                            value: 64,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.set_count_127"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::SetCount {
                                            kind,
                                            list_index,
                                            value: 127,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.decrease_damage"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AdjustDamage {
                                            kind,
                                            list_index,
                                            delta: -1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.increase_damage"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AdjustDamage {
                                            kind,
                                            list_index,
                                            delta: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.reset_damage"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::SetDamage {
                                            kind,
                                            list_index,
                                            value: 0,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.duplicate_item"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::DuplicateItem { kind, list_index },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(danger_button(colors, t!("common.delete")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.run_player_item_mutation(
                                PlayerItemMutation::DeleteItem { kind, list_index },
                                cx,
                            )
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        player_item_action_button(colors, t!("MapViewer.name_clipboard"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_player_item_name_from_clipboard(kind, list_index, cx)
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.lore_clipboard"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_player_item_lore_from_clipboard(kind, list_index, cx)
                                }),
                            ),
                    )
                    .when(lore_count > 0, |this| {
                        this.child(status_badge(
                            colors,
                            t!("MapViewer.lore_count", count = lore_count),
                        ))
                    })
                    .when(entry.item.has_tag, |this| {
                        this.child(status_badge(colors, t!("MapViewer.has_tag")))
                    }),
            )
            .child(self.render_player_enchant_editor(colors, kind, list_index, &enchantments, cx))
    }

    fn render_player_enchant_editor(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        list_index: usize,
        enchantments: &[PlayerEnchantEntry],
        cx: &mut Context<Self>,
    ) -> Div {
        let i18n = cx.global::<I18n>().clone();
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(
                        div()
                            .mr(px(3.0))
                            .text_size(px(10.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_muted)
                            .child(t!("MapViewer.enchantments")),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.sharpness_plus_one"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AddEnchant {
                                            kind,
                                            list_index,
                                            id: 9,
                                            level: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.protection_plus_one"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AddEnchant {
                                            kind,
                                            list_index,
                                            id: 0,
                                            level: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.efficiency_plus_one"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AddEnchant {
                                            kind,
                                            list_index,
                                            id: 15,
                                            level: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.unbreaking_plus_one"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AddEnchant {
                                            kind,
                                            list_index,
                                            id: 17,
                                            level: 1,
                                        },
                                        cx,
                                    )
                                }),
                            ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.mending")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.run_player_item_mutation(
                                    PlayerItemMutation::AddEnchant {
                                        kind,
                                        list_index,
                                        id: 26,
                                        level: 1,
                                    },
                                    cx,
                                )
                            }),
                        ),
                    )
                    .child(
                        player_item_action_button(colors, t!("MapViewer.enchant_clipboard"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.add_player_item_enchant_from_clipboard(
                                        kind, list_index, cx,
                                    )
                                }),
                            ),
                    ),
            )
            .children(enchantments.iter().map(|enchantment| {
                let enchant_index = enchantment.list_index;
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .pl(px(4.0))
                    .child(
                        div()
                            .min_w(px(145.0))
                            .text_size(px(10.0))
                            .text_color(colors.text_secondary)
                            .child(format!(
                                "{} · id {} · lvl {}",
                                localized_enchant_name(&i18n, enchantment.id),
                                enchantment.id,
                                enchantment.level
                            )),
                    )
                    .child(player_item_action_button(colors, "−").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.run_player_item_mutation(
                                PlayerItemMutation::AdjustEnchant {
                                    kind,
                                    list_index,
                                    enchant_index,
                                    delta: -1,
                                },
                                cx,
                            )
                        }),
                    ))
                    .child(player_item_action_button(colors, "+").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.run_player_item_mutation(
                                PlayerItemMutation::AdjustEnchant {
                                    kind,
                                    list_index,
                                    enchant_index,
                                    delta: 1,
                                },
                                cx,
                            )
                        }),
                    ))
                    .child(danger_button(colors, t!("MapViewer.remove")).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.run_player_item_mutation(
                                PlayerItemMutation::RemoveEnchant {
                                    kind,
                                    list_index,
                                    enchant_index,
                                },
                                cx,
                            )
                        }),
                    ))
                    .into_any_element()
            }))
            .when(enchantments.is_empty(), |this| {
                this.child(
                    div()
                        .pl(px(4.0))
                        .text_size(px(10.0))
                        .text_color(colors.text_muted)
                        .child(t!("MapViewer.no_enchantments_hint")),
                )
            })
    }

    fn render_player_item_add_section(
        &self,
        colors: &ThemeColors,
        catalog: &[PlayerItemTexture],
        i18n: &I18n,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .mt(px(2.0))
            .pt(px(9.0))
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
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child(t!("MapViewer.add_item")),
                    )
                    .child(status_badge(
                        colors,
                        t!("MapViewer.vanilla_catalog", count = catalog.len()),
                    ))
                    .child(div().flex_1())
                    .child(
                        toolbar_button(colors, t!("MapViewer.item_id_clipboard")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.add_player_item_from_clipboard(cx)
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.item_catalog_hint")),
            )
            .child(
                div()
                    .max_h(px(218.0))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .children(catalog.iter().map(|entry| {
                        let id = entry.id.to_string();
                        div()
                            .w(px(74.0))
                            .h(px(72.0))
                            .p(px(5.0))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.16,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: 0.20,
                                ..colors.surface_hover
                            })
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| {
                                style.bg(Hsla {
                                    a: 0.62,
                                    ..colors.surface_hover
                                })
                            })
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(4.0))
                            .child(img(entry.path.clone()).w(px(34.0)).h(px(34.0)))
                            .child(
                                div()
                                    .w_full()
                                    .text_size(px(9.0))
                                    .text_align(TextAlign::Center)
                                    .text_color(colors.text_muted)
                                    .truncate()
                                    .child(entry.label.clone()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.run_player_item_mutation(
                                        PlayerItemMutation::AddItem {
                                            kind: PlayerInventoryKind::Inventory,
                                            name: id.clone(),
                                        },
                                        cx,
                                    )
                                }),
                            )
                            .into_any_element()
                    })),
            )
            .when(catalog.is_empty(), |this| {
                this.child(
                    div()
                        .p(px(10.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.20,
                            ..colors.surface_hover
                        })
                        .text_size(px(11.0))
                        .text_color(colors.text_muted)
                        .child(t!("MapViewer.no_item_catalog")),
                )
            })
    }

    pub(super) fn render_player_quick_actions(
        &self,
        colors: &ThemeColors,
        i18n: &I18n,
        cx: &mut Context<Self>,
    ) -> Div {
        let pending = self.players.pending_save_confirmation.as_ref();
        let move_label = if pending == Some(&PlayerQuickEdit::MoveToMapCenter) {
            t!("MapViewer.confirm_move")
        } else {
            t!("MapViewer.move_player_to_center")
        };
        let dimension_edit = PlayerQuickEdit::SetDimension(self.dimension);
        let dimension_label_text = if pending == Some(&dimension_edit) {
            t!("MapViewer.confirm_dimension")
        } else {
            t!("MapViewer.set_current_dimension")
        };
        let clear_label = if pending == Some(&PlayerQuickEdit::ClearInventory) {
            t!("MapViewer.confirm_clear_inventory")
        } else {
            t!("MapViewer.clear_inventory")
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .child(toolbar_button(colors, move_label).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.run_player_quick_edit(PlayerQuickEdit::MoveToMapCenter, cx)
                }),
            ))
            .child(toolbar_button(colors, dimension_label_text).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.run_player_quick_edit(PlayerQuickEdit::SetDimension(this.dimension), cx)
                }),
            ))
            .child(danger_button(colors, clear_label).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.run_player_quick_edit(PlayerQuickEdit::ClearInventory, cx)
                }),
            ))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.dangerous_action_hint")),
            )
    }
}

fn player_item_action_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::XS))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.34,
            ..colors.surface
        })
        .hover(|style| {
            style.bg(Hsla {
                a: 0.72,
                ..colors.surface_hover
            })
        })
        .cursor(CursorStyle::PointingHand)
        .text_size(px(10.0))
        .text_color(colors.text_secondary)
        .child(label.into())
}

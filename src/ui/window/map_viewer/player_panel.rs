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
                    .child(panel_title(colors, "玩家"))
                    .child(status_badge(
                        colors,
                        format!("{} 条", self.players.players.len()),
                    ))
                    .child(div().flex_1())
                    .child(toolbar_button(colors, "刷新").on_mouse_down(
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
                    .child("默认排序：本地玩家 → 服务器玩家 → 其他记录 → 无效数据"),
            )
            .child(
                div()
                    .px(px(4.0))
                    .text_size(px(10.0))
                    .line_height(px(15.0))
                    .text_color(colors.text_muted)
                    .child("有效玩家会显示在地图上；保持“玩家”页开启时，单击玩家标记可直接选中。"),
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
                                    "正在读取并校验玩家列表..."
                                } else {
                                    "未读取到玩家记录。"
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
        let kind = match &player.id {
            PlayerId::Local => "本地",
            PlayerId::Xuid(_) => "服务器",
            PlayerId::LegacyLevelDat => "旧版",
            PlayerId::Unknown(_) => "其他",
        };
        let invalid = player.label.as_ref().starts_with("无效记录");

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
                            .child(
                                img("images/map/entity/player.png")
                                    .w(px(34.0))
                                    .h(px(34.0)),
                            ),
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
                                    .child(player.label.clone()),
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
                            .child(if invalid { "无效" } else { kind }),
                    ),
            )
    }

    pub(super) fn render_player_detail(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
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
                        .unwrap_or_else(|| SharedString::from("选择玩家后显示可编辑数据。")),
                );
        };

        let friendly_name = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_friendly_label(&detail.id, true)));
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
                            .child(
                                img("images/map/entity/player.png")
                                    .w(px(42.0))
                                    .h(px(42.0)),
                            ),
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
                        this.child(status_badge(colors, "正在写入..."))
                    })
                    .child(
                        toolbar_button(colors, "高级 NBT 文本").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.open_selected_player_in_editor(cx)
                            }),
                        ),
                    ),
            )
            .child(self.render_player_quick_actions(colors, cx))
            .child(player_detail_grid(colors, detail))
            .child(self.render_player_inventory_section(colors, &entries, cx))
            .child(self.render_player_item_add_section(colors, &catalog, cx))
    }

    fn render_player_inventory_section(
        &self,
        colors: &ThemeColors,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {
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
                            .child("物品"),
                    )
                    .child(status_badge(colors, format!("{} 个有效物品", entries.len())))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child("空槽位不再以 #0/#1… 原始 NBT 行显示"),
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
                        .child("当前玩家没有有效物品。可从下方原版图标库添加。"),
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
        let slot_label = entry
            .slot
            .map_or_else(|| format!("#{}", list_index), |slot| format!("槽位 {slot}"));

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
                    .child(player_item_action_button(colors, "数量 −").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "数量 +").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "x64").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "x127").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "Damage −").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "Damage +").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "Damage 0").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "复制物品").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.run_player_item_mutation(
                                PlayerItemMutation::DuplicateItem { kind, list_index },
                                cx,
                            )
                        }),
                    ))
                    .child(
                        danger_button(colors, "删除").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.run_player_item_mutation(
                                    PlayerItemMutation::DeleteItem { kind, list_index },
                                    cx,
                                )
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(5.0))
                    .child(player_item_action_button(colors, "名称 ← 剪贴板").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_player_item_name_from_clipboard(kind, list_index, cx)
                        }),
                    ))
                    .child(player_item_action_button(colors, "Lore ← 剪贴板").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_player_item_lore_from_clipboard(kind, list_index, cx)
                        }),
                    ))
                    .when(lore_count > 0, |this| {
                        this.child(status_badge(colors, format!("Lore {lore_count} 行")))
                    })
                    .when(entry.item.has_tag, |this| {
                        this.child(status_badge(colors, "含 tag"))
                    }),
            )
            .child(self.render_player_enchant_editor(
                colors,
                kind,
                list_index,
                &enchantments,
                cx,
            ))
    }

    fn render_player_enchant_editor(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        list_index: usize,
        enchantments: &[PlayerEnchantEntry],
        cx: &mut Context<Self>,
    ) -> Div {
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
                            .child("附魔"),
                    )
                    .child(player_item_action_button(colors, "锋利 +1").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "保护 +1").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "效率 +1").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "耐久 +1").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "经验修补").on_mouse_down(
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
                    ))
                    .child(player_item_action_button(colors, "id:等级 ← 剪贴板").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.add_player_item_enchant_from_clipboard(kind, list_index, cx)
                        }),
                    )),
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
                                enchant_name(enchantment.id),
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
                    .child(danger_button(colors, "移除").on_mouse_down(
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
                        .child("无附魔。高级值可直接粘贴，例如 `9:32767`；NBT Short 范围内均允许。"),
                )
            })
    }

    fn render_player_item_add_section(
        &self,
        colors: &ThemeColors,
        catalog: &[PlayerItemTexture],
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
                            .child("添加物品"),
                    )
                    .child(status_badge(
                        colors,
                        format!("原版图标库 {}", catalog.len()),
                    ))
                    .child(div().flex_1())
                    .child(
                        toolbar_button(colors, "物品 ID ← 剪贴板").on_mouse_down(
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
                    .child(
                        "图标直接读取当前实例 data/resource_packs/vanilla/textures/items。点击图标添加；\
                         自定义/新版/非正常物品可复制 `namespace:item_id` 后使用右侧按钮。",
                    ),
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
                        .child(
                            "当前实例未找到 vanilla/textures/items；仍可通过剪贴板物品 ID 添加，\
                             但不会显示原版图标。",
                        ),
                )
            })
    }

    pub(super) fn render_player_quick_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let pending = self.players.pending_save_confirmation.as_ref();
        let move_label = if pending == Some(&PlayerQuickEdit::MoveToMapCenter) {
            "确认移动"
        } else {
            "移到地图中心"
        };
        let dimension_edit = PlayerQuickEdit::SetDimension(self.dimension);
        let dimension_label_text = if pending == Some(&dimension_edit) {
            "确认维度"
        } else {
            "设为当前维度"
        };
        let clear_label = if pending == Some(&PlayerQuickEdit::ClearInventory) {
            "确认清空背包"
        } else {
            "清空主背包"
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
                    .child("危险操作需要二次点击；所有写入都会进入地图历史。"),
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

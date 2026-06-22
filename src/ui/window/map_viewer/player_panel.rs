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
            .child(
                div()
                    .w(px(280.0))
                    .flex_none()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .rounded(px(6.0))
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
                            .child(panel_title(colors, "玩家记录"))
                            .child(div().flex_1())
                            .child(toolbar_button(colors, "刷新").on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.refresh_players(cx)),
                            )),
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
                                            "正在加载玩家列表..."
                                        } else {
                                            "未读取到玩家记录。"
                                        }),
                                )
                            })
                            .children(self.players.players.iter().map(|player| {
                                self.render_player_row(colors, player, cx)
                                    .into_any_element()
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .rounded(px(6.0))
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
        div()
            .px(px(8.0))
            .py(px(7.0))
            .rounded(px(5.0))
            .cursor(CursorStyle::PointingHand)
            .text_size(px(12.0))
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
            .child(player.label.clone())
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
                        .unwrap_or_else(|| SharedString::from("选择玩家后显示数据预览。")),
                );
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(panel_title(colors, player_id_label(&detail.id)))
                    .child(status_badge(colors, format!("物品 {}", detail.item_count)))
                    .child(div().flex_1())
                    .child(toolbar_button(colors, "高级NBT").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.open_selected_player_in_editor(cx)
                        }),
                    )),
            )
            .child(self.render_player_quick_actions(colors, cx))
            .child(player_detail_grid(colors, detail))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_muted)
                    .child("背包 / 物品"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .children(
                        detail
                            .items
                            .iter()
                            .take(96)
                            .enumerate()
                            .map(|(index, item)| {
                                render_player_item_row(colors, index, item).into_any_element()
                            }),
                    )
                    .when(detail.items.is_empty(), |this| {
                        this.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_muted)
                                .child("没有解析到物品。"),
                        )
                    }),
            )
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
            "移到中心"
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
            "清空背包"
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
            .when(!self.professional.write_mode, |this| {
                this.child(status_badge(colors, "写入前需开启写入模式"))
            })
    }
}

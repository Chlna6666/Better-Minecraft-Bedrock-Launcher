use super::super::players::player_id_label;
use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn reset_map_player_search_scroll(&self) {
        self.map_image
            .player_scroll
            .scroll_to_item(0, ScrollStrategy::Top);
    }

    pub(in super::super) fn render_map_player_picker(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let query = self
            .map_image
            .player_search
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let mut players = vec![(
            PlayerId::Local,
            SharedString::from("本地玩家 · ~local_player"),
        )];
        players.extend(
            self.players
                .players
                .iter()
                .filter(|player| {
                    player.quality.health != PlayerRecordHealth::Invalid
                        && player.id.storage_key().is_some()
                        && player.id != PlayerId::Local
                })
                .map(|player| (player.id.clone(), player.label.clone())),
        );
        let total = players.len();
        let selected = self.map_image.selected_player.clone();
        let selected_label = players.iter().find(|(id, _)| *id == selected).map_or_else(
            || player_id_label(&selected),
            |(_, label)| label.to_string(),
        );
        players.retain(|(id, label)| matches_player_query(label, id, &query));
        let count = players.len();
        let view = cx.entity();
        let row_colors = *colors;
        let list = uniform_list("map-import-players", count, move |range, _window, _cx| {
            range
                .map(|index| {
                    let (id, label) = &players[index];
                    let player = id.clone();
                    let view = view.clone();
                    div()
                        .h(px(36.0))
                        .w_full()
                        .min_w(px(0.0))
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .bg(if *id == selected {
                            row_colors.surface_hover
                        } else {
                            row_colors.surface
                        })
                        .cursor_pointer()
                        .text_size(px(12.0))
                        .text_color(row_colors.text_primary)
                        .child(div().min_w(px(0.0)).truncate().child(label.clone()))
                        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                            view.update(cx, |this, cx| {
                                this.set_map_image_player(player.clone(), cx)
                            });
                            cx.stop_propagation();
                        })
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(self.map_image.player_scroll.clone())
        .size_full();
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .min_w(px(0.0))
            .child(generator_section_title(colors, "目标玩家"))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .truncate()
                    .child(format!("已选：{selected_label}")),
            )
            .child(
                Input::new(&self.map_image.player_search)
                    .cleanable(true)
                    .w_full()
                    .h(px(32.0)),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child(format!("{count} / {total} 位玩家")),
            )
            .child(
                div()
                    .h(px(180.0))
                    .flex_none()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors.border)
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(list),
            )
            .when(count == 0, |panel| {
                panel.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.text_secondary)
                        .child("没有匹配的玩家；可清空搜索"),
                )
            })
    }
}

fn matches_player_query(label: &str, id: &PlayerId, query: &str) -> bool {
    query.is_empty()
        || label.to_lowercase().contains(query)
        || player_id_label(id).to_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use super::{PlayerId, matches_player_query};

    #[test]
    fn player_search_matches_server_record_without_needing_its_display_name() {
        let player = PlayerId::Xuid("430db2e0-8790-4dba-99dc-5d92102c93ac".to_owned());
        assert!(matches_player_query("服务器玩家", &player, "8790-4dba"));
        assert!(matches_player_query(
            "服务器玩家",
            &player,
            "player_430db2e0"
        ));
        assert!(!matches_player_query("服务器玩家", &player, "missing"));
    }

    #[test]
    fn player_search_matches_local_record_and_names_without_changing_selection() {
        assert!(matches_player_query(
            "本地玩家",
            &PlayerId::Local,
            "~local_player"
        ));
        assert!(matches_player_query("ALICE", &PlayerId::Local, "alice"));
        assert!(!matches_player_query(
            "本地玩家",
            &PlayerId::Local,
            "missing"
        ));
    }
}

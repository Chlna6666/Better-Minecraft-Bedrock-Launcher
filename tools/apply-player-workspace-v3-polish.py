from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def edit(path, replacements):
    p = ROOT / path
    text = p.read_text(encoding="utf-8")
    for old, new in replacements:
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected one anchor, got {count}: {old[:120]!r}")
        text = text.replace(old, new, 1)
    p.write_text(text, encoding="utf-8", newline="\n")

edit("src/ui/window/map_viewer/player_workspace.rs", [
    (
'''                    .when(self.players.loading, |this| {
                        this.child(
                            div()
                                .p(px(8.0))
                                .text_size(px(11.0))
                                .text_color(colors.text_muted)
                                .child("正在读取并校验玩家记录..."),
                        )
                    })''',
'''                    .when(self.players.loading && self.players.players.is_empty(), |this| {
                        this.child(
                            div()
                                .p(px(8.0))
                                .text_size(px(11.0))
                                .text_color(colors.text_muted)
                                .child("正在读取并校验玩家记录..."),
                        )
                    })'''),
    (
'''                            .child("先选中一个槽位，再点击物品替换/添加"),''',
'''                            .child("点击可替换已选槽位，也可直接拖到背包 / 末影箱 / 装备槽"),'''),
    (
'''                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        this.replace_selected_player_item_with_id(&click_id, cx)
                                    }),
                                )''',
'''                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        this.replace_selected_player_item_with_id(&click_id, cx)
                                    }),
                                )'''),
    (
'''        let title = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_id_label(&detail.id)));
        div()
            .size_full()
            .min_w(px(0.0))''',
'''        let title = self
            .players
            .players
            .iter()
            .find(|player| player.id == detail.id)
            .map(|player| player.label.clone())
            .unwrap_or_else(|| SharedString::from(player_id_label(&detail.id)));
        let title = SharedString::from(stable_middle_ellipsis(title.as_ref(), 42));
        div()
            .size_full()
            .min_w(px(0.0))'''),
    (
'''                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))''',
'''                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))'''),
    (
'''            .child(div().flex_1())
            .child(status_badge(
                colors,
                format!("{} 个物品", detail.item_count),
            ))''',
'''            .child(div().flex_1())
            .when(!self.player_workspace.multi_selected_items.is_empty(), |this| {
                this.child(status_badge(
                    colors,
                    format!("已选 {} 项", self.player_workspace.multi_selected_items.len()),
                ))
            })
            .child(status_badge(
                colors,
                format!("{} 个物品", detail.item_count),
            ))''')
])

edit("src/ui/window/map_viewer/interactions.rs", [
    (
'''            } else if let Some(id) = self.players.players.first().map(|p| p.id.clone()) {
                self.player_workspace.open_first_after_refresh = false;
                self.open_player_workspace_for_player(id, PlayerWorkspaceCenter::Inventory, cx);
                return;
            }''',
'''            } else if let Some(id) = preferred_player_id(&self.players.players) {
                self.player_workspace.open_first_after_refresh = false;
                self.open_player_workspace_for_player(id, PlayerWorkspaceCenter::Inventory, cx);
                return;
            }''')
])

edit("src/ui/window/map_viewer/players.rs", [
    (
'''                        if !selected_still_exists {
                            this.players.selected =
                                this.players.players.first().map(|player| player.id.clone());
                        }''',
'''                        if !selected_still_exists {
                            this.players.selected = preferred_player_id(&this.players.players);
                        }'''),
    (
'''                            if let Some(id) = this.players.players.first().map(|p| p.id.clone()) {
                                this.open_player_workspace_for_player(
                                    id,
                                    PlayerWorkspaceCenter::Inventory,
                                    cx,
                                );
                            }''',
'''                            if let Some(id) = preferred_player_id(&this.players.players) {
                                this.open_player_workspace_for_player(
                                    id,
                                    PlayerWorkspaceCenter::Inventory,
                                    cx,
                                );
                            }'''),
    (
'''pub(super) fn player_id_label(id: &PlayerId) -> String {
    match id {''',
'''pub(super) fn preferred_player_id(players: &[PlayerSummary]) -> Option<PlayerId> {
    players
        .iter()
        .find(|player| matches!(&player.id, PlayerId::Local))
        .or_else(|| players.first())
        .map(|player| player.id.clone())
}

pub(super) fn player_id_label(id: &PlayerId) -> String {
    match id {''')
])

from pathlib import Path

path = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/player_workspace.rs"
text = path.read_text(encoding="utf-8")
old = '''    fn render_player_inventory_slot(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        slot: i32,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> Div {'''
new = '''    fn render_player_inventory_slot(
        &self,
        colors: &ThemeColors,
        kind: PlayerInventoryKind,
        slot: i32,
        entries: &[PlayerInventoryEntry],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {'''
if old not in text:
    raise SystemExit("render_player_inventory_slot signature missing")
text = text.replace(old, new, 1)
old = '''                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x2b2620)),'''
new = '''                                .text_size(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0x2b2620))
                                .child(self.count.to_string()),'''
if old not in text:
    raise SystemExit("drag preview count anchor missing")
text = text.replace(old, new, 1)
path.write_text(text, encoding="utf-8", newline="\n")
print("player drag return type and preview count fixed")

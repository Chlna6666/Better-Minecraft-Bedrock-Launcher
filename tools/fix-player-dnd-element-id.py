from pathlib import Path

path = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/player_workspace.rs"
text = path.read_text(encoding="utf-8")
old = '''        cx: &mut Context<Self>,
    ) -> Div {
        let metrics = self.player_workspace_metrics();'''
new = '''        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let metrics = self.player_workspace_metrics();'''
if old not in text:
    raise SystemExit("slot return type anchor missing")
text = text.replace(old, new, 1)
old = '''        let icon_size = (metrics.slot_size * 0.72).clamp(20.0, 38.0);
        div()
            .id(("player-item-slot", kind.nbt_key(), slot))'''
new = '''        let icon_size = (metrics.slot_size * 0.72).clamp(20.0, 38.0);
        let kind_index = match kind {
            PlayerInventoryKind::Inventory => 0usize,
            PlayerInventoryKind::Armor => 1,
            PlayerInventoryKind::Offhand => 2,
            PlayerInventoryKind::EnderChest => 3,
        };
        let slot_element_id = kind_index
            .saturating_mul(256)
            .saturating_add(usize::try_from(slot.max(0)).unwrap_or_default());
        div()
            .id(("player-item-slot", slot_element_id))'''
if old not in text:
    raise SystemExit("slot id anchor missing")
text = text.replace(old, new, 1)
path.write_text(text, encoding="utf-8", newline="\n")
print("player drag element id fixed")

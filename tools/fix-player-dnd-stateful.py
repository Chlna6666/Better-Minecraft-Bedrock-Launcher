from pathlib import Path

path = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/player_workspace.rs"
text = path.read_text(encoding="utf-8")
old = '''        div()
            .relative()
            .w(px(metrics.slot_size))'''
new = '''        div()
            .id(("player-item-slot", kind.nbt_key(), slot))
            .relative()
            .w(px(metrics.slot_size))'''
if old not in text:
    raise SystemExit("player workspace slot anchor missing")
path.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
print("player drag slot made stateful")

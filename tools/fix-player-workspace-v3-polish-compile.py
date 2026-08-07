from pathlib import Path
p = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/interactions.rs"
text = p.read_text(encoding="utf-8")
old = "} else if let Some(id) = preferred_player_id(&self.players.players) {"
new = "} else if let Some(id) = super::players::preferred_player_id(&self.players.players) {"
if text.count(old) != 1:
    raise SystemExit(f"expected one preferred_player_id call, got {text.count(old)}")
p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch(path, replacements):
    file = ROOT / path
    text = file.read_text(encoding="utf-8")
    for old, new in replacements:
        if old not in text:
            raise SystemExit(f"{path}: missing patch anchor: {old[:120]!r}")
        text = text.replace(old, new)
    file.write_text(text, encoding="utf-8", newline="\n")


patch(
    "src/ui/window/map_viewer/player_workspace.rs",
    [
        (
            "Input::new(self.player_workspace.search.clone())",
            "Input::new(&self.player_workspace.search)",
        ),
        ("Input::new(input)", "Input::new(&input)"),
        ("pretty_json", "workspace_pretty_json"),
    ],
)

workspace = ROOT / "src/ui/window/map_viewer/player_workspace.rs"
text = workspace.read_text(encoding="utf-8")
if "fn workspace_pretty_json(" not in text:
    text += '''\nfn workspace_pretty_json(value: serde_json::Value) -> SharedString {\n    SharedString::from(\n        serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()),\n    )\n}\n'''
workspace.write_text(text, encoding="utf-8", newline="\n")

patch(
    "src/ui/window/map_viewer/players.rs",
    [
        ("score: i16::MIN,", "score: -32767,"),
        (
            '''    let completeness = i16::from(has_unique_id)\n        + i16::from(has_position)\n        + i16::from(has_dimension)\n        + i16::from(has_inventory);''',
            '''    let completeness = (if has_unique_id { 1 } else { 0 })\n        + (if has_position { 1 } else { 0 })\n        + (if has_dimension { 1 } else { 0 })\n        + (if has_inventory { 1 } else { 0 });''',
        ),
    ],
)

print("player workspace v2 compile fixes applied")

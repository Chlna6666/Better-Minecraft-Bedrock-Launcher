from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch(path, old, new):
    file = ROOT / path
    text = file.read_text(encoding="utf-8")
    if old not in text:
        raise SystemExit(f"{path}: missing patch anchor: {old[:160]!r}")
    file.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


patch(
    "src/ui/window/map_viewer/player_workspace.rs",
    "use lucide_gpui::icons as lucide_icons;\n",
    "use lucide_gpui::icons as lucide_icons;\nuse gpui::StatefulInteractiveElement as _;\n",
)

patch(
    "src/ui/window/map_viewer/players.rs",
    "                        label,\n                    });",
    "                        label,\n                        player_id: Some(detail.id.clone()),\n                    });",
)

print("player overlay compile fixes applied")

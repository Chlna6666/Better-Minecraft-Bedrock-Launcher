from __future__ import annotations

import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASE = "9fab27923410349e13d48c8c7e6735799a648b8b"


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def remove_balanced_call(text: str, start: int) -> str:
    paren = text.find("(", start)
    if paren < 0:
        raise RuntimeError("call opening parenthesis not found")
    depth = 0
    in_string = False
    escaped = False
    end = None
    for index in range(paren, len(text)):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                end = index + 1
                break
    if end is None:
        raise RuntimeError("unterminated call")
    while end < len(text) and text[end] in " \t\r\n":
        end += 1
    return text[:start] + text[end:]


def restore_panels() -> None:
    subprocess.run(
        ["git", "fetch", "origin", BASE, "--depth=1"],
        cwd=ROOT,
        check=True,
    )
    result = subprocess.run(
        ["git", "show", f"{BASE}:src/ui/window/map_viewer/panels.rs"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
    )
    text = result.stdout.decode("utf-8")

    probe_label = text.find('"Probe 诊断')
    if probe_label < 0:
        raise RuntimeError("probe diagnostics label not found in source panels")
    probe_child = text.rfind("            .child(", 0, probe_label)
    if probe_child < 0:
        raise RuntimeError("probe diagnostics child call not found")
    text = remove_balanced_call(text, probe_child)

    events_ref = text.find("self.manifest_probe_diagnostics")
    if events_ref < 0:
        raise RuntimeError("probe events reference not found in source panels")
    events_call = text.rfind("            .children(", 0, events_ref)
    if events_call < 0:
        raise RuntimeError("probe events children call not found")
    text = remove_balanced_call(text, events_call)

    # The status bar now reports the explicit final empty state rather than
    # treating corruption/invalid records as ordinary empty map tiles.
    text = text.replace(
        "                        self.tile_manager.invalid_count(),\n                        self.tile_reveal_state.ready_batches,",
        "                        self.tile_manager.empty_count(),\n                        self.tile_reveal_state.ready_batches,",
        1,
    )
    text = text.replace(
        "    if view.tile_manager.invalid_count() > 0 {\n        return format!(\"空 {}\", view.tile_manager.invalid_count());\n    }",
        "    if view.tile_manager.empty_count() > 0 {\n        return format!(\"空 {}\", view.tile_manager.empty_count());\n    }",
        1,
    )
    write("src/ui/window/map_viewer/panels.rs", text)


def clean_model() -> None:
    path = "src/ui/window/map_viewer/model.rs"
    text = read(path)
    pattern = re.compile(
        r"\nimpl Default for ManifestProbeDiagnostics \{.*?\n\}\n",
        re.S,
    )
    text, count = pattern.subn("\n", text, count=1)
    if count != 1:
        raise RuntimeError(f"orphan ManifestProbeDiagnostics impl count={count}")
    write(path, text)


def clean_lifecycle() -> None:
    path = "src/ui/window/map_viewer/lifecycle.rs"
    text = read(path)

    text = replace_once(
        text,
        """        Some(positions) if positions.is_empty() => !tile_manager
            .entries
            .get(&coord)
            .is_some_and(|entry| entry.state == TileLoadState::Invalid),""",
        """        Some(positions) if positions.is_empty() => !tile_manager
            .entries
            .get(&coord)
            .is_some_and(|entry| {
                matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)
            }),""",
        "empty indexed tile final states",
    )
    text = replace_once(
        text,
        """                || (entry.state == TileLoadState::Loaded && entry.image.is_some())
                || entry.state == TileLoadState::Invalid""",
        """                || (entry.state == TileLoadState::Loaded && entry.image.is_some())
                || matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)""",
        "occupied indexed tile final states",
    )

    text = text.replace(
        """                                        this.mark_occupancy_tile_empty(coord, cx);
                                        changed_tiles.push(coord);""",
        """                                        Self::drop_render_image(
                                            this.tile_manager.mark_empty(coord),
                                            cx,
                                        );
                                        this.available_tiles.remove(&coord);
                                        this.tile_chunk_index.remove(&coord);
                                        changed_tiles.push(coord);""",
    )
    text = re.sub(
        r"(?m)^\s*this\.occupancy_scanned_tiles\.insert\(coord\);\n",
        "",
        text,
    )

    old_empty = """                            Self::drop_render_image(
                                this.tile_manager
                                    .mark_invalid(coord, SharedString::from(message)),
                                cx,
                            );
                            this.available_tiles.remove(&coord);
                            this.tile_chunk_index
                                .insert(coord, TileChunkPositions::from(Vec::<ChunkPos>::new()));"""
    new_empty = """                            Self::drop_render_image(
                                this.tile_manager.mark_empty(coord),
                                cx,
                            );
                            this.available_tiles.remove(&coord);
                            self::drop_empty_index_entry(&mut this.tile_chunk_index, coord);"""
    # Use a simple direct remove; keep the replacement separate to avoid
    # introducing a compatibility helper.
    new_empty = new_empty.replace(
        "self::drop_empty_index_entry(&mut this.tile_chunk_index, coord);",
        "this.tile_chunk_index.remove(&coord);",
    )
    if old_empty not in text:
        raise RuntimeError("TileRenderEvent::Empty old branch not found")
    text = text.replace(old_empty, new_empty, 1)

    old_failed_empty = """                                    Self::drop_render_image(
                                        this.tile_manager
                                            .mark_invalid(coord, SharedString::from(message)),
                                        cx,
                                    );"""
    new_failed_empty = """                                    Self::drop_render_image(
                                        this.tile_manager.mark_empty(coord),
                                        cx,
                                    );
                                    this.available_tiles.remove(&coord);
                                    this.tile_chunk_index.remove(&coord);"""
    if old_failed_empty not in text:
        raise RuntimeError("no-renderable-chunks invalid branch not found")
    text = text.replace(old_failed_empty, new_failed_empty, 1)

    text = text.replace(
        """                                        TileLoadState::Loaded
                                            | TileLoadState::Queued
                                            | TileLoadState::Failed
                                            | TileLoadState::Invalid,""",
        """                                        TileLoadState::Loaded
                                            | TileLoadState::Empty
                                            | TileLoadState::Queued
                                            | TileLoadState::Failed
                                            | TileLoadState::Invalid,""",
        1,
    )
    write(path, text)


def clean_renderer_warning() -> None:
    path = "crates/bedrock-render/src/renderer/pipeline.rs"
    text = read(path)
    text = text.replace(
        "CancelFlag as WorldCancelFlag, ChunkBlockEntity, ChunkBounds, ChunkData, ChunkDataRequest,",
        "CancelFlag as WorldCancelFlag, ChunkBlockEntity, ChunkData, ChunkDataRequest,",
        1,
    )
    write(path, text)


def main() -> None:
    restore_panels()
    clean_model()
    clean_lifecycle()
    clean_renderer_warning()


if __name__ == "__main__":
    main()

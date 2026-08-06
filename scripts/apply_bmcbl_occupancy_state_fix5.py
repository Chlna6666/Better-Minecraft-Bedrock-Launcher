from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def item_end(text: str, brace: int) -> int:
    depth = 0
    in_string = False
    escaped = False
    for index in range(brace, len(text)):
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
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    raise RuntimeError("unterminated Rust item")


def item_start(text: str, position: int) -> int:
    start = text.rfind("\n", 0, position) + 1
    cursor = start
    while cursor > 0:
        previous_end = cursor - 1
        previous_start = text.rfind("\n", 0, previous_end) + 1
        line = text[previous_start:previous_end].strip()
        if line.startswith("#[") or line.startswith("///") or line == "":
            start = previous_start
            cursor = previous_start
        else:
            break
    return start


def remove_function(text: str, name: str) -> str:
    pattern = re.compile(
        rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\("
    )
    while True:
        match = pattern.search(text)
        if match is None:
            return text
        brace = text.find("{", match.end())
        start = item_start(text, match.start())
        end = item_end(text, brace)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]


def remove_struct(text: str, name: str) -> str:
    pattern = re.compile(rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+{re.escape(name)}\b")
    match = pattern.search(text)
    if match is None:
        return text
    brace = text.find("{", match.end())
    start = item_start(text, match.start())
    end = item_end(text, brace)
    while end < len(text) and text[end] in " \t\r\n":
        end += 1
    return text[:start] + text[end:]


cache_path = ROOT / "crates/bedrock-render/src/renderer/cache.rs"
cache = cache_path.read_text(encoding="utf-8")
cache = re.sub(r"(?m)^static CACHE_ATOMIC_WRITE_ID:.*\n", "", cache)
for function in [
    "write_atomic_bytes",
    "cache_temp_path",
    "cleanup_temp_manifest_cache",
    "push_u8",
    "u8",
]:
    cache = remove_function(cache, function)
cache_path.write_text(cache, encoding="utf-8")

pipeline_path = ROOT / "crates/bedrock-render/src/renderer/pipeline.rs"
pipeline = pipeline_path.read_text(encoding="utf-8")
pipeline = remove_function(pipeline, "wait_if_paused")
pipeline = remove_struct(pipeline, "TileBounds")
for function in [
    "tile_bounds_from_coords",
    "tile_coords_from_bounds",
    "chunk_bounds_from_positions",
    "check_render_control_cancelled",
]:
    pipeline = remove_function(pipeline, function)
pipeline_path.write_text(pipeline, encoding="utf-8")

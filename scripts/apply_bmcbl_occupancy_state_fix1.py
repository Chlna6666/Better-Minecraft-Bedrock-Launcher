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


def remove_old_tests(text: str) -> str:
    needles = (
        "PendingManifest",
        "pending_manifest",
        "manifest_probe",
        "TileManifest",
        "tile_manifest",
    )
    cursor = 0
    attribute_pattern = re.compile(r"(?m)^\s*#\[[^\n]*test\]\s*$")
    while True:
        match = attribute_pattern.search(text, cursor)
        if match is None:
            return text
        fn_match = re.search(r"(?m)^\s*fn\s+\w+\s*\(", text[match.end():])
        if fn_match is None:
            return text
        function_position = match.end() + fn_match.start()
        brace = text.find("{", function_position)
        if brace < 0:
            return text
        end = item_end(text, brace)
        block = text[match.start():end]
        if not any(needle in block for needle in needles):
            cursor = end
            continue
        start = item_start(text, match.start())
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]
        cursor = start


def remove_named_function(text: str, name: str) -> str:
    pattern = re.compile(rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+{re.escape(name)}\s*\(")
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


tests_path = ROOT / "src/ui/window/map_viewer/tests.rs"
tests = tests_path.read_text(encoding="utf-8")
tests_path.write_text(remove_old_tests(tests), encoding="utf-8")

state_path = ROOT / "src/ui/window/map_viewer/tile_state.rs"
state = state_path.read_text(encoding="utf-8")
state = state.replace(".pending_manifest", ".empty")
state_path.write_text(state, encoding="utf-8")

cache_path = ROOT / "crates/bedrock-render/src/renderer/cache.rs"
cache = cache_path.read_text(encoding="utf-8")
cache = cache.replace("TileManifestCacheReader", "CacheBinaryReader")
cache = cache.replace("tile_manifest_temp_path", "cache_temp_path")
cache = remove_named_function(cache, "test_key")
cache = remove_named_function(cache, "test_snapshot")
cache = remove_old_tests(cache)
cache_path.write_text(cache, encoding="utf-8")

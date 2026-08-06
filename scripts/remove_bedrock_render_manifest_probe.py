from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def item_end(text: str, brace: int) -> int:
    depth = 0
    in_string = False
    escaped = False
    in_char = False
    index = brace
    while index < len(text):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif in_char:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == "'":
                in_char = False
        elif char == '"':
            in_string = True
        elif char == "'":
            # Lifetimes do not have a closing quote. Treat only quoted chars as char literals.
            if index + 2 < len(text) and text[index + 2] == "'":
                in_char = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise RuntimeError("unterminated Rust item")


def item_start_with_docs(text: str, signature_start: int) -> int:
    line_start = text.rfind("\n", 0, signature_start) + 1
    start = line_start
    cursor = line_start
    while cursor > 0:
        previous_end = cursor - 1
        previous_start = text.rfind("\n", 0, previous_end) + 1
        line = text[previous_start:previous_end].strip()
        if line.startswith("///") or line.startswith("#[") or line == "":
            start = previous_start
            cursor = previous_start
            continue
        break
    return start


def remove_named_functions(text: str, names: list[str]) -> str:
    for name in names:
        while True:
            match = re.search(
                rf"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+{re.escape(name)}\s*\(",
                text,
            )
            if match is None:
                break
            brace = text.find("{", match.end())
            if brace < 0:
                raise RuntimeError(f"missing body for {name}")
            start = item_start_with_docs(text, match.start())
            end = item_end(text, brace)
            while end < len(text) and text[end] in " \t\r\n":
                end += 1
            text = text[:start] + text[end:]
    return text


def remove_tests_containing(text: str, needle: str) -> str:
    search_from = 0
    while True:
        attribute = text.find("#[test]", search_from)
        if attribute < 0:
            return text
        function = re.search(r"(?m)^\s*fn\s+\w+\s*\(", text[attribute:])
        if function is None:
            return text
        function_start = attribute + function.start()
        brace = text.find("{", attribute + function.end())
        if brace < 0:
            return text
        end = item_end(text, brace)
        block = text[attribute:end]
        if needle not in block:
            search_from = end
            continue
        start = item_start_with_docs(text, attribute)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]
        search_from = start


def remove_struct(text: str, name: str) -> str:
    pattern = re.compile(rf"(?m)^\s*pub\s+struct\s+{re.escape(name)}\b")
    while True:
        match = pattern.search(text)
        if match is None:
            return text
        brace = text.find("{", match.end())
        if brace < 0:
            raise RuntimeError(f"missing struct body for {name}")
        start = item_start_with_docs(text, match.start())
        end = item_end(text, brace)
        while end < len(text) and text[end] in " \t\r\n":
            end += 1
        text = text[:start] + text[end:]


def clean_export_file(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    text = text.replace("TileManifestProbeRequest,\n    TileManifestProbeResult, ", "")
    text = text.replace("TileManifestProbeRequest,", "")
    text = text.replace("TileManifestProbeResult,", "")
    text = re.sub(r"[ \t]+\n", "\n", text)
    path.write_text(text, encoding="utf-8")


pipeline_path = ROOT / "crates/bedrock-render/src/renderer/pipeline.rs"
pipeline = pipeline_path.read_text(encoding="utf-8")
pipeline = remove_struct(pipeline, "TileManifestProbeRequest")
pipeline = remove_struct(pipeline, "TileManifestProbeResult")
pipeline = remove_named_functions(
    pipeline,
    ["probe_tile_manifest_blocking", "probe_tile_manifest_async"],
)
pipeline = remove_tests_containing(pipeline, "TileManifestProbe")
pipeline_path.write_text(pipeline, encoding="utf-8")

clean_export_file(ROOT / "crates/bedrock-render/src/renderer.rs")
clean_export_file(ROOT / "crates/bedrock-render/src/bedrock_render.rs")

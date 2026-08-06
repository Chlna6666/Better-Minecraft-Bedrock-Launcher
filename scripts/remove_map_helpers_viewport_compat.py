from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def function_span(text: str, signature: str) -> tuple[int, int]:
    start = text.find(signature)
    if start < 0:
        raise RuntimeError(f"function not found: {signature}")
    brace = text.find("{", start)
    if brace < 0:
        raise RuntimeError(f"function body not found: {signature}")
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
            continue
        if char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                end = index + 1
                while end < len(text) and text[end] in " \t\r\n":
                    end += 1
                return start, end
    raise RuntimeError(f"unterminated function: {signature}")


def extract_function(text: str, signature: str) -> str:
    start, end = function_span(text, signature)
    return text[start:end].rstrip()


def replace_function(text: str, signature: str, replacement: str) -> str:
    start, end = function_span(text, signature)
    return text[:start] + replacement.rstrip() + "\n\n" + text[end:]


def insert_after_imports(text: str, block: str) -> str:
    lines = text.splitlines(keepends=True)
    index = 0
    while index < len(lines):
        stripped = lines[index].strip()
        if stripped.startswith("use "):
            while index < len(lines) and ";" not in lines[index]:
                index += 1
            index += 1
            continue
        if stripped == "" or stripped.startswith("//") or stripped.startswith("#!["):
            index += 1
            continue
        break
    return "".join(lines[:index]) + block.rstrip() + "\n\n" + "".join(lines[index:])


def merge_helpers() -> None:
    stable_path = ROOT / "src/ui/window/map_viewer/helpers_stable.rs"
    canonical_path = ROOT / "src/ui/window/map_viewer/helpers.rs"
    if not stable_path.exists():
        return
    stable = stable_path.read_text(encoding="utf-8")
    canonical = canonical_path.read_text(encoding="utf-8")

    cache_limit = extract_function(stable, "pub(super) fn tile_cache_memory_limit")
    memory_budget = extract_function(stable, "pub(super) fn ui_tile_memory_budget_bytes")
    memory_budget = memory_budget.replace(
        "super::helpers_legacy::available_system_memory_bytes()",
        "available_system_memory_bytes()",
    )
    visible_count = extract_function(stable, "fn visible_tile_count")

    canonical = replace_function(
        canonical,
        "pub(super) fn tile_cache_memory_limit",
        cache_limit,
    )
    canonical = replace_function(
        canonical,
        "pub(super) fn ui_tile_memory_budget_bytes",
        memory_budget,
    )

    constants = """const MIN_REGION_CACHE_ENTRIES: usize = 131_072;
const MAX_REGION_CACHE_ENTRIES: usize = 262_144;
const MIN_REGION_CACHE_BYTES: usize = 96 * 1024 * 1024;
const MAX_REGION_CACHE_BYTES: usize = 384 * 1024 * 1024;
const TARGET_RESIDENT_TILE_IMAGES: usize = 4_096;"""
    if "const MIN_REGION_CACHE_ENTRIES" not in canonical:
        canonical = insert_after_imports(canonical, constants)
    if "fn visible_tile_count" not in canonical:
        canonical = canonical.rstrip() + "\n\n" + visible_count + "\n"

    canonical_path.write_text(canonical, encoding="utf-8")
    stable_path.unlink()


def merge_viewport() -> None:
    stable_path = ROOT / "src/ui/window/map_viewer/viewport_stable.rs"
    canonical_path = ROOT / "src/ui/window/map_viewer/viewport.rs"
    if not stable_path.exists():
        return
    stable = stable_path.read_text(encoding="utf-8")
    canonical = canonical_path.read_text(encoding="utf-8")

    public_signature = "pub(super) fn paint_tile_bounds_for_viewport"
    start, _ = function_span(canonical, public_signature)
    canonical = (
        canonical[:start]
        + canonical[start:].replace(
            public_signature,
            "fn raw_paint_tile_bounds_for_viewport",
            1,
        )
    )

    wrapper = extract_function(stable, public_signature)
    wrapper = wrapper.replace(
        "super::viewport_base::paint_tile_bounds_for_viewport",
        "raw_paint_tile_bounds_for_viewport",
    )
    constants = """const CANVAS_PAINT_PAGE_TILES: i32 = 32;
const CANVAS_PAINT_GUARD_TILES: i32 = 8;"""
    canonical = insert_after_imports(canonical, constants + "\n\n" + wrapper)

    canonical_path.write_text(canonical, encoding="utf-8")
    stable_path.unlink()


def update_module_graph() -> None:
    path = ROOT / "src/ui/window/map_viewer.rs"
    text = path.read_text(encoding="utf-8")
    helper_alias = """#[path = \"map_viewer/helpers_stable.rs\"]
mod helpers;
#[path = \"map_viewer/helpers.rs\"]
mod helpers_legacy;"""
    if helper_alias in text:
        text = text.replace(helper_alias, "mod helpers;", 1)
    viewport_alias = """#[path = \"map_viewer/viewport_stable.rs\"]
mod viewport;
#[path = \"map_viewer/viewport.rs\"]
mod viewport_base;"""
    if viewport_alias in text:
        text = text.replace(viewport_alias, "mod viewport;", 1)
    path.write_text(text, encoding="utf-8")


def assert_removed() -> None:
    forbidden = (
        "helpers_stable",
        "helpers_legacy",
        "viewport_stable",
        "viewport_base",
    )
    targets = [
        ROOT / "src/ui/window/map_viewer.rs",
        *list((ROOT / "src/ui/window/map_viewer").glob("*.rs")),
    ]
    offenders: list[str] = []
    for path in targets:
        text = path.read_text(encoding="utf-8")
        for symbol in forbidden:
            if symbol in text:
                offenders.append(f"{path.relative_to(ROOT)}: {symbol}")
    if offenders:
        raise RuntimeError("map compatibility layer remains:\n" + "\n".join(offenders))


def main() -> None:
    merge_helpers()
    merge_viewport()
    update_module_graph()
    assert_removed()


if __name__ == "__main__":
    main()

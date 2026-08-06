from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def replace_exact(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


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


def remove_function(text: str, signature: str) -> str:
    start, end = function_span(text, signature)
    return text[:start] + text[end:]


def rename_partitioned_scan_api() -> None:
    replacements = (
        ("for_each_key_partitioned_locked", "scan_keys_partitioned_locked"),
        ("for_each_entry_partitioned_locked", "scan_entries_partitioned_locked"),
        ("for_each_key_partitioned", "scan_keys_partitioned"),
        ("for_each_entry_partitioned", "scan_entries_partitioned"),
    )
    roots = (
        ROOT / "crates/bedrock-leveldb",
        ROOT / "crates/bedrock-world",
        ROOT / "crates/bedrock-render",
        ROOT / "src",
    )
    suffixes = {".rs", ".md", ".toml"}
    for base in roots:
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in suffixes:
                continue
            text = path.read_text(encoding="utf-8")
            updated = text
            for old, new in replacements:
                updated = updated.replace(old, new)
            if updated != text:
                path.write_text(updated, encoding="utf-8")


def merge_tile_plan() -> None:
    active_path = "src/ui/window/map_viewer/tile_plan_stable.rs"
    canonical_path = "src/ui/window/map_viewer/tile_plan.rs"
    active = read(active_path)
    canonical = read(canonical_path)

    build = extract_function(active, "pub(super) fn build_viewport_tile_plan")
    build = build.replace(
        "super::tile_plan_legacy::retained_tile_filter_for_visible_bounds",
        "retained_tile_filter_for_visible_bounds",
    ).replace(
        "super::tile_plan_legacy::tile_coords_for_bounds",
        "tile_coords_for_bounds",
    )
    canonical = replace_function(
        canonical,
        "pub(super) fn build_viewport_tile_plan",
        build,
    )

    visible = """pub(super) fn tile_coords_for_visible_bounds(
    visible: TileBounds,
    center: (i32, i32),
) -> Vec<(i32, i32)> {
    center_first_visible_tile_coords(visible, center)
}"""
    canonical = replace_function(
        canonical,
        "pub(super) fn tile_coords_for_visible_bounds",
        visible,
    )
    canonical = remove_function(canonical, "fn projected_drag_prefetch_tiles")

    center_first = extract_function(active, "fn center_first_visible_tile_coords")
    tests_start = active.find("#[cfg(test)]")
    if tests_start < 0:
        raise RuntimeError("tile plan tests not found")
    tests = active[tests_start:].strip()
    if "fn center_first_visible_tile_coords" not in canonical:
        canonical = canonical.rstrip() + "\n\n" + center_first + "\n\n" + tests + "\n"
    write(canonical_path, canonical)
    (ROOT / active_path).unlink()


def merge_lifecycle_wrapper() -> None:
    canonical_path = "src/ui/window/map_viewer/lifecycle.rs"
    wrapper_path = "src/ui/window/map_viewer/lifecycle_stable.rs"
    canonical = read(canonical_path)
    prefix = """const VIEWPORT_INTERACTION_IDLE_DELAY: std::time::Duration = std::time::Duration::ZERO;
const VIEWPORT_TILE_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
const INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT: usize = usize::MAX;

fn paint_tile_bounds_for_viewport(
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    radius: i32,
) -> Option<super::viewport::TileBounds> {
    super::viewport::paint_tile_bounds_for_viewport(viewport, layout, radius)
}

fn screen_image_bounds(
    _bounds: gpui::Bounds<gpui::Pixels>,
    _viewport: super::model::MapViewport,
    _image: &super::canvas::ScreenPaintImage,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    None
}

"""
    if "const VIEWPORT_INTERACTION_IDLE_DELAY" not in canonical:
        canonical = prefix + canonical
    write(canonical_path, canonical)
    (ROOT / wrapper_path).unlink()


def simplify_map_modules() -> None:
    path = "src/ui/window/map_viewer.rs"
    text = read(path)
    text = replace_exact(
        text,
        """#[path = \"map_viewer/canvas_frontend_stable.rs\"]
mod canvas;
#[path = \"map_viewer/canvas_stable.rs\"]
mod canvas_base;
#[path = \"map_viewer/canvas.rs\"]
mod canvas_legacy;""",
        """#[path = \"map_viewer/canvas_stable.rs\"]
mod canvas;
#[path = \"map_viewer/canvas.rs\"]
mod canvas_legacy;""",
        "canvas frontend alias chain",
    )
    text = replace_exact(
        text,
        """#[path = \"map_viewer/lifecycle_stable.rs\"]
mod lifecycle;""",
        "mod lifecycle;",
        "lifecycle wrapper module",
    )
    text = replace_exact(
        text,
        """#[path = \"map_viewer/tile_plan_stable.rs\"]
mod tile_plan;
#[path = \"map_viewer/tile_plan.rs\"]
mod tile_plan_legacy;""",
        "mod tile_plan;",
        "tile plan wrapper modules",
    )
    write(path, text)
    (ROOT / "src/ui/window/map_viewer/canvas_frontend_stable.rs").unlink()


def delete_migration_artifacts() -> None:
    patterns = (
        "apply_bmcbl_occupancy_state*.py",
        "apply_local_bedrock_*",
        "fix_bmcbl_occupancy_compile.py",
        "remove_bedrock_render_manifest_probe.py",
        "second_phase_inspection.txt",
        "second_phase_validation.txt",
    )
    scripts = ROOT / "scripts"
    for pattern in patterns:
        for path in scripts.glob(pattern):
            if path.name == Path(__file__).name:
                continue
            if path.is_file():
                path.unlink()


def assert_removed() -> None:
    forbidden = (
        "for_each_key_partitioned",
        "for_each_entry_partitioned",
        "tile_plan_legacy",
        "tile_plan_stable",
        "lifecycle_stable",
        "canvas_frontend_stable",
    )
    roots = (
        ROOT / "crates/bedrock-leveldb",
        ROOT / "crates/bedrock-world",
        ROOT / "crates/bedrock-render",
        ROOT / "src/ui/window/map_viewer.rs",
        ROOT / "src/ui/window/map_viewer",
    )
    offenders: list[str] = []
    for root in roots:
        paths = [root] if root.is_file() else list(root.rglob("*.rs"))
        for path in paths:
            text = path.read_text(encoding="utf-8")
            for symbol in forbidden:
                if symbol in text:
                    offenders.append(f"{path.relative_to(ROOT)}: {symbol}")
    if offenders:
        raise RuntimeError("legacy API remains:\n" + "\n".join(offenders))


def main() -> None:
    rename_partitioned_scan_api()
    merge_tile_plan()
    merge_lifecycle_wrapper()
    simplify_map_modules()
    delete_migration_artifacts()
    assert_removed()


if __name__ == "__main__":
    main()

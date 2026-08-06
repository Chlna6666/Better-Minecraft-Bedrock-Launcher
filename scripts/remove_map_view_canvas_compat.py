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


def append_function_if_missing(text: str, signature: str, function: str) -> str:
    if signature in text:
        return replace_function(text, signature, function)
    return text.rstrip() + "\n\n" + function.rstrip() + "\n"


def merge_view() -> None:
    stable_path = ROOT / "src/ui/window/map_viewer/view_stable.rs"
    canonical_path = ROOT / "src/ui/window/map_viewer/view.rs"
    if not stable_path.exists():
        return

    stable = stable_path.read_text(encoding="utf-8")
    canonical = canonical_path.read_text(encoding="utf-8")

    import_anchor = "use super::region_package;\n"
    imports = """use crate::ui::window::map_viewer::lifecycle::VIEWPORT_COMPOSITE_ENABLED;
use super::tile_state::TileLoadState;
use super::viewport::TileBounds;
use std::time::Duration;
"""
    if "VIEWPORT_COMPOSITE_ENABLED" not in canonical:
        if import_anchor not in canonical:
            raise RuntimeError("view import anchor not found")
        canonical = canonical.replace(import_anchor, import_anchor + imports, 1)

    constants_anchor = "const MAP_VIEWER_MAX_DISPLAY_RATIO: f32 = 0.9;\n"
    watchdog_constants = """const VIEWPORT_WATCHDOG_INTERVAL: Duration = Duration::from_millis(80);
const FRONTEND_TILE_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
const FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT: usize = 8;
const FRONTEND_REPAINT_SAFETY_PASSES: usize = 2;
const FRONTEND_REPAINT_PROGRESS_LOG_INTERVAL: usize = 8;
"""
    if "const VIEWPORT_WATCHDOG_INTERVAL" not in canonical:
        if constants_anchor not in canonical:
            raise RuntimeError("view constants anchor not found")
        canonical = canonical.replace(
            constants_anchor,
            constants_anchor + watchdog_constants,
            1,
        )

    helpers = []
    for signature in (
        "fn frontend_repaint_passes",
        "fn frontend_snapshot_image_ids",
        "fn visible_tile_frontend_ready",
    ):
        helper = extract_function(stable, signature)
        if signature == "fn visible_tile_frontend_ready":
            helper = helper.replace(
                "entry.state == TileLoadState::Invalid\n            || (entry.state == TileLoadState::Loaded && entry.image.is_some())",
                "matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)\n            || (entry.state == TileLoadState::Loaded && entry.image.is_some())",
            )
        helpers.append(helper)
    helper_block = "\n\n".join(helpers)
    if "fn frontend_repaint_passes" not in canonical:
        drop_impl = canonical.find("impl Drop for MapViewerWindowView")
        if drop_impl < 0:
            raise RuntimeError("view Drop impl anchor not found")
        canonical = canonical[:drop_impl] + helper_block + "\n\n" + canonical[drop_impl:]

    watchdog = extract_function(stable, "fn spawn_viewport_watchdog")
    if "fn spawn_viewport_watchdog" not in canonical:
        canonical = canonical.rstrip() + "\n\nimpl MapViewerWindowView {\n" + watchdog + "\n}\n"

    view_creation = "        let view = cx.new(|cx| MapViewerWindowView::new(init, window, cx));\n"
    spawn_call = "        view.update(cx, |this, cx| this.spawn_viewport_watchdog(cx));\n"
    if spawn_call not in canonical:
        if view_creation not in canonical:
            raise RuntimeError("map viewer creation anchor not found")
        canonical = canonical.replace(view_creation, view_creation + spawn_call, 1)

    canonical_path.write_text(canonical, encoding="utf-8")
    stable_path.unlink()


def empty_debug_label() -> str:
    return """label: match entry.state {
                    super::tile_state::TileLoadState::Empty => SharedString::from("空"),
                    super::tile_state::TileLoadState::Invalid => SharedString::from("无效"),
                    _ => SharedString::from("失败"),
                },"""


def merge_canvas() -> None:
    stable_path = ROOT / "src/ui/window/map_viewer/canvas_stable.rs"
    canonical_path = ROOT / "src/ui/window/map_viewer/canvas.rs"
    if not stable_path.exists():
        return

    stable = stable_path.read_text(encoding="utf-8")
    canonical = canonical_path.read_text(encoding="utf-8")

    canonical = canonical.replace(
        "use std::sync::atomic::{AtomicBool, Ordering};",
        "use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};",
        1,
    )
    static_anchor = "static MAP_TILE_PAINT_RESOURCES_UNAVAILABLE: AtomicBool = AtomicBool::new(false);\n"
    additional_state = """const SUSTAINED_PAINT_RESOURCE_FAILURE_LIMIT: usize = 240;
static PAINT_RESOURCE_FAILURE_STREAK: AtomicUsize = AtomicUsize::new(0);
"""
    if "PAINT_RESOURCE_FAILURE_STREAK" not in canonical:
        if static_anchor not in canonical:
            raise RuntimeError("canvas resource state anchor not found")
        canonical = canonical.replace(static_anchor, static_anchor + additional_state, 1)

    take = extract_function(stable, "pub(super) fn take_map_tile_paint_resources_unavailable")
    take = take.replace(
        "super::canvas_legacy::take_map_tile_paint_resources_unavailable()",
        "MAP_TILE_PAINT_RESOURCES_UNAVAILABLE.swap(false, Ordering::Relaxed)",
    )
    canonical = replace_function(
        canonical,
        "pub(super) fn take_map_tile_paint_resources_unavailable",
        take,
    )

    state_match_old = """super::tile_state::TileLoadState::Failed
                    | super::tile_state::TileLoadState::Invalid"""
    state_match_new = """super::tile_state::TileLoadState::Failed
                    | super::tile_state::TileLoadState::Empty
                    | super::tile_state::TileLoadState::Invalid"""
    old_label = """label: if entry.state == super::tile_state::TileLoadState::Invalid {
                    SharedString::from("空")
                } else {
                    SharedString::from("失败")
                },"""

    for signature in (
        "pub(super) fn build_tile_paint_snapshot",
        "pub(super) fn patch_tile_paint_snapshot",
        "fn patch_tile",
        "fn patch_overlay",
    ):
        function = extract_function(stable, signature)
        function = function.replace(state_match_old, state_match_new)
        function = function.replace(old_label, empty_debug_label())
        canonical = append_function_if_missing(canonical, signature, function)

    canonical_path.write_text(canonical, encoding="utf-8")
    stable_path.unlink()


def update_module_graph() -> None:
    path = ROOT / "src/ui/window/map_viewer.rs"
    text = path.read_text(encoding="utf-8")
    canvas_alias = """#[path = \"map_viewer/canvas_stable.rs\"]
mod canvas;
#[path = \"map_viewer/canvas.rs\"]
mod canvas_legacy;"""
    if canvas_alias in text:
        text = text.replace(canvas_alias, "mod canvas;", 1)
    view_alias = """#[path = \"map_viewer/view_stable.rs\"]
mod view;
#[path = \"map_viewer/view.rs\"]
mod view_legacy;"""
    if view_alias in text:
        text = text.replace(view_alias, "mod view;", 1)
    path.write_text(text, encoding="utf-8")


def assert_removed() -> None:
    forbidden = (
        "canvas_stable",
        "canvas_legacy",
        "view_stable",
        "view_legacy",
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
        raise RuntimeError("view/canvas compatibility layer remains:\n" + "\n".join(offenders))


def main() -> None:
    merge_view()
    merge_canvas()
    update_module_graph()
    assert_removed()


if __name__ == "__main__":
    main()

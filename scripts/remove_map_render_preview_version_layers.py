from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAP = ROOT / "src/ui/window/map_viewer"
MODULE_ROOT = ROOT / "src/ui/window/map_viewer.rs"


def rename_tile_render_modules() -> None:
    old_core = MAP / "tile_render_legacy.rs"
    old_composite = MAP / "tile_render_stable.rs"
    old_facade = MAP / "tile_render_current.rs"
    if not old_facade.exists():
        return
    if not old_core.exists() or not old_composite.exists():
        raise RuntimeError("incomplete tile-render versioned module set")

    core = old_core.read_text(encoding="utf-8")
    composite = old_composite.read_text(encoding="utf-8")
    facade = old_facade.read_text(encoding="utf-8")

    composite = composite.replace(
        "pub(super) use super::tile_render_legacy::*;",
        "pub(super) use super::tile_render_core::*;",
        1,
    )
    composite = composite.replace(
        "use super::tile_render_legacy as legacy;",
        "use super::tile_render_core as core;",
        1,
    )
    composite = composite.replace("legacy::", "core::")

    facade = facade.replace(
        "pub(super) use super::tile_render_stable::*;",
        "pub(super) use super::tile_render_composite::*;",
        1,
    )
    facade = facade.replace(
        "use super::tile_render_legacy as legacy;",
        "use super::tile_render_core as core;",
        1,
    )
    facade = facade.replace(
        "use super::tile_render_stable as stable;",
        "use super::tile_render_composite as composite;",
        1,
    )
    facade = facade.replace("legacy::", "core::")
    facade = facade.replace("stable::", "composite::")

    (MAP / "tile_render_core.rs").write_text(core, encoding="utf-8")
    (MAP / "tile_render_composite.rs").write_text(composite, encoding="utf-8")
    (MAP / "tile_render.rs").write_text(facade, encoding="utf-8")
    old_core.unlink()
    old_composite.unlink()
    old_facade.unlink()


def rename_preview_modules() -> None:
    old_source = MAP / "preview_3d.rs"
    old_facade = MAP / "preview_3d_region.rs"
    if not old_facade.exists():
        return
    if not old_source.exists():
        raise RuntimeError("preview 3D source module missing")

    source = old_source.read_text(encoding="utf-8")
    facade = old_facade.read_text(encoding="utf-8")
    facade = facade.replace("preview_3d_legacy", "preview_3d_source")
    facade = facade.replace("LegacyMeshSignature", "SourceMeshSignature")
    facade = facade.replace("legacy_mesh_signature", "source_mesh_signature")
    facade = facade.replace("convert_legacy_mesh", "convert_source_mesh")
    facade = re.sub(r"\blegacy\b", "source_mesh", facade)

    (MAP / "preview_3d_source.rs").write_text(source, encoding="utf-8")
    old_source.write_text(facade, encoding="utf-8")
    old_facade.unlink()


def update_references() -> None:
    replacements = (
        ("tile_render_legacy", "tile_render_core"),
        ("tile_render_stable", "tile_render_composite"),
        ("preview_3d_legacy", "preview_3d_source"),
    )
    for path in MAP.glob("*.rs"):
        text = path.read_text(encoding="utf-8")
        updated = text
        for old, new in replacements:
            updated = updated.replace(old, new)
        if updated != text:
            path.write_text(updated, encoding="utf-8")


def update_module_graph() -> None:
    text = MODULE_ROOT.read_text(encoding="utf-8")
    preview_aliases = """#[path = \"map_viewer/preview_3d_region.rs\"]
mod preview_3d;
#[path = \"map_viewer/preview_3d.rs\"]
mod preview_3d_legacy;"""
    preview_modules = """mod preview_3d;
mod preview_3d_source;"""
    if preview_aliases in text:
        text = text.replace(preview_aliases, preview_modules, 1)

    render_aliases = """#[path = \"map_viewer/tile_render_current.rs\"]
mod tile_render;
mod tile_render_legacy;
#[path = \"map_viewer/tile_render_stable.rs\"]
mod tile_render_stable;"""
    render_modules = """mod tile_render;
mod tile_render_composite;
mod tile_render_core;"""
    if render_aliases in text:
        text = text.replace(render_aliases, render_modules, 1)

    MODULE_ROOT.write_text(text, encoding="utf-8")


def assert_removed() -> None:
    forbidden = (
        "tile_render_current",
        "tile_render_stable",
        "tile_render_legacy",
        "preview_3d_region",
        "preview_3d_legacy",
        "LegacyMeshSignature",
        "legacy_mesh_signature",
        "convert_legacy_mesh",
    )
    targets = [MODULE_ROOT, *list(MAP.glob("*.rs"))]
    offenders: list[str] = []
    for path in targets:
        text = path.read_text(encoding="utf-8")
        for symbol in forbidden:
            if symbol in text:
                offenders.append(f"{path.relative_to(ROOT)}: {symbol}")
    old_files = (
        MAP / "tile_render_current.rs",
        MAP / "tile_render_stable.rs",
        MAP / "tile_render_legacy.rs",
        MAP / "preview_3d_region.rs",
    )
    offenders.extend(
        f"{path.relative_to(ROOT)}: old file remains" for path in old_files if path.exists()
    )
    required = (
        MAP / "tile_render.rs",
        MAP / "tile_render_core.rs",
        MAP / "tile_render_composite.rs",
        MAP / "preview_3d.rs",
        MAP / "preview_3d_source.rs",
    )
    offenders.extend(
        f"{path.relative_to(ROOT)}: required file missing" for path in required if not path.exists()
    )
    if offenders:
        raise RuntimeError("render/preview version layer remains:\n" + "\n".join(offenders))


def main() -> None:
    rename_tile_render_modules()
    rename_preview_modules()
    update_references()
    update_module_graph()
    assert_removed()


if __name__ == "__main__":
    main()

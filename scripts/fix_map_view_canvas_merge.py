from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CANVAS = ROOT / "src/ui/window/map_viewer/canvas.rs"

PATCH_TILE = r'''fn patch_tile(
    tiles: &mut Vec<super::tile_state::PaintTile>,
    tile_manager: &super::tile_state::RegionManager,
    paint_bounds: Option<super::viewport::TileBounds>,
    coord: (i32, i32),
) -> bool {
    let key = super::viewport::tile_paint_sort_key(coord);
    let existing = tiles.binary_search_by_key(&key, |tile| {
        super::viewport::tile_paint_sort_key(tile.coord)
    });
    let replacement = paint_bounds
        .filter(|bounds| bounds.contains(coord))
        .and_then(|_| tile_manager.entries.get(&coord))
        .and_then(|entry| entry.image.as_ref())
        .map(|tile| super::tile_state::PaintTile {
            coord,
            image: tile.image.clone(),
            pixel_format: tile.pixel_format,
            width: tile.width,
            height: tile.height,
            estimated_bytes: tile.estimated_bytes,
        });

    match (existing, replacement) {
        (Ok(index), Some(replacement)) => {
            let current = &tiles[index];
            if Arc::ptr_eq(&current.image, &replacement.image)
                && current.pixel_format == replacement.pixel_format
                && current.width == replacement.width
                && current.height == replacement.height
                && current.estimated_bytes == replacement.estimated_bytes
            {
                return false;
            }
            tiles[index] = replacement;
            true
        }
        (Ok(index), None) => {
            tiles.remove(index);
            true
        }
        (Err(index), Some(replacement)) => {
            tiles.insert(index, replacement);
            true
        }
        (Err(_), None) => false,
    }
}
'''


def main() -> None:
    text = CANVAS.read_text(encoding="utf-8")

    old_helpers_start = text.find("\n#[derive(Clone, Copy)]\nstruct PaintTilePatchChange")
    patch_overlay_start = text.find("\nfn patch_overlay(")
    if old_helpers_start >= 0:
        if patch_overlay_start < old_helpers_start:
            raise RuntimeError("patch_overlay anchor precedes old helper block")
        text = text[:old_helpers_start] + "\n\n" + text[patch_overlay_start + 1 :]

    if "\nfn patch_tile(\n" not in text:
        patch_overlay_start = text.find("\nfn patch_overlay(")
        if patch_overlay_start < 0:
            raise RuntimeError("patch_overlay anchor not found")
        text = text[:patch_overlay_start] + "\n\n" + PATCH_TILE + text[patch_overlay_start:]

    old_match = """super::tile_state::TileLoadState::Failed
                        | super::tile_state::TileLoadState::Invalid"""
    new_match = """super::tile_state::TileLoadState::Failed
                        | super::tile_state::TileLoadState::Empty
                        | super::tile_state::TileLoadState::Invalid"""
    text = text.replace(old_match, new_match)

    forbidden = (
        "PaintTilePatchChange",
        "fn patch_paint_tile(",
        "fn patch_debug_overlay(",
        "fn paint_tile_for_coord(",
        "fn debug_overlay_for_coord(",
        "fn insert_or_replace_paint_tile(",
        "fn remove_paint_tile(",
        "fn insert_or_replace_debug_overlay(",
        "fn remove_debug_overlay(",
        "fn paint_bounds_contains(",
        "fn paint_tile_same(",
        "fn debug_overlay_same(",
        "fn paint_tiles_are_ordered(",
        "fn debug_overlays_are_ordered(",
    )
    remaining = [symbol for symbol in forbidden if symbol in text]
    if remaining:
        raise RuntimeError("obsolete canvas patch helpers remain: " + ", ".join(remaining))
    if text.count("\nfn patch_tile(\n") != 1:
        raise RuntimeError("expected exactly one canonical patch_tile function")

    CANVAS.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()

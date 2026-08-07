from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, got {count}")
    return text.replace(old, new, 1)


path = ROOT / "src/core/minecraft/map_info_cache.rs"
text = path.read_text(encoding="utf-8")
start = text.index("pub fn load_cached_map_info_tiles_blocking(")
end = text.index("/// Removes the cached tiles", start)
section = text[start:end]
section = replace_once(
    section,
    "    let index = cache.load_index()?;\n    let mut payloads = BTreeMap::new();",
    "    let index = cache.load_index()?;\n    let requested_tile_count = keys.len();\n    let mut payloads = BTreeMap::new();",
    "cached requested count seed",
)
section = replace_once(
    section,
    "        keys.len(),\n    ))",
    "        requested_tile_count,\n    ))",
    "cached requested count use",
)
text = text[:start] + section + text[end:]
path.write_text(text, encoding="utf-8")

path = ROOT / "src/ui/window/map_viewer/overlays.rs"
text = path.read_text(encoding="utf-8")
count = text.count("if let Some(world_bounds) = indexed_bounds {")
if count != 2:
    raise RuntimeError(f"indexed bounds ownership: expected 2 matches, got {count}")
text = text.replace(
    "if let Some(world_bounds) = indexed_bounds {",
    "if let Some(world_bounds) = indexed_bounds.as_ref() {",
)
path.write_text(text, encoding="utf-8")

print("map entity query/cache v2 follow-up applied")

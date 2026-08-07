from pathlib import Path

path = Path("src/ui/window/map_viewer/map_history.rs")
text = path.read_text(encoding="utf-8")

private_call = "bedrock_world::surface::is_air_block_name(&state.name)"
if text.count(private_call) != 1:
    raise SystemExit(
        f"expected one private air helper call, found {text.count(private_call)}"
    )
text = text.replace(private_call, "history_block_name_is_air(&state.name)", 1)

bytes_call = "Bytes::copy_from_slice(bytes)"
bytes_count = text.count(bytes_call)
if bytes_count != 2:
    raise SystemExit(f"expected two Bytes calls, found {bytes_count}")
text = text.replace(bytes_call, "bytes::Bytes::copy_from_slice(bytes)")

marker = "fn history_subchunk_block_is_air(\n"
if text.count(marker) != 1:
    raise SystemExit(
        f"expected one history_subchunk_block_is_air marker, found {text.count(marker)}"
    )
helper = '''fn history_block_name_is_air(name: &str) -> bool {
    matches!(
        name,
        "air"
            | "cave_air"
            | "void_air"
            | "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:structure_void"
            | "minecraft:light_block"
            | "minecraft:light"
    )
}

'''
text = text.replace(marker, helper + marker, 1)

path.write_text(text, encoding="utf-8")

from pathlib import Path

path = Path("src/ui/window/map_viewer/preview_3d_source.rs")
source = path.read_text(encoding="utf-8")

old_line = "    let neighbor_model = preview_3d_neighbor_model(state);\n"
if source.count(old_line) != 1:
    raise RuntimeError(f"neighbor model line: expected 1 match, got {source.count(old_line)}")
source = source.replace(old_line, "", 1)

anchor = '''    let resolved_shape = block_models
        .and_then(|models| preview_3d_resolved_detail_shape_for_block(models, state, block_class));
'''
replacement = anchor + '''    let neighbor_model = if resolved_shape.is_none() {
        preview_3d_neighbor_model(state)
    } else {
        Preview3dNeighborModel::None
    };
'''
if source.count(anchor) != 1:
    raise RuntimeError(f"resolved shape anchor: expected 1 match, got {source.count(anchor)}")
source = source.replace(anchor, replacement, 1)
path.write_text(source, encoding="utf-8")

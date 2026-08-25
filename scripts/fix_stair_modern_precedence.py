from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
path = ROOT / "crates/bedrock-block-model/src/java.rs"
text = path.read_text(encoding="utf-8")

old = '''        ModelFamily::Stairs => {
            if let Some(direction) = state_i64(state, "weirdo_direction")
                .and_then(cardinal_direction_0_3)
                .or_else(|| bedrock_cardinal_direction(state))
            {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
            let top = state_top_half(state).unwrap_or(false);
'''
new = '''        ModelFamily::Stairs => {
            if let Some(direction) = bedrock_cardinal_direction(state) {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
            let top = state_top_half(state).unwrap_or(false);
'''
if text.count(old) != 1:
    raise RuntimeError(f"stairs precedence block: expected 1 match, got {text.count(old)}")
text = text.replace(old, new, 1)

old_import = "use crate::model_family::shape::{detail_cuboid_with_local_uv, ModelCuboid, ModelPlane, ModelShape};"
new_import = "use crate::model_family::shape::{detail_cuboid_with_local_uv, ModelCuboid, ModelShape};"
if text.count(old_import) != 1:
    raise RuntimeError(f"java shape import: expected 1 match, got {text.count(old_import)}")
text = text.replace(old_import, new_import, 1)

path.write_text(text, encoding="utf-8")
print("modern stair direction precedence fixed")

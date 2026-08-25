from pathlib import Path

path = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/preview_3d_source.rs"
text = path.read_text(encoding="utf-8")
old = '''                let straight = connected.len() == 2
                    && connected[0].opposite() == connected[1]
                    && self.block_class_at(block.above()) != Some(Preview3dBlockClass::Opaque);
'''
new = '''                let straight = connected.len() == 2
                    && connected[0].opposite() == connected[1]
                    && self.block_class_at(block.key.above()) != Some(Preview3dBlockClass::Opaque);
'''
count = text.count(old)
if count != 1:
    raise RuntimeError(f"preview wall-up compile fix: expected 1 contextual match, got {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")

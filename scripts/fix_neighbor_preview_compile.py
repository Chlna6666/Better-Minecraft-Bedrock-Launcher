from pathlib import Path

path = Path(__file__).resolve().parents[1] / "src/ui/window/map_viewer/preview_3d_source.rs"
text = path.read_text(encoding="utf-8")
old = "self.block_class_at(block.above())"
new = "self.block_class_at(block.key.above())"
count = text.count(old)
if count != 1:
    raise RuntimeError(f"preview wall-up compile fix: expected 1 match, got {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")

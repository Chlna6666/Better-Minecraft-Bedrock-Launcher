from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
for relative in [
    "src/ui/window/map_viewer/lifecycle_stable.rs",
    "src/ui/window/map_viewer/model.rs",
]:
    path = root / relative
    text = path.read_text(encoding="utf-8")
    text = re.sub(r"(?m)^\s*const\s+0\s*:\s*usize\s*=\s*\d+\s*;\n", "", text)
    path.write_text(text, encoding="utf-8")

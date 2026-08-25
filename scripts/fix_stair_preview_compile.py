from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PREVIEW = ROOT / "src/ui/window/map_viewer/preview_3d_source.rs"


def ensure_detail_shape_initializers(text: str) -> tuple[str, int]:
    """Ensure every explicit Preview3dDetailShape literal initializes front_only_planes."""
    pattern = re.compile(r"(?<!struct )Preview3dDetailShape\s*\{")
    insertions: list[tuple[int, str, str]] = []

    for match in pattern.finditer(text):
        open_brace = text.find("{", match.start(), match.end())
        depth = 0
        close_brace = -1
        for index in range(open_brace, len(text)):
            char = text[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    close_brace = index
                    break
        if close_brace < 0:
            raise RuntimeError("unterminated Preview3dDetailShape initializer")

        body = text[open_brace + 1 : close_brace]
        if "front_only_planes" in body or ".." in body:
            continue

        line_start = text.rfind("\n", 0, close_brace) + 1
        closing_indent = text[line_start:close_brace]
        field_indent = closing_indent + "    "
        insertions.append((close_brace, closing_indent, field_indent))

    for close_brace, closing_indent, field_indent in reversed(insertions):
        text = (
            text[:close_brace]
            + f"{field_indent}front_only_planes: Vec::new(),\n{closing_indent}"
            + text[close_brace:]
        )

    return text, len(insertions)


def main() -> None:
    text = PREVIEW.read_text(encoding="utf-8")
    if "front_only_planes: Vec<Preview3dPlane>" not in text:
        raise RuntimeError("front_only_planes field was not installed by the primary patch")

    text, inserted = ensure_detail_shape_initializers(text)
    PREVIEW.write_text(text, encoding="utf-8")
    print(f"preview compile fix applied: initialized {inserted} missed detail-shape literal(s)")


if __name__ == "__main__":
    main()

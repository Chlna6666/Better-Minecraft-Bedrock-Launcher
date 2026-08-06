from pathlib import Path

root = Path(__file__).resolve().parents[1]
path = root / "src/ui/window/map_viewer/panels.rs"
text = path.read_text(encoding="utf-8")
needle = ".children(\n                    .recent_events"
start = text.find(needle)
if start < 0:
    raise RuntimeError("orphan diagnostics children chain not found")
paren = text.find("(", start)
depth = 0
in_string = False
escaped = False
end = None
for index in range(paren, len(text)):
    char = text[index]
    if in_string:
        if escaped:
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == '"':
            in_string = False
    elif char == '"':
        in_string = True
    elif char == "(":
        depth += 1
    elif char == ")":
        depth -= 1
        if depth == 0:
            end = index + 1
            break
if end is None:
    raise RuntimeError("unterminated orphan diagnostics children chain")
path.write_text(text[:start] + text[end:], encoding="utf-8")

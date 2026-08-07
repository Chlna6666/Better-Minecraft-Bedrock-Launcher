from pathlib import Path

script_path = Path(__file__).with_name("apply-player-item-context-v4.py")
source = script_path.read_text(encoding="utf-8")
needle = "        if count != 1:\n            raise SystemExit(f\"{path}: expected one anchor, got {count}: {old[:140]!r}\")"
replacement = "        if count < 1:\n            raise SystemExit(f\"{path}: missing anchor: {old[:140]!r}\")"
if source.count(needle) != 1:
    raise SystemExit("unexpected apply script edit() implementation")
source = source.replace(needle, replacement, 1)
exec(compile(source, str(script_path), "exec"), {"__name__": "__main__", "__file__": str(script_path)})

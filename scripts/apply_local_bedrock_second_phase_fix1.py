from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]

table_path = root / "crates/bedrock-leveldb/src/table.rs"
table = table_path.read_text(encoding="utf-8")
old = "NativeBlockCache::new(1024)"
new = "NativeBlockCache::new(1024, 1024, 8, 4)"
count = table.count(old)
if count != 1:
    raise RuntimeError(f"expected one legacy cache constructor, found {count}")
table_path.write_text(table.replace(old, new, 1), encoding="utf-8")

db_path = root / "crates/bedrock-leveldb/src/db.rs"
db = db_path.read_text(encoding="utf-8")
db, count = re.subn(
    r"\nfn approximate_overlay_size\(values: &BTreeMap<Vec<u8>, Option<Bytes>>\) -> usize \{.*?\n\}\n",
    "\n",
    db,
    count=1,
    flags=re.S,
)
if count != 1:
    raise RuntimeError(f"expected one obsolete approximate_overlay_size function, found {count}")
db_path.write_text(db, encoding="utf-8")

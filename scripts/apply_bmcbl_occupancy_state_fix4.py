from pathlib import Path

root = Path(__file__).resolve().parents[1]
path = root / "crates/bedrock-render/src/renderer/cache.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;",
    "const RENDER_CACHE_VALIDATION_VERSION: u32 = 1;\nconst FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;",
    1,
)
text = text.replace(
    "static TILE_AUTHORITY_CACHE_WRITE_ID: AtomicUsize = AtomicUsize::new(0);",
    "static TILE_AUTHORITY_CACHE_WRITE_ID: AtomicUsize = AtomicUsize::new(0);\nstatic CACHE_ATOMIC_WRITE_ID: AtomicUsize = AtomicUsize::new(0);",
    1,
)
text = text.replace("TILE_MANIFEST_CACHE_VERSION", "RENDER_CACHE_VALIDATION_VERSION")
text = text.replace("TILE_MANIFEST_CACHE_WRITE_ID", "CACHE_ATOMIC_WRITE_ID")
text = text.replace("use bedrock_world::{ChunkBounds, ChunkPos, Dimension};", "use bedrock_world::{ChunkPos, Dimension};")
text = text.replace("use std::collections::{BTreeMap, BTreeSet};", "use std::collections::BTreeSet;")
path.write_text(text, encoding="utf-8")

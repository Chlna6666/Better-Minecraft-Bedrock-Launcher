#[allow(clippy::missing_errors_doc)]
use super::pipeline::{
    DEFAULT_PALETTE_VERSION, RENDERER_CACHE_VERSION, RenderBackend, RenderGpuBackend, RenderLayout,
    RenderMode,
};
use crate::error::{BedrockRenderError, Result};
use bedrock_world::{ChunkPos, Dimension};
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use xxhash_rust::xxh3::xxh3_128;

const RENDER_CACHE_VALIDATION_VERSION: u32 = 1;
const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;
const TILE_AUTHORITY_CACHE_VERSION: u32 = 2;
const TILE_AUTHORITY_HEADER_MAGIC: &[u8; 8] = b"BRTCHD01";
const TILE_AUTHORITY_CHUNKS_MAGIC: &[u8; 8] = b"BRTCHK01";
const TILE_AUTHORITY_TILES_MAGIC: &[u8; 8] = b"BRTTIL01";
const TILE_AUTHORITY_DEPS_MAGIC: &[u8; 8] = b"BRTDEP01";
const TILE_AUTHORITY_REFS_MAGIC: &[u8; 8] = b"BRTREF01";
const TILE_AUTHORITY_FREE_EXTENTS_MAGIC: &[u8; 8] = b"BRTFRE01";
const TILE_AUTHORITY_BLOB_MAGIC: &[u8; 8] = b"BRTBLB01";
const TILE_AUTHORITY_WAL_MAGIC: &[u8; 8] = b"BRTWAL01";
const TILE_AUTHORITY_INDEX_STEMS: [&str; 5] = [
    "chunks",
    "tiles",
    "tile_deps",
    "chunk_tiles",
    "free_extents",
];
// Keep one prior generation for readers that opened the header before a commit.
const TILE_AUTHORITY_RETAINED_GENERATIONS: u64 = 2;
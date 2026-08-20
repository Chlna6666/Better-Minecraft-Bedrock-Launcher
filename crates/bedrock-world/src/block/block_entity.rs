//! Preservation-first handling for historical Minecraft Bedrock `BlockEntity` NBT.
//!
//! Block entities remain chunk-scoped `BlockEntity` records rather than actor records. There is no
//! universal block-entity schema version embedded in every root compound, so writes are selected by
//! concrete caller evidence and only rewrite layouts whose historical shape is known.

use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{StorageBatch, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_root_nbt_with_consumed, serialize_root_nbt};
use bytes::Bytes;
use indexmap::IndexMap;

/// Version evidence supplied to one block-entity rewrite pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockEntityRewriteContext {
    /// Source `LevelChunk` version when already known by the caller.
    pub source_chunk_version: Option<u8>,
    /// Target `LevelChunk` version selected by the caller.
    pub target_chunk_version: Option<u8>,
}

/// Result classification for one block-entity root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEntityRewriteStatus {
    /// The root already uses the requested persisted shape.
    Unchanged,
    /// The root was rewritten while retaining fields not owned by the rewriter.
    Rewritten,
    /// No authoritative rewrite matched and the original root must be retained byte-for-byte.
    Preserved,
}

/// Result of rewriting one block-entity root NBT compound.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntityRewriteOutcome {
    /// Rewritten or preserved NBT root.
    pub nbt: NbtTag,
    /// Classification of the write action.
    pub status: BlockEntityRewriteStatus,
}

/// Block-entity rewrite backend for one explicitly selected persisted shape.
///
/// Implementations must preserve fields they do not explicitly own. Returning an error aborts the
/// whole chunk rewrite before storage mutation.
pub trait BlockEntityRewriter: Send + Sync {
    /// Rewrites one parsed block-entity NBT root.
    fn rewrite(
        &self,
        nbt: &NbtTag,
        context: BlockEntityRewriteContext,
    ) -> Result<BlockEntityRewriteOutcome>;
}

/// Conservative vanilla rewriter for confirmed historical block-entity layouts.
///
/// The current built-in rule rewrites Sign text from `Text1`..`Text4` or `Text` into the
/// `FrontText`/`BackText` representation. Unknown identifiers and unrecognised layouts are preserved
/// unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct VanillaBlockEntityRewriter;

impl BlockEntityRewriter for VanillaBlockEntityRewriter {
    fn rewrite(
        &self,
        nbt: &NbtTag,
        _context: BlockEntityRewriteContext,
    ) -> Result<BlockEntityRewriteOutcome> {
        let NbtTag::Compound(root) = nbt else {
            return Err(BedrockWorldError::Validation(
                "block-entity root must be an NBT compound".to_string(),
            ));
        };
        let Some(id) = string_field(root, "id") else {
            return Ok(BlockEntityRewriteOutcome {
                nbt: nbt.clone(),
                status: BlockEntityRewriteStatus::Preserved,
            });
        };
        if !is_sign_identifier(id) {
            return Ok(BlockEntityRewriteOutcome {
                nbt: nbt.clone(),
                status: BlockEntityRewriteStatus::Preserved,
            });
        }
        rewrite_sign(root)
    }
}

/// Summary of one chunk-scoped `BlockEntity` payload rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockEntityChunkRewriteReport {
    /// Number of consecutive NBT roots inspected.
    pub roots_seen: usize,
    /// Number of roots rewritten by the selected rewriter.
    pub roots_rewritten: usize,
    /// Number of roots already using the selected representation.
    pub roots_unchanged: usize,
    /// Number of roots preserved because no authoritative rewrite matched.
    pub roots_preserved: usize,
    /// Whether the LevelDB value changed.
    pub payload_rewritten: bool,
}

/// Rewrites one chunk's `BlockEntity` payload atomically.
///
/// Every consecutive NBT root is parsed and processed before any storage write is issued. Roots marked
/// `Unchanged` or `Preserved` are copied byte-for-byte from the source payload; only `Rewritten` roots
/// are serialized again. An empty payload is treated as corrupt rather than deleted or replaced.
pub fn rewrite_block_entity_chunk_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    rewriter: &dyn BlockEntityRewriter,
    context: BlockEntityRewriteContext,
) -> Result<BlockEntityChunkRewriteReport> {
    let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
    let Some(raw) = storage.get(&key)? else {
        return Ok(BlockEntityChunkRewriteReport::default());
    };

    let mut remaining = raw.as_ref();
    let mut report = BlockEntityChunkRewriteReport::default();
    let mut encoded = Vec::with_capacity(raw.len());
    while !remaining.is_empty() {
        let (root, consumed) = parse_root_nbt_with_consumed(remaining)?;
        if consumed == 0 || consumed > remaining.len() {
            return Err(BedrockWorldError::Nbt(
                "BlockEntity consecutive NBT parser did not advance".to_string(),
            ));
        }
        report.roots_seen = report.roots_seen.saturating_add(1);
        let outcome = rewriter.rewrite(&root, context)?;
        match outcome.status {
            BlockEntityRewriteStatus::Unchanged => {
                if outcome.nbt != root {
                    return Err(BedrockWorldError::Validation(
                        "BlockEntity rewriter returned modified NBT with Unchanged status"
                            .to_string(),
                    ));
                }
                encoded.extend_from_slice(&remaining[..consumed]);
                report.roots_unchanged = report.roots_unchanged.saturating_add(1);
            }
            BlockEntityRewriteStatus::Rewritten => {
                encoded.extend_from_slice(&serialize_root_nbt(&outcome.nbt)?);
                report.roots_rewritten = report.roots_rewritten.saturating_add(1);
            }
            BlockEntityRewriteStatus::Preserved => {
                if outcome.nbt != root {
                    return Err(BedrockWorldError::Validation(
                        "BlockEntity rewriter returned modified NBT with Preserved status"
                            .to_string(),
                    ));
                }
                encoded.extend_from_slice(&remaining[..consumed]);
                report.roots_preserved = report.roots_preserved.saturating_add(1);
            }
        }
        remaining = &remaining[consumed..];
    }

    if report.roots_seen == 0 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "BlockEntity record for chunk ({}, {}, {}) contains no NBT roots",
            pos.x,
            pos.z,
            pos.dimension.id()
        )));
    }
    if encoded.as_slice() == raw.as_ref() {
        return Ok(report);
    }
    let mut batch = StorageBatch::new();
    batch.put(key, Bytes::from(encoded));
    storage.write_batch(&batch)?;
    report.payload_rewritten = true;
    Ok(report)
}

/// Rewrites confirmed historical Sign text layouts using the built-in vanilla rules.
pub fn rewrite_block_entity_sign_text_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    context: BlockEntityRewriteContext,
) -> Result<BlockEntityChunkRewriteReport> {
    rewrite_block_entity_chunk_blocking(storage, pos, &VanillaBlockEntityRewriter, context)
}

fn rewrite_sign(root: &IndexMap<String, NbtTag>) -> Result<BlockEntityRewriteOutcome> {
    if matches!(root.get("FrontText"), Some(NbtTag::Compound(_))) {
        let mut rewritten = root.clone();
        let mut changed = false;
        if !matches!(rewritten.get("BackText"), Some(NbtTag::Compound(_))) {
            rewritten.insert("BackText".to_string(), empty_sign_text());
            changed = true;
        }
        if !matches!(rewritten.get("IsWaxed"), Some(NbtTag::Byte(_))) {
            rewritten.insert("IsWaxed".to_string(), NbtTag::Byte(0));
            changed = true;
        }
        return Ok(BlockEntityRewriteOutcome {
            nbt: NbtTag::Compound(rewritten),
            status: if changed {
                BlockEntityRewriteStatus::Rewritten
            } else {
                BlockEntityRewriteStatus::Unchanged
            },
        });
    }

    let legacy_blob = string_field(root, "Text").map(str::to_string);
    let legacy_lines = read_legacy_sign_lines(root);
    let Some(text) = legacy_blob.or(legacy_lines) else {
        return Ok(BlockEntityRewriteOutcome {
            nbt: NbtTag::Compound(root.clone()),
            status: BlockEntityRewriteStatus::Preserved,
        });
    };

    let lighting_bug_resolved = matches!(
        byte_field(root, "TextIgnoreLegacyBugResolved"),
        Some(value) if value != 0
    );
    let glowing = lighting_bug_resolved && byte_field(root, "IgnoreLighting").unwrap_or(0) != 0;
    let color = int_field(root, "SignTextColor").unwrap_or(-16_777_216);
    let persist_formatting = byte_field(root, "PersistFormatting").unwrap_or(1);

    let mut front = IndexMap::new();
    front.insert("Text".to_string(), NbtTag::String(text));
    front.insert("SignTextColor".to_string(), NbtTag::Int(color));
    front.insert(
        "IgnoreLighting".to_string(),
        NbtTag::Byte(if glowing { 1 } else { 0 }),
    );
    front.insert(
        "PersistFormatting".to_string(),
        NbtTag::Byte(persist_formatting),
    );

    let mut rewritten = root.clone();
    rewritten.insert("FrontText".to_string(), NbtTag::Compound(front));
    rewritten
        .entry("BackText".to_string())
        .or_insert_with(empty_sign_text);
    rewritten
        .entry("IsWaxed".to_string())
        .or_insert(NbtTag::Byte(0));

    Ok(BlockEntityRewriteOutcome {
        nbt: NbtTag::Compound(rewritten),
        status: BlockEntityRewriteStatus::Rewritten,
    })
}

fn empty_sign_text() -> NbtTag {
    NbtTag::Compound(IndexMap::from([
        ("Text".to_string(), NbtTag::String(String::new())),
        ("SignTextColor".to_string(), NbtTag::Int(-16_777_216)),
        ("IgnoreLighting".to_string(), NbtTag::Byte(0)),
        ("PersistFormatting".to_string(), NbtTag::Byte(1)),
    ]))
}

fn read_legacy_sign_lines(root: &IndexMap<String, NbtTag>) -> Option<String> {
    let mut saw_line = false;
    let mut lines = Vec::with_capacity(4);
    for index in 1..=4 {
        let key = format!("Text{index}");
        let line = string_field(root, &key).unwrap_or_default();
        saw_line |= root.contains_key(&key);
        lines.push(line.to_string());
    }
    if !saw_line {
        return None;
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    Some(lines.join("\n"))
}

fn is_sign_identifier(id: &str) -> bool {
    matches!(
        id,
        "Sign" | "minecraft:sign" | "HangingSign" | "minecraft:hanging_sign"
    )
}

fn string_field<'a>(root: &'a IndexMap<String, NbtTag>, key: &str) -> Option<&'a str> {
    match root.get(key)? {
        NbtTag::String(value) => Some(value),
        _ => None,
    }
}

fn byte_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<i8> {
    match root.get(key)? {
        NbtTag::Byte(value) => Some(*value),
        _ => None,
    }
}

fn int_field(root: &IndexMap<String, NbtTag>, key: &str) -> Option<i32> {
    match root.get(key)? {
        NbtTag::Int(value) => Some(*value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Dimension;
    use crate::database::{MemoryStorage, WorldStorage};

    fn sign_with(entries: impl IntoIterator<Item = (String, NbtTag)>) -> NbtTag {
        let mut root = IndexMap::from([
            ("id".to_string(), NbtTag::String("Sign".to_string())),
            ("x".to_string(), NbtTag::Int(1)),
            ("y".to_string(), NbtTag::Int(64)),
            ("z".to_string(), NbtTag::Int(2)),
        ]);
        root.extend(entries);
        NbtTag::Compound(root)
    }

    #[test]
    fn four_line_sign_is_rewritten_without_dropping_legacy_fields() {
        let root = sign_with([
            ("Text1".to_string(), NbtTag::String("one".to_string())),
            ("Text2".to_string(), NbtTag::String("two".to_string())),
        ]);
        let output = VanillaBlockEntityRewriter
            .rewrite(&root, BlockEntityRewriteContext::default())
            .expect("rewrite sign");
        assert_eq!(output.status, BlockEntityRewriteStatus::Rewritten);
        let NbtTag::Compound(values) = output.nbt else {
            panic!("compound");
        };
        assert_eq!(
            values.get("Text1"),
            Some(&NbtTag::String("one".to_string()))
        );
        let Some(NbtTag::Compound(front)) = values.get("FrontText") else {
            panic!("front text");
        };
        assert_eq!(
            front.get("Text"),
            Some(&NbtTag::String("one\ntwo".to_string()))
        );
        assert!(matches!(values.get("BackText"), Some(NbtTag::Compound(_))));
    }

    #[test]
    fn unresolved_legacy_lighting_bug_does_not_create_glowing_text() {
        let root = sign_with([
            ("Text".to_string(), NbtTag::String("hello".to_string())),
            ("IgnoreLighting".to_string(), NbtTag::Byte(1)),
        ]);
        let output = VanillaBlockEntityRewriter
            .rewrite(&root, BlockEntityRewriteContext::default())
            .expect("rewrite sign");
        let NbtTag::Compound(values) = output.nbt else {
            panic!("compound");
        };
        let Some(NbtTag::Compound(front)) = values.get("FrontText") else {
            panic!("front text");
        };
        assert_eq!(front.get("IgnoreLighting"), Some(&NbtTag::Byte(0)));
    }

    #[test]
    fn unknown_block_entity_is_preserved() {
        let root = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("FutureThing".to_string())),
            ("future".to_string(), NbtTag::Long(42)),
        ]));
        let output = VanillaBlockEntityRewriter
            .rewrite(&root, BlockEntityRewriteContext::default())
            .expect("preserve");
        assert_eq!(output.status, BlockEntityRewriteStatus::Preserved);
        assert_eq!(output.nbt, root);
    }

    #[test]
    fn preserved_sibling_root_keeps_exact_bytes_when_sign_changes() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
        let future = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("FutureThing".to_string())),
            ("future".to_string(), NbtTag::Long(42)),
        ]));
        let future_raw = serialize_root_nbt(&future).unwrap();
        let sign = sign_with([("Text".to_string(), NbtTag::String("hello".to_string()))]);
        let mut payload = future_raw.clone();
        payload.extend_from_slice(&serialize_root_nbt(&sign).unwrap());
        storage.put(&key, &payload).unwrap();

        let report = rewrite_block_entity_sign_text_blocking(
            &storage,
            pos,
            BlockEntityRewriteContext::default(),
        )
        .unwrap();
        assert_eq!(report.roots_preserved, 1);
        assert_eq!(report.roots_rewritten, 1);
        let rewritten = storage.get(&key).unwrap().unwrap();
        assert_eq!(&rewritten[..future_raw.len()], future_raw.as_slice());
    }

    #[test]
    fn chunk_payload_rewrite_is_atomic_and_idempotent() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let key = ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode();
        let root = sign_with([("Text".to_string(), NbtTag::String("hello".to_string()))]);
        storage
            .put(&key, &serialize_root_nbt(&root).unwrap())
            .unwrap();

        let first = rewrite_block_entity_sign_text_blocking(
            &storage,
            pos,
            BlockEntityRewriteContext::default(),
        )
        .unwrap();
        assert_eq!(first.roots_rewritten, 1);
        assert!(first.payload_rewritten);

        let second = rewrite_block_entity_sign_text_blocking(
            &storage,
            pos,
            BlockEntityRewriteContext::default(),
        )
        .unwrap();
        assert_eq!(second.roots_unchanged, 1);
        assert!(!second.payload_rewritten);
    }
}

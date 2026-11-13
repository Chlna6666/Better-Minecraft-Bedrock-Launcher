//! Backend-independent world editing and guarded storage commits.

use bedrock_world::{
    BedrockWorldError, ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LevelDatDocument,
    MemoryStorage, NbtTag, OpenOptions, World, WorldStorage, chunk::ticks::ChunkTicks, global,
    level, nbt, storage::GlobalRecordKind,
};
use bytes::Bytes;
use indexmap::IndexMap;
use std::sync::Arc;

fn root(value: i64) -> NbtTag {
    NbtTag::Compound(IndexMap::from([
        ("currentTick".into(), NbtTag::Long(value)),
        ("future".into(), NbtTag::ByteArray(vec![-1, 0, 2])),
    ]))
}

fn world(name: &str, read_only: bool) -> (World, Arc<MemoryStorage>) {
    let storage = Arc::new(MemoryStorage::new());
    let world = World::from_storage(
        name,
        storage.clone(),
        OpenOptions {
            read_only,
            ..OpenOptions::default()
        },
    );
    (world, storage)
}

fn pos() -> ChunkPos {
    ChunkPos {
        x: -2,
        z: 3,
        dimension: Dimension::Nether,
    }
}

#[test]
fn level_fields_preserve_absence_types_unknown_data_and_order() {
    let mut document = LevelDatDocument::new(10, root(42));
    assert!(!document.has_field("commandsEnabled").unwrap());
    assert_eq!(
        document
            .set_field("commandsEnabled".into(), NbtTag::Byte(0))
            .unwrap(),
        None
    );
    assert!(document.has_field("commandsEnabled").unwrap());
    assert_eq!(
        document.field("commandsEnabled").unwrap(),
        Some(&NbtTag::Byte(0))
    );
    *document.field_mut("currentTick").unwrap().unwrap() = NbtTag::Long(43);
    assert_eq!(
        document.remove_field("commandsEnabled").unwrap(),
        Some(NbtTag::Byte(0))
    );
    assert_eq!(document.remove_field("missing").unwrap(), None);
    let encoded = level::encode(&document).unwrap();
    let parsed = level::parse(&encoded).unwrap();
    assert_eq!(parsed.root, root(43));
    assert_eq!(parsed.version(), 10);
    assert!(parsed.warnings.is_empty());
}

#[test]
fn level_field_edits_reject_invalid_roots_and_end_markers() {
    let mut invalid = LevelDatDocument::new(10, NbtTag::Int(1));
    assert!(invalid.has_field("x").is_err());
    assert!(invalid.set_field("x".into(), NbtTag::Int(2)).is_err());
    assert!(invalid.remove_field("x").is_err());
    assert!(level::encode(&invalid).is_err());
    let mut valid = LevelDatDocument::new(10, root(1));
    assert!(valid.set_field("invalid".into(), NbtTag::End).is_err());
    assert!(!valid.has_field("invalid").unwrap());
}

#[test]
fn detached_ticks_distinguish_empty_records_from_absence() {
    let empty = ChunkTicks::from_records(Some(Bytes::new()), None).unwrap();
    assert!(empty.has_pending());
    assert_eq!(empty.pending(), Some(&[][..]));
    assert_eq!(empty.encode_pending().unwrap(), Some(Bytes::new()));
    assert!(!empty.has_random());
    assert_eq!(empty.encode_random().unwrap(), None);
    let roots = vec![root(1), root(2)];
    let mut ticks = ChunkTicks::default();
    ticks.set_pending(roots.clone());
    let parsed = ChunkTicks::from_records(ticks.encode_pending().unwrap(), None).unwrap();
    assert_eq!(parsed.pending(), Some(roots.as_slice()));
    assert!(ChunkTicks::from_records(Some(Bytes::from_static(&[10, 0])), None).is_err());
}

#[test]
fn invalid_second_tick_queue_does_not_stage_the_first() {
    let (world, storage) = world("invalid-tick-edit", false);
    let mut ticks = ChunkTicks::default();
    ticks.set_pending(vec![root(1)]);
    ticks.set_random(vec![NbtTag::Int(1)]);
    let mut transaction = world.transaction();
    assert!(transaction.write_chunk_ticks(pos(), &ticks).is_err());
    transaction.commit().unwrap();
    for tag in [ChunkRecordTag::PendingTicks, ChunkRecordTag::RandomTicks] {
        assert!(
            storage
                .get(&ChunkKey::new(pos(), tag).encode())
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn ticks_roundtrip_and_stale_edits_cannot_replace_or_delete_newer_data() {
    let (world, _) = world("tick-source-guard", false);
    let mut ticks = world.chunk_ticks(pos()).unwrap();
    ticks.set_pending(vec![root(1)]);
    ticks.set_random(Vec::new());
    world.write_chunk_ticks(pos(), &ticks).unwrap();
    let mut current = world.chunk_ticks(pos()).unwrap();
    let mut stale = current.clone();
    current.set_pending(vec![root(2)]);
    world.write_chunk_ticks(pos(), &current).unwrap();
    stale.remove_pending();
    stale.remove_random();
    assert!(matches!(
        world.write_chunk_ticks(pos(), &stale),
        Err(BedrockWorldError::ConcurrentWrite(_))
    ));
    let saved = world.chunk_ticks(pos()).unwrap();
    assert_eq!(saved.pending(), Some(&[root(2)][..]));
    assert!(saved.has_random());
    let mut removal = saved;
    removal.remove_pending();
    removal.remove_random();
    world.write_chunk_ticks(pos(), &removal).unwrap();
    let removed = world.chunk_ticks(pos()).unwrap();
    assert!(!removed.has_pending());
    assert!(!removed.has_random());
}

#[test]
fn global_empty_source_is_present_and_can_be_edited() {
    let (world, storage) = world("global-empty-source", false);
    let kind = GlobalRecordKind::MobEvents;
    assert!(!world.has_global(&kind).unwrap());
    storage.put(&kind.storage_key(), &[]).unwrap();
    assert!(world.has_global(&kind).unwrap());
    let mut record = world.global(kind.clone()).unwrap().unwrap();
    assert_eq!(record.source(), Some(&Bytes::new()));
    record.roots_mut().push(root(1));
    world.save_global(&record).unwrap();
    assert_eq!(
        world.global(kind.clone()).unwrap().unwrap().roots(),
        &[root(1)]
    );
    let fresh = global::Record::new(kind.clone(), vec![root(2)]);
    assert!(matches!(
        world.save_global(&fresh),
        Err(BedrockWorldError::ConcurrentWrite(_))
    ));
    assert!(matches!(
        world.remove_global(&record),
        Err(BedrockWorldError::ConcurrentWrite(_))
    ));
    let current = world.global(kind.clone()).unwrap().unwrap();
    world.remove_global(&current).unwrap();
    assert!(!world.has_global(&kind).unwrap());
}

#[test]
fn detached_global_records_preserve_unknown_fields_without_storage() {
    let original = global::Record::new(GlobalRecordKind::Scoreboard, vec![root(1)]);
    let mut parsed =
        global::parse(original.kind().clone(), global::encode(&original).unwrap()).unwrap();
    parsed.roots_mut().push(root(2));
    let saved = global::parse(parsed.kind().clone(), global::encode(&parsed).unwrap()).unwrap();
    assert_eq!(saved.roots(), &[root(1), root(2)]);
    let empty = global::Record::new(GlobalRecordKind::MobEvents, Vec::new());
    assert!(global::encode(&empty).unwrap().is_empty());
    assert_eq!(
        nbt::parse_consecutive_roots(&nbt::serialize_consecutive_roots(saved.roots()).unwrap())
            .unwrap(),
        saved.roots()
    );
}

#[test]
fn read_only_world_rejects_global_and_tick_edits() {
    let (world, _) = world("read-only-domains", true);
    let record = global::Record::new(GlobalRecordKind::MobEvents, vec![root(1)]);
    assert!(matches!(
        world.save_global(&record),
        Err(BedrockWorldError::ReadOnly)
    ));
    assert!(matches!(
        world.remove_global(&record),
        Err(BedrockWorldError::ReadOnly)
    ));
    assert!(matches!(
        world.write_chunk_ticks(pos(), &ChunkTicks::default()),
        Err(BedrockWorldError::ReadOnly)
    ));
}

#[test]
fn structure_volumes_are_retained_as_binary_and_never_classified_as_versions() {
    use bedrock_world::scan::{self, ChunkValue, ScanOptions, ScanValue};
    let (world, storage) = world("structure-volume-tag", false);
    let key = ChunkKey::new(pos(), ChunkRecordTag::from_byte(0x77));
    let binary = Bytes::from_static(&[1, 2, 3, 4, 5]);
    storage.put(&key.encode(), &binary).unwrap();
    let chunk = world.chunk(pos()).unwrap();
    assert_eq!(chunk.records[0].key.tag, ChunkRecordTag::AabbVolumes);
    let capabilities = bedrock_world::ChunkCapabilities::inspect(&chunk.records);
    assert!(!capabilities.has_version_record);
    assert_eq!(
        capabilities.compatibility,
        bedrock_world::CompatibilityLevel::ReadCompatible
    );
    let parsed = scan::parse_world(
        LevelDatDocument::new(10, root(0)),
        storage.as_ref(),
        ScanOptions::full(),
    )
    .unwrap();
    let ScanValue::Chunk(entry) = &parsed.entries[0].value else {
        panic!("chunk entry")
    };
    assert_eq!(entry.value, ChunkValue::Raw(binary));
    assert!(parsed.report.parse_errors.is_empty());
}

#[test]
fn spawn_area_edits_roundtrip_and_reject_stale_deletion() {
    use bedrock_world::chunk::{HardcodedSpawnArea, HardcodedSpawnAreaKind};
    let (world, _) = world("spawn-area-edit", false);
    let area = HardcodedSpawnArea {
        kind: HardcodedSpawnAreaKind::Unknown(77),
        min: [-32, 4, 48],
        max: [-17, 9, 63],
    };
    let (areas, source) = world.hardcoded_spawn_areas_snapshot(pos()).unwrap();
    assert!(areas.is_empty());
    let mut transaction = world.transaction();
    transaction
        .update_hardcoded_spawn_areas(pos(), source, std::slice::from_ref(&area))
        .unwrap();
    transaction.commit().unwrap();
    let (loaded, source) = world.hardcoded_spawn_areas_snapshot(pos()).unwrap();
    assert_eq!(loaded, vec![area.clone()]);
    let decoded: HardcodedSpawnArea =
        serde_json::from_str(&serde_json::to_string(&area).unwrap()).unwrap();
    assert_eq!(decoded, area);
    let mut transaction = world.transaction();
    transaction
        .update_hardcoded_spawn_areas(pos(), source.clone(), &[])
        .unwrap();
    transaction.commit().unwrap();
    let mut stale = world.transaction();
    stale.remove_hardcoded_spawn_areas(pos(), source);
    assert!(matches!(
        stale.commit(),
        Err(BedrockWorldError::ConcurrentWrite(_))
    ));
}

#[test]
fn block_entities_parse_without_storage_and_allow_guarded_empty_edits() {
    use bedrock_world::block::BlockEntity;
    let (world, _) = world("block-entity-edit", false);
    let entity_nbt = NbtTag::Compound(IndexMap::from([
        ("x".into(), NbtTag::Int(-32)),
        ("y".into(), NbtTag::Int(4)),
        ("z".into(), NbtTag::Int(48)),
        ("future".into(), NbtTag::Long(42)),
    ]));
    let entity = BlockEntity::from_nbt(entity_nbt.clone()).unwrap();
    assert_eq!(entity.nbt, entity_nbt);
    assert!(BlockEntity::from_nbt(NbtTag::Int(3)).is_err());
    let mut transaction = world.transaction();
    transaction
        .update_block_entities(pos(), None, &[entity])
        .unwrap();
    transaction.commit().unwrap();
    let (entities, source) = world.block_entities_snapshot(pos()).unwrap();
    assert_eq!(entities[0].entity.nbt, entity_nbt);
    let mut transaction = world.transaction();
    transaction
        .update_block_entities(pos(), source.clone(), &[])
        .unwrap();
    transaction.commit().unwrap();
    let (empty, present) = world.block_entities_snapshot(pos()).unwrap();
    assert!(empty.is_empty());
    assert_eq!(present, Some(Bytes::new()));
    let mut stale = world.transaction();
    stale.update_block_entities(pos(), source, &[]).unwrap();
    assert!(matches!(
        stale.commit(),
        Err(BedrockWorldError::ConcurrentWrite(_))
    ));
}

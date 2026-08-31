use super::*;
use crate::chunk::{
    LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN, LEGACY_TERRAIN_BLOCK_COUNT, LEGACY_TERRAIN_VALUE_LEN,
    LegacySubChunk,
};
use crate::parsed::HardcodedSpawnAreaKind;
use crate::{
    Dimension, MemoryStorage, NbtTag, StorageBatch, StorageReadOptions, StorageScanOutcome,
    block_storage_index,
};
use indexmap::IndexMap;
use std::sync::Arc;

#[derive(Clone)]
struct KeyOnlyPlayerStorage;

impl WorldStorage for KeyOnlyPlayerStorage {
    fn get(&self, _key: &[u8]) -> Result<Option<Bytes>> {
        Ok(None)
    }

    fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn delete(&self, _key: &[u8]) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn for_each_key(
        &self,
        _options: StorageReadOptions,
        _visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        Ok(StorageScanOutcome::empty())
    }

    fn for_each_prefix(
        &self,
        _prefix: &[u8],
        _options: StorageReadOptions,
        _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        Err(BedrockWorldError::Validation(
            "player listing requested values".to_string(),
        ))
    }

    fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        _options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        assert_eq!(prefix, b"player_");
        let _ = visitor(b"player_12345")?;
        Ok(StorageScanOutcome::empty())
    }

    fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
        Err(BedrockWorldError::ReadOnly)
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(feature = "bedrock-leveldb")]
fn temp_world_dir(name: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};

    std::env::temp_dir().join(format!(
        "bedrock-world-{name}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ))
}

fn exact_surface_request(
    subchunks: ExactSurfaceSubchunkPolicy,
    biome: ExactSurfaceBiomeLoad,
    block_entities: bool,
) -> ChunkDataRequest {
    ChunkDataRequest::new()
        .surface_columns(subchunks)
        .biome(match biome {
            ExactSurfaceBiomeLoad::None => BiomeDataRequirement::None,
            ExactSurfaceBiomeLoad::TopColumns => BiomeDataRequirement::SurfaceColumns,
            ExactSurfaceBiomeLoad::All => BiomeDataRequirement::All,
        })
        .block_entities_if(block_entities)
}

#[test]
fn player_listing_uses_key_only_prefix_scan() {
    let world = BedrockWorld::from_typed_storage(
        "memory",
        KeyOnlyPlayerStorage,
        BedrockWorldOpenOptions::default(),
    );

    assert_eq!(
        world.list_players_blocking().expect("list players"),
        vec![PlayerId::Xuid("12345".to_string())]
    );
}

#[test]
fn world_threading_uses_bounded_desktop_background_budget() {
    let expected_auto = default_world_worker_budget().min(10_000);
    assert_eq!(
        WorldThreadingOptions::Auto
            .resolve_checked(10_000)
            .expect("auto threads"),
        expected_auto
    );
    assert_eq!(
        WorldThreadingOptions::Fixed(MAX_WORLD_THREADS)
            .resolve_checked(10_000)
            .expect("max fixed threads"),
        MAX_WORLD_THREADS
    );
    assert!(WorldThreadingOptions::Fixed(0).resolve_checked(10).is_err());
    assert!(
        WorldThreadingOptions::Fixed(MAX_WORLD_THREADS + 1)
            .resolve_checked(10)
            .is_err()
    );
}

#[test]
fn map_and_global_records_roundtrip_through_world_transactions() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let map_id = MapRecordId::new("9").expect("map id");
    let map = ParsedMapData {
        id: map_id.to_string(),
        record_id: map_id.clone(),
        roots: vec![NbtTag::Compound(IndexMap::from([(
            "scale".to_string(),
            NbtTag::Byte(1),
        )]))],
        known_fields: crate::map::MapKnownFields::default(),
        pixels: None,
        raw: Bytes::new(),
    };

    world.write_map_record_blocking(&map).expect("write map");
    let read_map = world
        .read_map_record_blocking(&map_id)
        .expect("read map")
        .expect("map exists");
    assert_eq!(read_map.known_fields.scale, Some(1));

    let global = ParsedGlobalData {
        name: "scoreboard".to_string(),
        kind: GlobalRecordKind::Scoreboard,
        roots: vec![NbtTag::Compound(IndexMap::new())],
        raw: Bytes::new(),
    };
    world
        .write_global_record_blocking(&global)
        .expect("write global");
    assert!(
        world
            .read_global_record_blocking(GlobalRecordKind::Scoreboard)
            .expect("read global")
            .is_some()
    );

    world
        .delete_map_record_blocking(&map_id)
        .expect("delete map");
    assert!(
        world
            .read_map_record_blocking(&map_id)
            .expect("read deleted")
            .is_none()
    );
}

#[test]
fn hsa_and_block_entities_roundtrip_with_chunk_validation() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage,
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let area = ParsedHardcodedSpawnArea {
        kind: HardcodedSpawnAreaKind::NetherFortress,
        min: [0, 32, 0],
        max: [15, 80, 15],
    };
    world
        .put_hsa_for_chunk_blocking(pos, std::slice::from_ref(&area))
        .expect("write hsa");
    assert_eq!(
        world
            .scan_hsa_records_blocking(WorldScanOptions::default())
            .expect("scan hsa")[0]
            .1,
        vec![area]
    );

    let block_entity = ParsedBlockEntity {
        id: Some("Chest".to_string()),
        position: Some([1, 64, 1]),
        is_movable: Some(true),
        custom_name: None,
        items: Vec::new(),
        nbt: NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("Chest".to_string())),
            ("x".to_string(), NbtTag::Int(1)),
            ("y".to_string(), NbtTag::Int(64)),
            ("z".to_string(), NbtTag::Int(1)),
        ])),
    };
    world
        .put_block_entities_blocking(pos, std::slice::from_ref(&block_entity))
        .expect("write block entity");
    assert_eq!(
        world
            .block_entities_in_chunk_blocking(pos)
            .expect("read block entities")[0]
            .entity
            .position,
        Some([1, 64, 1])
    );
}

#[test]
fn actor_write_updates_digest_and_prefix_together() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let pos = ChunkPos {
        x: 2,
        z: 3,
        dimension: Dimension::Overworld,
    };
    let actor_nbt = NbtTag::Compound(IndexMap::from([
        (
            "identifier".to_string(),
            NbtTag::String("minecraft:pig".to_string()),
        ),
        ("UniqueID".to_string(), NbtTag::Long(77)),
        (
            "Pos".to_string(),
            NbtTag::List(vec![
                NbtTag::Float(32.0),
                NbtTag::Float(64.0),
                NbtTag::Float(48.0),
            ]),
        ),
    ]));
    let actor = ParsedEntity {
        identifier: Some("minecraft:pig".to_string()),
        definitions: Vec::new(),
        unique_id: Some(77),
        position: Some([32.0, 64.0, 48.0]),
        rotation: None,
        motion: None,
        items: Vec::new(),
        nbt: actor_nbt,
    };
    let actor_uid = ActorUid::from_unique_id(77);

    world.put_actor_blocking(pos, &actor).expect("put actor");
    let digest = storage
        .get(&ActorDigestKey::new(pos).storage_key())
        .expect("get digest")
        .expect("digest exists");
    assert_eq!(
        parse_actor_digest_ids(&digest).expect("parse digest"),
        vec![actor_uid]
    );
    assert!(
        storage
            .get(&actor_uid.storage_key())
            .expect("get actor")
            .is_some()
    );

    world
        .delete_actor_blocking(pos, actor_uid)
        .expect("delete actor");
    assert!(
        storage
            .get(&ActorDigestKey::new(pos).storage_key())
            .expect("get deleted digest")
            .is_none()
    );
    assert!(
        storage
            .get(&actor_uid.storage_key())
            .expect("get deleted actor")
            .is_none()
    );
}

fn actor_owner_pos(x: i32) -> ChunkPos {
    ChunkPos {
        x,
        z: 0,
        dimension: Dimension::Overworld,
    }
}

fn seed_shared_actor(storage: &MemoryStorage, uid: ActorUid, owners: &[ChunkPos]) {
    for owner in owners {
        storage
            .put(
                &ActorDigestKey::new(*owner).storage_key(),
                &encode_actor_digest_ids(&[uid]),
            )
            .expect("seed actor digest");
    }
    storage
        .put(&uid.storage_key(), b"actor-payload")
        .expect("seed actor payload");
}

#[test]
fn deleting_one_of_multiple_actor_owners_preserves_actor_payload() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let uid = ActorUid::from_unique_id(501);
    let first = actor_owner_pos(1);
    let second = actor_owner_pos(2);
    seed_shared_actor(storage.as_ref(), uid, &[first, second]);

    let mut transaction = world.transaction();
    transaction.delete_chunk(first).expect("delete first chunk");
    transaction.commit().expect("commit first deletion");

    assert!(
        storage
            .get(&ActorDigestKey::new(first).storage_key())
            .unwrap()
            .is_none()
    );
    assert!(
        storage
            .get(&ActorDigestKey::new(second).storage_key())
            .unwrap()
            .is_some()
    );
    assert!(storage.get(&uid.storage_key()).unwrap().is_some());
}

#[test]
fn deleting_last_staged_actor_owner_deletes_actor_payload() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let uid = ActorUid::from_unique_id(502);
    let first = actor_owner_pos(1);
    let second = actor_owner_pos(2);
    seed_shared_actor(storage.as_ref(), uid, &[first, second]);

    let mut transaction = world.transaction();
    transaction.delete_chunk(first).expect("delete first chunk");
    transaction
        .delete_chunk(second)
        .expect("delete second chunk");
    transaction.commit().expect("commit both deletions");

    assert!(storage.get(&uid.storage_key()).unwrap().is_none());
}

#[test]
fn actor_move_rejects_destination_while_another_digest_still_owns_uid() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let uid = ActorUid::from_unique_id(503);
    let first = actor_owner_pos(1);
    let second = actor_owner_pos(2);
    let destination = actor_owner_pos(3);
    seed_shared_actor(storage.as_ref(), uid, &[first, second]);

    let mut transaction = world.transaction();
    transaction
        .delete_actor(first, uid)
        .expect("stage source removal");
    assert!(
        transaction
            .put_actor(destination, uid, Bytes::from_static(b"moved-payload"))
            .is_err()
    );
    drop(transaction);

    assert!(
        storage
            .get(&ActorDigestKey::new(first).storage_key())
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .get(&ActorDigestKey::new(second).storage_key())
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .get(&ActorDigestKey::new(destination).storage_key())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage.get(&uid.storage_key()).unwrap().unwrap(),
        Bytes::from_static(b"actor-payload")
    );
}

#[test]
fn actor_overwrite_rejects_new_digest_owner_without_changing_storage() {
    let storage = Arc::new(MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let uid = ActorUid::from_unique_id(504);
    let owner = actor_owner_pos(1);
    let destination = actor_owner_pos(4);
    seed_shared_actor(storage.as_ref(), uid, &[owner]);

    let mut transaction = world.transaction();
    assert!(
        transaction
            .put_actor(destination, uid, Bytes::from_static(b"replacement"))
            .is_err()
    );
    drop(transaction);

    assert!(
        storage
            .get(&ActorDigestKey::new(owner).storage_key())
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .get(&ActorDigestKey::new(destination).storage_key())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage.get(&uid.storage_key()).unwrap().unwrap(),
        Bytes::from_static(b"actor-payload")
    );
}

#[test]
fn render_chunk_priority_distance_orders_from_center() {
    let mut positions = vec![
        ChunkPos {
            x: 12,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: -3,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
    ];

    sort_render_chunk_positions(
        &mut positions,
        ChunkLoadPriority::DistanceFrom {
            chunk_x: 0,
            chunk_z: 0,
        },
    );

    let ordered = positions
        .iter()
        .map(|pos| (pos.x, pos.z))
        .collect::<Vec<_>>();
    assert_eq!(ordered, vec![(0, 0), (1, 0), (-3, 0), (12, 0)]);
}

#[test]
fn world_pipeline_options_resolve_automatic_bounds() {
    let options = WorldPipelineOptions::default();

    assert!(options.resolve_queue_depth(4, 64) >= 1);
    assert_eq!(options.resolve_progress_interval(), 256);

    let explicit = WorldPipelineOptions {
        queue_depth: 7,
        progress_interval: 9,
        ..WorldPipelineOptions::default()
    };
    assert_eq!(explicit.resolve_queue_depth(4, 64), 7);
    assert_eq!(explicit.resolve_progress_interval(), 9);
}

#[test]
fn generic_memory_storage_matches_dynamic_storage_queries() {
    let storage = MemoryStorage::new();
    storage
        .put(b"~local_player", b"local")
        .expect("put local player");
    storage
        .put(b"player_remote", b"remote")
        .expect("put remote player");

    let generic_world = BedrockWorld::from_typed_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions::default(),
    );
    let dynamic_world = BedrockWorld::from_storage(
        "memory",
        Arc::new(storage) as Arc<dyn WorldStorage>,
        BedrockWorldOpenOptions::default(),
    );

    assert_eq!(
        generic_world.list_players_blocking().expect("generic"),
        dynamic_world.list_players_blocking().expect("dynamic")
    );
    assert_eq!(
        generic_world
            .classify_keys_blocking(WorldScanOptions::default())
            .expect("generic classify"),
        dynamic_world
            .classify_keys_blocking(WorldScanOptions::default())
            .expect("dynamic classify")
    );
}

#[cfg(feature = "bedrock-leveldb")]
#[test]
fn generic_leveldb_storage_matches_dynamic_storage_queries() {
    let temp = temp_world_dir("generic-leveldb");
    std::fs::create_dir_all(&temp).expect("temp dir");
    let db_path = temp.join("db");
    let db = bedrock_leveldb::Db::open(&db_path, bedrock_leveldb::LevelDbOpenOptions::default())
        .expect("initialize db");
    drop(db);
    let storage = BedrockLevelDbStorage::open(&db_path).expect("open storage");
    storage
        .put(b"~local_player", b"local")
        .expect("put local player");
    storage
        .put(b"player_remote", b"remote")
        .expect("put remote player");
    storage.flush().expect("flush");

    let generic_world = BedrockWorld::from_typed_storage(
        &temp,
        storage.clone(),
        BedrockWorldOpenOptions::default(),
    );
    let dynamic_world = BedrockWorld::from_storage(
        &temp,
        Arc::new(storage) as Arc<dyn WorldStorage>,
        BedrockWorldOpenOptions::default(),
    );

    assert_eq!(
        generic_world.list_players_blocking().expect("generic"),
        dynamic_world.list_players_blocking().expect("dynamic")
    );
    assert_eq!(
        generic_world
            .classify_keys_blocking(WorldScanOptions::default())
            .expect("generic classify"),
        dynamic_world
            .classify_keys_blocking(WorldScanOptions::default())
            .expect("dynamic classify")
    );
    std::fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn transaction_respects_read_only_option() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let key = ChunkKey::new(pos, ChunkRecordTag::Version);
    let encoded = key.encode();
    let storage = Arc::new(MemoryStorage::new());
    let read_only_world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions::default(),
    );
    let mut transaction = read_only_world.transaction();
    transaction.put_raw_record(&key, Bytes::from_static(b"\x01"));

    let error = transaction.commit().expect_err("read-only commit");

    assert_eq!(error.kind(), crate::BedrockWorldErrorKind::ReadOnly);
    assert_eq!(storage.get(&encoded).expect("get"), None);

    let writable_world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let mut transaction = writable_world.transaction();
    transaction.put_raw_record(&key, Bytes::from_static(b"\x02"));
    transaction.commit().expect("writable commit");

    assert_eq!(
        storage.get(&encoded).expect("get"),
        Some(Bytes::from_static(b"\x02"))
    );
}

#[test]
fn transaction_replaces_chunk_records_and_typed_payloads_in_one_commit() {
    let pos = ChunkPos {
        x: 3,
        z: -2,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    let old_key = ChunkKey::new(pos, ChunkRecordTag::Version);
    storage
        .put(&old_key.encode(), b"\x01")
        .expect("put old chunk record");
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
    );
    let block_entity = ParsedBlockEntity {
        id: Some("Chest".to_string()),
        position: Some([49, 64, -31]),
        is_movable: None,
        custom_name: None,
        items: Vec::new(),
        nbt: NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::String("Chest".to_string())),
            ("x".to_string(), NbtTag::Int(49)),
            ("y".to_string(), NbtTag::Int(64)),
            ("z".to_string(), NbtTag::Int(-31)),
        ])),
    };
    let area = ParsedHardcodedSpawnArea {
        kind: HardcodedSpawnAreaKind::NetherFortress,
        min: [48, 32, -32],
        max: [63, 80, -17],
    };
    let new_key = ChunkKey::new(pos, ChunkRecordTag::FinalizedState);

    let mut transaction = world.transaction();
    assert_eq!(transaction.delete_chunk(pos).expect("stage delete"), 1);
    transaction.put_raw_record(&new_key, Bytes::from_static(b"\x02\0\0\0"));
    transaction
        .put_block_entities(pos, std::slice::from_ref(&block_entity))
        .expect("stage block entities");
    transaction
        .put_hsa_for_chunk(pos, std::slice::from_ref(&area))
        .expect("stage hardcoded spawn area");
    transaction.commit().expect("commit replacement");

    assert_eq!(storage.get(&old_key.encode()).expect("get old"), None);
    assert_eq!(
        storage.get(&new_key.encode()).expect("get new"),
        Some(Bytes::from_static(b"\x02\0\0\0"))
    );
    assert_eq!(
        world
            .block_entities_in_chunk_blocking(pos)
            .expect("read block entities")[0]
            .entity
            .position,
        block_entity.position
    );
    assert_eq!(
        world
            .scan_hsa_records_blocking(WorldScanOptions::default())
            .expect("read hardcoded spawn areas")[0]
            .1,
        vec![area]
    );
}

#[test]
fn biome_and_height_queries_read_legacy_data2d_in_zx_column_order() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_asymmetric_data2d_bytes(),
        )
        .expect("put Data2D");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    assert_eq!(
        world
            .get_biome_id_blocking(pos, 3, 2, 64)
            .expect("biome id"),
        Some(32)
    );
    assert_eq!(
        world
            .get_biome_id_blocking(pos, 2, 3, 64)
            .expect("biome id"),
        Some(23)
    );
    assert_eq!(
        world.get_height_at_blocking(pos, 3, 2).expect("height"),
        Some(132)
    );
    assert_eq!(
        world.get_height_at_blocking(pos, 2, 3).expect("height"),
        Some(123)
    );
}

#[test]
fn data3d_height_map_is_normalized_to_dimension_min_y() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
            &test_data3d_height_bytes(130),
        )
        .expect("put Data3D");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    assert_eq!(
        world.get_height_at_blocking(pos, 4, 2).expect("height"),
        Some(66)
    );
    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new().height_map(),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load render chunk");

    assert_eq!(
        chunk.height_map.expect("height map")[usize::from(2_u8)][usize::from(4_u8)],
        Some(66)
    );
    assert!(chunk.column_samples.is_none());
}

#[test]
fn render_chunk_exact_load_preserves_data2d_xz_height_and_biome_coordinates() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_asymmetric_data2d_bytes(),
        )
        .expect("put Data2D");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load render chunk");
    let height_map = chunk.height_map.as_ref().expect("height map");
    let biome_storage = chunk
        .biome_data
        .values()
        .next()
        .expect("render biome storage");

    assert_eq!(height_map[3][1], Some(113));
    assert_eq!(height_map[1][3], Some(131));
    assert_eq!(biome_storage.biome_id_at(1, 0, 3), Some(13));
    assert_eq!(biome_storage.biome_id_at(3, 0, 1), Some(31));
}

#[test]
fn subchunk_layer_query_uses_block_y() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(&ChunkKey::subchunk(pos, -1).encode(), &[8, 0])
        .expect("put subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let subchunk = world
        .get_subchunk_layer_blocking(pos, -1, SubChunkDecodeMode::CountsOnly)
        .expect("query")
        .expect("subchunk");
    assert_eq!(subchunk.y, -1);
}

#[test]
fn render_chunk_needed_surface_subchunks_avoids_full_y_range() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(65, 7),
        )
        .expect("put Data2D");
    storage
        .put(
            &ChunkKey::subchunk(pos, 4).encode(),
            &test_surface_subchunk_bytes(),
        )
        .expect("put subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let needed = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::HintThenVerify,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("needed render chunk");
    let full = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("full render chunk");
    assert!(needed.subchunks.contains_key(&4));
    assert_eq!(needed.subchunks.get(&4), full.subchunks.get(&4));
    assert!(needed.subchunks.len() <= full.subchunks.len());
}

#[test]
fn render_chunk_needed_surface_subchunks_include_lookup_above_heightmap() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(64, 7),
        )
        .expect("put Data2D");
    storage
        .put(
            &ChunkKey::subchunk(pos, 4).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:stone"),
        )
        .expect("put heightmap subchunk");
    storage
        .put(
            &ChunkKey::subchunk(pos, 5).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
        )
        .expect("put upper subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::HintThenVerify,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("needed render chunk");

    assert!(chunk.subchunks.contains_key(&4));
    assert!(chunk.subchunks.contains_key(&5));
    assert!(!chunk.subchunks.contains_key(&9));
    let sample = chunk
        .column_sample_at(0, 0)
        .expect("computed surface sample");
    assert_eq!(sample.surface_y, 95);
    assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
}

#[test]
fn render_chunk_needed_exact_surface_reloads_full_when_window_top_is_touched() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(64, 7),
        )
        .expect("put Data2D");
    storage
        .put(
            &ChunkKey::subchunk(pos, 8).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:stone"),
        )
        .expect("put window-top subchunk");
    storage
        .put(
            &ChunkKey::subchunk(pos, 9).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
        )
        .expect("put hidden upper subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::HintThenVerify,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("needed render chunk");

    assert!(chunk.subchunks.contains_key(&8));
    assert!(chunk.subchunks.contains_key(&9));
    let sample = chunk
        .column_sample_at(0, 0)
        .expect("computed surface sample");
    assert_eq!(sample.surface_y, 159);
    assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
}

#[test]
fn render_chunk_needed_exact_surface_reloads_full_when_raw_height_is_stale() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(0, 7),
        )
        .expect("put stale Data2D");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:stone"),
        )
        .expect("put stale-height subchunk");
    storage
        .put(
            &ChunkKey::subchunk(pos, 4).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:air"),
        )
        .expect("put high empty hint-window subchunk");
    storage
        .put(
            &ChunkKey::subchunk(pos, 10).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
        )
        .expect("put true roof subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::HintThenVerify,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("needed render chunk");

    assert!(chunk.subchunks.contains_key(&10));
    let sample = chunk
        .column_sample_at(0, 0)
        .expect("computed surface sample");
    assert_eq!(sample.surface_y, 175);
    assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
}

#[test]
fn render_chunk_raw_heightmap_request_does_not_build_surface_samples() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(0, 7),
        )
        .expect("put raw height");
    storage
        .put(
            &ChunkKey::subchunk(pos, 10).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
        )
        .expect("put high surface subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new().height_map(),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load raw heightmap chunk");

    assert_eq!(chunk.height_map.as_ref().unwrap()[0][0], Some(0));
    assert!(chunk.column_samples.is_none());
    assert!(chunk.subchunks.is_empty());
}

#[test]
fn render_chunk_needed_surface_subchunks_fall_back_to_full_without_heightmap() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::subchunk(pos, 5).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:oak_leaves"),
        )
        .expect("put upper subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::HintThenVerify,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("needed render chunk");

    assert!(chunk.subchunks.contains_key(&5));
    let sample = chunk
        .column_sample_at(0, 0)
        .expect("computed surface sample");
    assert_eq!(sample.surface_y, 95);
    assert_eq!(sample.surface_block_state.name, "minecraft:oak_leaves");
}

#[test]
fn render_chunk_loads_block_entities_when_requested() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    let block_entity = NbtTag::Compound(IndexMap::from([
        ("id".to_string(), NbtTag::String("Banner".to_string())),
        ("x".to_string(), NbtTag::Int(3)),
        ("y".to_string(), NbtTag::Int(65)),
        ("z".to_string(), NbtTag::Int(4)),
    ]));
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::BlockEntity).encode(),
            &crate::nbt::serialize_root_nbt(&block_entity).expect("serialize block entity"),
        )
        .expect("put block entity");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let without_entities = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load render chunk without block entities");
    let with_entities = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::TopColumns,
                    true,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load render chunk with block entities");

    assert!(without_entities.block_entities.is_empty());
    assert_eq!(with_entities.block_entities.len(), 1);
    assert_eq!(
        with_entities.block_entities[0].id.as_deref(),
        Some("Banner")
    );
    assert_eq!(with_entities.block_entities[0].position, Some([3, 65, 4]));
}

#[test]
fn surface_column_query_returns_top_block_and_water_context() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let storage = Arc::new(MemoryStorage::new());
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(65, 7),
        )
        .expect("put Data2D");
    storage
        .put(
            &ChunkKey::subchunk(pos, 4).encode(),
            &test_surface_subchunk_bytes(),
        )
        .expect("put subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let column = world
        .get_surface_column_blocking(pos, 0, 0, SurfaceColumnOptions::default())
        .expect("surface query")
        .expect("surface column");

    assert_eq!(column.y, 65);
    assert_eq!(column.block_name, "minecraft:water");
    assert_eq!(column.biome_id, Some(7));
    assert_eq!(column.water_depth, 1);
    assert_eq!(
        column.under_water_block_name.as_deref(),
        Some("minecraft:sand")
    );
}

#[test]
fn chunk_bounds_and_nearest_loaded_chunk_use_key_only_scan() {
    let storage = Arc::new(MemoryStorage::new());
    let positions = [
        ChunkPos {
            x: -4,
            z: 3,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 2,
            z: -1,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 9,
            z: 9,
            dimension: Dimension::Nether,
        },
    ];
    for pos in positions {
        storage
            .put(&ChunkKey::new(pos, ChunkRecordTag::Version).encode(), &[1])
            .expect("put chunk version");
    }
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let bounds = world
        .discover_chunk_bounds_blocking(Dimension::Overworld, WorldScanOptions::default())
        .expect("bounds")
        .expect("overworld bounds");
    assert_eq!(bounds.min_chunk_x, -4);
    assert_eq!(bounds.max_chunk_z, 3);
    assert_eq!(bounds.chunk_count, 2);

    let nearest = world
        .nearest_loaded_chunk_to_spawn_blocking(
            Dimension::Overworld,
            0,
            0,
            WorldScanOptions::default(),
        )
        .expect("nearest")
        .expect("nearest chunk");
    assert_eq!(nearest.x, 2);
    assert_eq!(nearest.z, -1);
}

#[test]
#[allow(clippy::similar_names)]
fn render_region_index_uses_key_only_scan_and_parallel_load_keeps_order() {
    let storage = Arc::new(MemoryStorage::new());
    let render_positions = [
        ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Overworld,
        },
    ];
    for pos in render_positions {
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &test_data2d_bytes(64, 3),
            )
            .expect("put render chunk");
    }
    storage
        .put(
            &ChunkKey::new(
                ChunkPos {
                    x: 2,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                ChunkRecordTag::Version,
            )
            .encode(),
            &[1],
        )
        .expect("put non-render chunk");
    storage
        .put(
            &ChunkKey::new(
                ChunkPos {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Nether,
                },
                ChunkRecordTag::Data2D,
            )
            .encode(),
            &test_data2d_bytes(64, 3),
        )
        .expect("put nether chunk");

    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());
    let visible = world
        .list_chunk_positions_in_region_blocking(
            WorldChunkQueryRegion {
                dimension: Dimension::Overworld,
                min_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_x: 2,
                max_chunk_z: 0,
            },
            WorldScanOptions {
                threading: WorldThreadingOptions::Fixed(2),
                ..WorldScanOptions::default()
            },
        )
        .expect("render region index");

    assert_eq!(visible, render_positions.to_vec());

    let chunks = world
        .query_chunk_data_many_blocking(
            visible,
            ChunkLoadOptions {
                threading: WorldThreadingOptions::Fixed(2),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("parallel render chunk load");
    assert_eq!(
        chunks.iter().map(|chunk| chunk.pos).collect::<Vec<_>>(),
        render_positions.to_vec()
    );
}

#[test]
fn legacy_terrain_is_renderable_and_exact_batch_loaded() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
            &test_legacy_terrain_bytes(2, 65),
        )
        .expect("put legacy terrain");
    let world = BedrockWorld::from_storage_with_format(
        "memory",
        storage,
        BedrockWorldOpenOptions::default(),
        WorldFormat::LevelDbLegacyTerrain,
    );

    let positions = world
        .list_chunk_positions_in_region_blocking(
            WorldChunkQueryRegion {
                dimension: Dimension::Overworld,
                min_chunk_x: 0,
                min_chunk_z: 0,
                max_chunk_x: 0,
                max_chunk_z: 0,
            },
            WorldScanOptions::default(),
        )
        .expect("legacy render index");
    assert_eq!(positions, vec![pos]);

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            positions,
            ChunkLoadOptions {
                threading: WorldThreadingOptions::Single,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("legacy exact render load");
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].is_loaded);
    assert!(chunks[0].legacy_terrain.is_some());
    assert_eq!(chunks[0].height_map.as_ref().unwrap()[0][0], Some(65));
    assert!(chunks[0].legacy_biomes.is_some());
    assert!(chunks[0].legacy_biome_colors.is_some());
    assert_eq!(stats.prefix_scans, 0);
    assert_eq!(stats.legacy_terrain_records, 1);
    assert_eq!(stats.legacy_biome_samples, 1);
    assert_eq!(stats.legacy_biome_colors, 1);
    assert_eq!(stats.terrain_source_legacy, 1);
    assert_eq!(stats.detected_format, WorldFormat::LevelDbLegacyTerrain);
}

#[test]
fn legacy_terrain_biome_rgb_takes_priority_over_data2d_biome_id() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let mut terrain = test_legacy_terrain_bytes(2, 65);
    write_legacy_biome_sample(&mut terrain, 0, 0, 12, 0x0034_a853);
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
            &terrain,
        )
        .expect("put legacy terrain");
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(2, 24),
        )
        .expect("put conflicting old data2d");
    let world = BedrockWorld::from_storage_with_format(
        "memory",
        storage,
        BedrockWorldOpenOptions::default(),
        WorldFormat::LevelDbLegacyTerrain,
    );

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            [pos],
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::All,
                    false,
                ),
                threading: WorldThreadingOptions::Single,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load conflicting legacy render chunk");

    let sample = chunks[0]
        .column_sample_at(0, 0)
        .expect("computed column sample");
    assert_eq!(
        sample.biome,
        Some(TerrainColumnBiome::Legacy(LegacyBiomeSample {
            biome_id: 12,
            red: 0x34,
            green: 0xa8,
            blue: 0x53,
        }))
    );
    assert_eq!(stats.legacy_biome_preferred_columns, 256);
    assert_eq!(stats.modern_biome_fallback_columns, 0);
}

#[test]
fn modern_data2d_biome_remains_available_without_legacy_terrain() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(2, 24),
        )
        .expect("put modern data2d");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
        )
        .expect("put surface subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            [pos],
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::All,
                    false,
                ),
                threading: WorldThreadingOptions::Single,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load modern render chunk");

    let sample = chunks[0]
        .column_sample_at(0, 0)
        .expect("computed column sample");
    assert_eq!(sample.biome, Some(TerrainColumnBiome::Id(24)));
    assert_eq!(stats.legacy_biome_preferred_columns, 0);
    assert_eq!(stats.modern_biome_fallback_columns, 0);
}

#[test]
fn legacy_terrain_exposes_biome_colors_without_transposing_columns() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let mut terrain = test_legacy_terrain_bytes(2, 65);
    write_legacy_biome_sample(&mut terrain, 0, 0, 1, 0x0011_2233);
    write_legacy_biome_sample(&mut terrain, 15, 0, 2, 0x0044_5566);
    write_legacy_biome_sample(&mut terrain, 0, 15, 3, 0x0077_8899);
    write_legacy_biome_sample(&mut terrain, 15, 15, 4, 0x00aa_bbcc);
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
            &terrain,
        )
        .expect("put legacy terrain");
    let world = BedrockWorld::from_storage_with_format(
        "memory",
        storage,
        BedrockWorldOpenOptions::default(),
        WorldFormat::LevelDbLegacyTerrain,
    );

    let chunk = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load legacy render chunk");
    let colors = chunk.legacy_biome_colors.expect("legacy biome colors");
    let samples = chunk.legacy_biomes.expect("legacy biome samples");
    assert_eq!(colors[0][0], Some(0x0011_2233));
    assert_eq!(colors[0][15], Some(0x0044_5566));
    assert_eq!(colors[15][0], Some(0x0077_8899));
    assert_eq!(colors[15][15], Some(0x00aa_bbcc));
    assert_eq!(samples[0][0].map(|sample| sample.biome_id), Some(1));
    assert_eq!(samples[0][15].map(|sample| sample.biome_id), Some(2));
    assert_eq!(samples[15][0].map(|sample| sample.biome_id), Some(3));
    assert_eq!(samples[15][15].map(|sample| sample.biome_id), Some(4));
    assert_eq!(
        world
            .get_legacy_biome_color_blocking(pos, 15, 0)
            .expect("legacy biome color"),
        Some(0x0044_5566)
    );
    assert_eq!(
        world
            .get_legacy_biome_sample_blocking(pos, 15, 0)
            .expect("legacy biome sample")
            .map(|sample| (sample.biome_id, sample.rgb_u32())),
        Some((2, 0x0044_5566))
    );
}

#[test]
fn render_load_keeps_subchunks_when_legacy_terrain_is_also_present() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
            &test_legacy_terrain_bytes(1, 1),
        )
        .expect("put legacy terrain");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_surface_subchunk_bytes(),
        )
        .expect("put subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            [pos],
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load mixed render chunk");

    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].legacy_terrain.is_some());
    assert!(chunks[0].subchunks.contains_key(&0));
    assert_eq!(stats.legacy_terrain_records, 1);
    assert_eq!(stats.terrain_source_subchunk, 1);
    assert_eq!(stats.terrain_source_legacy, 0);
}

#[test]
fn exact_surface_column_samples_use_top_block_not_raw_heightmap() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(1, 3),
        )
        .expect("put misleading raw height");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
        )
        .expect("put surface subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            [pos],
            ChunkLoadOptions {
                data_request: exact_surface_request(
                    ExactSurfaceSubchunkPolicy::Full,
                    ExactSurfaceBiomeLoad::TopColumns,
                    false,
                ),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load exact surface chunk");

    let sample = chunks[0]
        .column_sample_at(0, 0)
        .expect("computed column sample");
    assert_eq!(sample.surface_y, 15);
    assert_eq!(sample.surface_block_state.name, "minecraft:grass_block");
    assert_eq!(sample.source, TerrainSampleSource::Subchunk);
    assert_eq!(stats.computed_surface_columns, 256);
    assert_eq!(stats.raw_height_mismatch_columns, 256);
}

#[test]
fn exact_surface_columns_match_full_indices() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
            &test_data2d_bytes(1, 3),
        )
        .expect("put height map");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:grass_block"),
        )
        .expect("put surface subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());
    let surface_request = exact_surface_request(
        ExactSurfaceSubchunkPolicy::Full,
        ExactSurfaceBiomeLoad::TopColumns,
        false,
    );
    let full = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: surface_request.clone().full_3d_indices(),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load full indices");
    let surface = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: surface_request,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load surface columns");

    assert_eq!(full.column_samples, surface.column_samples);
    let samples = world
        .load_surface_columns_blocking(
            pos,
            ChunkLoadOptions::exact_surface_columns(
                ExactSurfaceSubchunkPolicy::Full,
                ExactSurfaceBiomeLoad::TopColumns,
                false,
            ),
        )
        .expect("load surface samples")
        .expect("surface samples");
    assert_eq!(surface.column_samples.as_ref(), Some(&samples));
}

#[test]
fn specialized_render_load_options_select_the_minimal_decode_contract() {
    let surface = ChunkLoadOptions::exact_surface_columns(
        ExactSurfaceSubchunkPolicy::HintThenVerify,
        ExactSurfaceBiomeLoad::TopColumns,
        false,
    );
    assert!(matches!(
        surface.data_request.subchunks.as_slice(),
        [SubchunkDataRequirement::SurfaceColumns(
            ExactSurfaceSubchunkPolicy::HintThenVerify
        )]
    ));
    assert_eq!(
        surface.data_request.preferred_decode_mode(),
        SubChunkDecodeMode::SurfaceColumns
    );

    let layer = ChunkLoadOptions::layer(64);
    assert!(matches!(
        layer.data_request.subchunks.as_slice(),
        [SubchunkDataRequirement::Layer(64)]
    ));
    assert_eq!(
        layer.data_request.preferred_decode_mode(),
        SubChunkDecodeMode::FullIndices
    );

    let height_map = ChunkLoadOptions::raw_height_map();
    assert!(height_map.data_request.height_map);
    assert_eq!(
        height_map.data_request.preferred_decode_mode(),
        SubChunkDecodeMode::CountsOnly
    );
}

#[test]
fn composable_map_data_request_unions_subchunk_reads_and_decoder_needs() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let request = ChunkDataRequest::new().layer(0).cave_slice(31).height_map();
    let options = ChunkLoadOptions::for_data_request(request.clone());
    let planned = planned_render_subchunk_ys(pos, &options, None).expect("plan subchunks");
    assert_eq!(planned.into_iter().collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(
        request.preferred_decode_mode(),
        SubChunkDecodeMode::FullIndices
    );

    let surface = ChunkDataRequest::new()
        .surface_columns(ExactSurfaceSubchunkPolicy::HintThenVerify)
        .biome(BiomeDataRequirement::SurfaceColumns);
    assert_eq!(
        surface.preferred_decode_mode(),
        SubChunkDecodeMode::SurfaceColumns
    );
    assert!(!surface.height_map);
}

#[test]
fn exact_surface_samples_keep_visual_overlay_and_primary_thin_blocks() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_named_subchunk_bytes_with_values(
                &[
                    "minecraft:air",
                    "minecraft:grass_block",
                    "minecraft:stone_button",
                    "minecraft:red_carpet",
                    "minecraft:snow_layer",
                    "minecraft:vine",
                ],
                |local_x, _, local_y| match (local_x, local_y) {
                    (_, 0) => 1,
                    (0, 1) => 2,
                    (1, 1) => 3,
                    (2, 1) => 4,
                    (3, 1) => 5,
                    _ => 0,
                },
            ),
        )
        .expect("put overlay subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let full = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load exact surface chunk");
    let surface = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                subchunk_decode: SubChunkDecodeMode::SurfaceColumns,
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load surface-column chunk");
    assert_eq!(full.column_samples, surface.column_samples);

    let button = surface.column_sample_at(0, 0).expect("button column");
    assert_eq!(button.surface_y, 0);
    assert_eq!(button.surface_block_state.name, "minecraft:grass_block");
    assert_eq!(
        button
            .overlay
            .as_ref()
            .map(|overlay| overlay.block_state.name.as_str()),
        Some("minecraft:stone_button")
    );
    let carpet = surface.column_sample_at(1, 0).expect("carpet column");
    assert_eq!(carpet.surface_y, 1);
    assert_eq!(carpet.surface_block_state.name, "minecraft:red_carpet");
    assert!(carpet.overlay.is_none());
    let snow = surface.column_sample_at(2, 0).expect("snow column");
    assert_eq!(snow.surface_y, 1);
    assert_eq!(snow.surface_block_state.name, "minecraft:snow_layer");
    assert!(snow.overlay.is_none());
    let vine = surface.column_sample_at(3, 0).expect("vine column");
    assert_eq!(vine.surface_y, 0);
    assert_eq!(
        vine.overlay
            .as_ref()
            .map(|overlay| overlay.block_state.name.as_str()),
        Some("minecraft:vine")
    );
}

#[test]
fn exact_surface_samples_high_roof_from_secondary_storage() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(&ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(), &{
            let mut bytes = Vec::with_capacity(768);
            for _ in 0..256 {
                bytes.extend_from_slice(&0_i16.to_le_bytes());
            }
            bytes.extend(std::iter::repeat_n(1_u8, 256));
            bytes
        })
        .expect("put low raw height map");
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_named_subchunk_bytes_with_values(
                &["minecraft:air", "minecraft:stone"],
                |_, _, local_y| u16::from(local_y == 0),
            ),
        )
        .expect("put low ground subchunk");
    storage
        .put(
            &ChunkKey::subchunk(pos, 10).encode(),
            &test_named_layered_subchunk_bytes(
                &["minecraft:air"],
                &["minecraft:air", "minecraft:copper_block"],
                |_, _, _| 0,
                |_, _, local_y| u16::from(local_y == 15),
            ),
        )
        .expect("put high secondary-storage roof");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load exact surface chunk");
    let sample = chunk.column_sample_at(0, 0).expect("roof column");

    assert_eq!(sample.surface_y, 175);
    assert_eq!(sample.surface_block_state.name, "minecraft:copper_block");
    assert_eq!(sample.source, TerrainSampleSource::Subchunk);
    assert_eq!(
        chunk.height_map.as_ref().expect("raw height map")[0][0],
        Some(0)
    );
}

#[test]
fn exact_surface_samples_process_secondary_storage_water_and_overlay() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_named_layered_subchunk_bytes(
                &["minecraft:air", "minecraft:sand", "minecraft:grass_block"],
                &["minecraft:air", "minecraft:water", "minecraft:stone_button"],
                |local_x, _, local_y| match (local_x, local_y) {
                    (0, 0) => 1,
                    (1, 1) => 2,
                    _ => 0,
                },
                |local_x, _, local_y| match (local_x, local_y) {
                    (0, 0) => 1,
                    (1, 1) => 2,
                    _ => 0,
                },
            ),
        )
        .expect("put layered water and overlay");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load exact surface chunk");
    let water = chunk.column_sample_at(0, 0).expect("water column");
    assert_eq!(water.surface_y, 0);
    assert_eq!(water.surface_block_state.name, "minecraft:water");
    assert_eq!(water.relief_y, 0);
    assert_eq!(water.relief_block_state.name, "minecraft:sand");
    assert_eq!(
        water.water.as_ref().and_then(|water| water.underwater_y),
        Some(0)
    );
    let overlay = chunk.column_sample_at(1, 0).expect("overlay column");
    assert_eq!(overlay.surface_y, 1);
    assert_eq!(overlay.surface_block_state.name, "minecraft:grass_block");
    assert_eq!(
        overlay
            .overlay
            .as_ref()
            .map(|overlay| overlay.block_state.name.as_str()),
        Some("minecraft:stone_button")
    );
}

#[test]
fn exact_surface_samples_keep_transparent_water_relief_context() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_named_subchunk_bytes_with_values(
                &["minecraft:air", "minecraft:sand", "minecraft:water"],
                |_, _, local_y| match local_y {
                    0 => 1,
                    1 | 2 => 2,
                    _ => 0,
                },
            ),
        )
        .expect("put water subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(pos, ChunkLoadOptions::default())
        .expect("load exact surface chunk");
    let sample = chunk.column_sample_at(0, 0).expect("water column");
    let water = sample.water.as_ref().expect("water context");
    assert_eq!(sample.surface_y, 2);
    assert_eq!(sample.surface_block_state.name, "minecraft:water");
    assert_eq!(sample.relief_y, 0);
    assert_eq!(sample.relief_block_state.name, "minecraft:sand");
    assert_eq!(water.depth, 2);
    assert_eq!(water.underwater_y, Some(0));
    assert_eq!(
        water
            .underwater_block_state
            .as_ref()
            .map(|state| state.name.as_str()),
        Some("minecraft:sand")
    );
}

#[test]
fn render_chunk_exact_load_preserves_legacy_subchunk_xzy_coordinates() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_asymmetric_legacy_subchunk_bytes(),
        )
        .expect("put legacy subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let chunk = world
        .query_chunk_data_blocking(
            pos,
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new().layer(10),
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load legacy subchunk render chunk");
    let subchunk = chunk.subchunks.get(&0).expect("loaded legacy subchunk");

    assert_eq!(subchunk.legacy_block_id_at(0, 10, 0), Some(1));
    assert_eq!(subchunk.legacy_block_id_at(15, 10, 0), Some(12));
    assert_eq!(subchunk.legacy_block_id_at(0, 10, 15), Some(24));
    assert_eq!(subchunk.legacy_block_id_at(15, 10, 15), Some(45));
}

#[test]
fn layer_query_does_not_read_surface_fallback_records() {
    let storage = Arc::new(MemoryStorage::new());
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    storage
        .put(
            &ChunkKey::subchunk(pos, 0).encode(),
            &test_uniform_named_subchunk_bytes("minecraft:stone"),
        )
        .expect("put layer subchunk");
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            [pos],
            ChunkLoadOptions::for_data_request(ChunkDataRequest::new().layer(0)),
        )
        .expect("query fixed layer");

    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].subchunks.contains_key(&0));
    assert_eq!(stats.keys_requested, 1);
    assert_eq!(stats.keys_found, 1);
    assert_eq!(stats.legacy_terrain_records, 0);
}

#[test]
fn chunk_query_defaults_to_reusing_storage_blocks() {
    assert_eq!(
        ChunkLoadOptions::default().storage_cache_policy,
        StorageCachePolicy::Use
    );
}

#[test]
fn decode_timing_preserves_sub_millisecond_samples() {
    let mut total = ChunkDecodeTiming::default();
    total.add(ChunkDecodeTiming {
        biome_parse_us: 125,
        subchunk_parse_us: 250,
        surface_scan_us: 375,
        block_entity_parse_us: 500,
    });
    total.add(ChunkDecodeTiming {
        biome_parse_us: 875,
        subchunk_parse_us: 750,
        surface_scan_us: 625,
        block_entity_parse_us: 500,
    });

    assert_eq!(total.biome_parse_us, 1_000);
    assert_eq!(total.subchunk_parse_us, 1_000);
    assert_eq!(total.surface_scan_us, 1_000);
    assert_eq!(total.block_entity_parse_us, 1_000);
}

#[test]
#[allow(clippy::similar_names)]
fn render_chunk_exact_batch_keeps_shuffled_positions_bound_to_records() {
    let storage = Arc::new(MemoryStorage::new());
    let fixtures = [
        (
            ChunkPos {
                x: -3,
                z: 1,
                dimension: Dimension::Overworld,
            },
            "minecraft:signature_a",
        ),
        (
            ChunkPos {
                x: 2,
                z: -4,
                dimension: Dimension::Overworld,
            },
            "minecraft:signature_b",
        ),
        (
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            "minecraft:signature_c",
        ),
    ];
    for (pos, block_name) in fixtures.iter().copied() {
        storage
            .put(
                &ChunkKey::subchunk(pos, 4).encode(),
                &test_uniform_named_subchunk_bytes(block_name),
            )
            .expect("put named subchunk");
    }
    let world = BedrockWorld::from_storage("memory", storage, BedrockWorldOpenOptions::default());

    let (chunks, stats) = world
        .query_chunk_data_with_stats_blocking(
            vec![fixtures[1].0, fixtures[0].0, fixtures[2].0, fixtures[1].0],
            ChunkLoadOptions {
                data_request: ChunkDataRequest::new().layer(64),
                threading: WorldThreadingOptions::Fixed(4),
                priority: ChunkLoadPriority::DistanceFrom {
                    chunk_x: 0,
                    chunk_z: 0,
                },
                ..ChunkLoadOptions::default()
            },
        )
        .expect("load shuffled render chunks");

    assert_eq!(chunks.len(), 4);
    assert_eq!(stats.prefix_scans, 0);
    assert!(stats.exact_get_batches > 0);
    for chunk in chunks {
        let expected = fixtures
            .iter()
            .find_map(|(pos, block_name)| (*pos == chunk.pos).then_some(*block_name))
            .expect("known chunk position");
        let subchunk = chunk.subchunks.get(&4).expect("loaded subchunk");
        let state = subchunk
            .block_state_at(0, 0, 0)
            .expect("decoded signature block");
        assert_eq!(state.name, expected, "chunk {:?}", chunk.pos);
    }
}

fn test_surface_subchunk_bytes() -> Vec<u8> {
    let palette = ["minecraft:air", "minecraft:sand", "minecraft:water"];
    let mut bytes = vec![8, 1, 2 << 1];
    let values_per_word = 16_usize;
    let mut words = vec![0_u32; 256];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for (local_y, value) in [(0_u8, 1_u32), (1, 2)] {
                let block_index = block_storage_index(local_x, local_y, local_z);
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * 2;
                words[word_index] |= value << bit_offset;
            }
        }
    }
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
    for name in palette {
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(name.to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
    }
    bytes
}

fn test_uniform_named_subchunk_bytes(block_name: &str) -> Vec<u8> {
    let palette = ["minecraft:air", block_name];
    let mut bytes = vec![8, 1, 1 << 1];
    let mut words = vec![0_u32; 128];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for local_y in 0..16_u8 {
                let block_index = block_storage_index(local_x, local_y, local_z);
                let word_index = block_index / 32;
                let bit_offset = block_index % 32;
                words[word_index] |= 1_u32 << bit_offset;
            }
        }
    }
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
    for name in palette {
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String(name.to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
    }
    bytes
}

fn test_named_subchunk_bytes_with_values(
    palette: &[&str],
    value_at: impl Fn(u8, u8, u8) -> u16,
) -> Vec<u8> {
    let bits_per_value = match palette.len() {
        0..=2 => 1_u8,
        3..=4 => 2_u8,
        5..=16 => 4_u8,
        _ => 8_u8,
    };
    let values_per_word = usize::from(32 / bits_per_value);
    let word_count = 4096_usize.div_ceil(values_per_word);
    let mut bytes = vec![8, 1, bits_per_value << 1];
    let mut words = vec![0_u32; word_count];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for local_y in 0..16_u8 {
                let value = value_at(local_x, local_z, local_y);
                if value == 0 {
                    continue;
                }
                let block_index = block_storage_index(local_x, local_y, local_z);
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * usize::from(bits_per_value);
                words[word_index] |= u32::from(value) << bit_offset;
            }
        }
    }
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
    for name in palette {
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String((*name).to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
    }
    bytes
}

fn test_named_layered_subchunk_bytes(
    lower_palette: &[&str],
    upper_palette: &[&str],
    lower_value_at: impl Fn(u8, u8, u8) -> u16,
    upper_value_at: impl Fn(u8, u8, u8) -> u16,
) -> Vec<u8> {
    let mut bytes = vec![8, 2];
    append_named_palette_storage(&mut bytes, lower_palette, lower_value_at);
    append_named_palette_storage(&mut bytes, upper_palette, upper_value_at);
    bytes
}

fn append_named_palette_storage(
    bytes: &mut Vec<u8>,
    palette: &[&str],
    value_at: impl Fn(u8, u8, u8) -> u16,
) {
    let bits_per_value = match palette.len() {
        0..=2 => 1_u8,
        3..=4 => 2_u8,
        5..=16 => 4_u8,
        _ => 8_u8,
    };
    let values_per_word = usize::from(32 / bits_per_value);
    let word_count = 4096_usize.div_ceil(values_per_word);
    let mut words = vec![0_u32; word_count];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for local_y in 0..16_u8 {
                let value = value_at(local_x, local_z, local_y);
                if value == 0 {
                    continue;
                }
                let block_index = block_storage_index(local_x, local_y, local_z);
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * usize::from(bits_per_value);
                words[word_index] |= u32::from(value) << bit_offset;
            }
        }
    }
    bytes.push(bits_per_value << 1);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(&(palette.len() as i32).to_le_bytes());
    for name in palette {
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String((*name).to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&crate::nbt::serialize_root_nbt(&tag).expect("nbt"));
    }
}

fn test_asymmetric_legacy_subchunk_bytes() -> Vec<u8> {
    let mut bytes = vec![0_u8; LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
    bytes[0] = 2;
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let block_id = match (local_x >= 8, local_z >= 8) {
                (false, false) => 1,
                (true, false) => 12,
                (false, true) => 24,
                (true, true) => 45,
            };
            let index =
                LegacySubChunk::block_index(local_x, 10, local_z).expect("legacy subchunk index");
            bytes[1 + index] = block_id;
        }
    }
    bytes
}

fn test_data2d_bytes(height: i16, biome: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(768);
    for _ in 0..256 {
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    bytes.extend(std::iter::repeat_n(biome, 256));
    bytes
}

fn test_data3d_height_bytes(height: i16) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    for _ in 0..256 {
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    bytes
}

fn test_asymmetric_data2d_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(768);
    for local_z in 0..16_i16 {
        for local_x in 0..16_i16 {
            let height = 100 + local_x * 10 + local_z;
            bytes.extend_from_slice(&height.to_le_bytes());
        }
    }
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            bytes.push(local_x * 10 + local_z);
        }
    }
    bytes
}

fn test_legacy_terrain_bytes(block_id: u8, height: u8) -> Vec<u8> {
    let mut bytes = vec![0_u8; LEGACY_TERRAIN_VALUE_LEN];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for local_y in 0..=height.min(127) {
                let index = LegacyTerrain::block_index(local_x, local_y, local_z)
                    .expect("legacy block index");
                bytes[index] = block_id;
            }
            bytes[LEGACY_TERRAIN_BLOCK_COUNT
                + LEGACY_TERRAIN_BLOCK_COUNT / 2 * 3
                + raw_2d_column_index(local_x, local_z)] = height;
        }
    }
    bytes
}

fn write_legacy_biome_sample(bytes: &mut [u8], local_x: u8, local_z: u8, biome_id: u8, color: u32) {
    let offset = LEGACY_TERRAIN_BLOCK_COUNT
        + LEGACY_TERRAIN_BLOCK_COUNT / 2 * 3
        + 16 * 16
        + raw_2d_column_index(local_x, local_z) * 4;
    bytes[offset] = biome_id;
    bytes[offset + 1] = ((color >> 16) & 0xff) as u8;
    bytes[offset + 2] = ((color >> 8) & 0xff) as u8;
    bytes[offset + 3] = (color & 0xff) as u8;
}

fn raw_2d_column_index(local_x: u8, local_z: u8) -> usize {
    usize::from(local_z) * 16 + usize::from(local_x)
}

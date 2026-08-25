use super::legacy::{
    LEGACY_TERRAIN_BIOME_OFFSET, LEGACY_TERRAIN_BLOCK_DATA_OFFSET,
    LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET, LEGACY_TERRAIN_HEIGHTMAP_OFFSET,
    LEGACY_TERRAIN_SKY_LIGHT_OFFSET,
};
use super::subchunk::packed_word_count;
use super::*;
use crate::nbt::{NbtTag, serialize_root_nbt};
use bytes::Bytes;
use indexmap::IndexMap;

#[test]
fn chunk_key_roundtrips_overworld_and_subchunk() {
    let pos = ChunkPos {
        x: -3,
        z: 7,
        dimension: Dimension::Overworld,
    };
    let key = ChunkKey::subchunk(pos, -4);
    let encoded = key.encode();
    assert_eq!(encoded.len(), 10);
    assert_eq!(ChunkKey::decode(&encoded).expect("decode"), key);
    assert_eq!(key.encode_inline().as_bytes(), encoded.as_ref());
    assert_eq!(
        BedrockDbKeyKind::classify(&encoded),
        BedrockDbKeyKind::Chunk(ChunkRecordTag::SubChunkPrefix)
    );
}

#[test]
fn key_kind_classification_handles_common_non_chunk_keys_without_decoding() {
    assert_eq!(
        BedrockDbKeyKind::classify(b"~local_player"),
        BedrockDbKeyKind::LocalPlayer
    );
    assert_eq!(
        BedrockDbKeyKind::classify(b"player_123"),
        BedrockDbKeyKind::RemotePlayer
    );
    assert_eq!(
        BedrockDbKeyKind::classify(b"actorprefix12345678"),
        BedrockDbKeyKind::ActorPrefix
    );
    assert_eq!(
        BedrockDbKeyKind::classify(b"unclassified"),
        BedrockDbKeyKind::Other
    );
}

#[test]
fn chunk_key_roundtrips_dimension_key() {
    let pos = ChunkPos {
        x: 1,
        z: 2,
        dimension: Dimension::Nether,
    };
    let key = ChunkKey::new(pos, ChunkRecordTag::Version);
    let encoded = key.encode();
    assert_eq!(encoded.len(), 13);
    assert_eq!(ChunkKey::decode(&encoded).expect("decode"), key);
}

#[test]
fn bedrock_db_key_decodes_actor_and_digp_keys() {
    let mut actor_key = b"actorprefix".to_vec();
    actor_key.extend_from_slice(&42_i64.to_le_bytes());
    assert_eq!(
        BedrockDbKey::decode(&actor_key),
        BedrockDbKey::ActorPrefix { actor_id: 42 }
    );

    let mut digp_key = b"digp".to_vec();
    digp_key.extend_from_slice(&1_i32.to_le_bytes());
    digp_key.extend_from_slice(&(-2_i32).to_le_bytes());
    assert_eq!(
        BedrockDbKey::decode(&digp_key),
        BedrockDbKey::ActorDigest {
            pos: ChunkPos {
                x: 1,
                z: -2,
                dimension: Dimension::Overworld
            }
        }
    );
}

#[test]
fn bedrock_db_key_encodes_documented_global_shapes() {
    let map_id = MapRecordId::new("42").expect("map id");
    assert_eq!(map_id.storage_key().as_ref(), b"map_42");
    assert_eq!(
        MapRecordId::from_storage_key(b"map_42"),
        Some(map_id.clone())
    );
    assert_eq!(
        BedrockDbKey::Map("42".to_string()).encode().as_deref(),
        Some(&b"map_42"[..])
    );

    let pos = ChunkPos {
        x: 7,
        z: -8,
        dimension: Dimension::End,
    };
    let digest = ActorDigestKey::new(pos).storage_key();
    assert_eq!(
        ActorDigestKey::from_storage_key(&digest),
        Some(ActorDigestKey::new(pos))
    );
    assert_eq!(
        BedrockDbKey::Global(GlobalRecordKind::Scoreboard)
            .encode()
            .as_deref(),
        Some(&b"scoreboard"[..])
    );
    assert_eq!(
        BedrockDbKey::decode(b"TheEnd"),
        BedrockDbKey::Global(GlobalRecordKind::Dimension(Dimension::End))
    );
}

#[test]
fn chunk_record_tags_align_with_bedrock_level_reference() {
    let expected = [
        (0x2b, ChunkRecordTag::Data3D),
        (0x2c, ChunkRecordTag::Version),
        (0x2d, ChunkRecordTag::Data2D),
        (0x2e, ChunkRecordTag::Data2DLegacy),
        (0x2f, ChunkRecordTag::SubChunkPrefix),
        (0x30, ChunkRecordTag::LegacyTerrain),
        (0x31, ChunkRecordTag::BlockEntity),
        (0x32, ChunkRecordTag::Entity),
        (0x33, ChunkRecordTag::PendingTicks),
        (0x34, ChunkRecordTag::BlockExtraData),
        (0x35, ChunkRecordTag::BiomeState),
        (0x36, ChunkRecordTag::FinalizedState),
        (0x37, ChunkRecordTag::ConversionData),
        (0x38, ChunkRecordTag::BorderBlocks),
        (0x39, ChunkRecordTag::HardcodedSpawners),
        (0x3a, ChunkRecordTag::RandomTicks),
        (0x3b, ChunkRecordTag::Checksums),
        (0x3c, ChunkRecordTag::GenerationSeed),
        (0x3d, ChunkRecordTag::GeneratedPreCavesAndCliffsBlending),
        (0x3e, ChunkRecordTag::BlendingBiomeHeight),
        (0x3f, ChunkRecordTag::MetaDataHash),
        (0x40, ChunkRecordTag::BlendingData),
        (0x41, ChunkRecordTag::ActorDigestVersion),
        (0x76, ChunkRecordTag::VersionOld),
    ];
    for (byte, tag) in expected {
        assert_eq!(ChunkRecordTag::from_byte(byte), tag);
        assert_eq!(tag.byte(), byte);
    }
}

#[test]
fn bedrock_db_key_decodes_specific_ascii_keys_before_plain_keys() {
    assert_eq!(
        BedrockDbKey::decode(b"map_42"),
        BedrockDbKey::Map("42".to_string())
    );
    assert!(matches!(
        BedrockDbKey::decode(b"VILLAGE_12345678-1234-1234-1234-123456789abc_INFO"),
        BedrockDbKey::Village(_)
    ));
    assert!(matches!(
        BedrockDbKey::decode(b"LevelChunkMetaDataDictionary"),
        BedrockDbKey::Global(GlobalRecordKind::LevelChunkMetaDataDictionary)
    ));
}

#[test]
fn chunk_pos_matches_bedrock_level_height_ranges() {
    let overworld = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    assert_eq!(overworld.y_range(ChunkVersion::Old), (0, 255));
    assert_eq!(overworld.y_range(ChunkVersion::New), (-64, 319));
    assert_eq!(overworld.subchunk_index_range(ChunkVersion::New), (-4, 19));
    assert_eq!(
        BlockPos {
            x: -1,
            y: 64,
            z: -1
        }
        .to_chunk_pos(Dimension::Overworld),
        ChunkPos {
            x: -1,
            z: -1,
            dimension: Dimension::Overworld
        }
    );
}

#[test]
fn legacy_terrain_exposes_old_leveldb_arrays() {
    let mut bytes = vec![0; LEGACY_TERRAIN_VALUE_LEN];
    let block_index = LegacyTerrain::block_index(1, 2, 3).expect("block index");
    let column_index = 3 * 16 + 1;
    assert_eq!(block_index, 2_434);
    assert_eq!(LegacyTerrain::column_index(1, 3), Some(column_index));
    bytes[block_index] = 42;
    bytes[LEGACY_TERRAIN_BLOCK_DATA_OFFSET + block_index / 2] = 0xba;
    bytes[LEGACY_TERRAIN_SKY_LIGHT_OFFSET + block_index / 2] = 0xc7;
    bytes[LEGACY_TERRAIN_BLOCK_LIGHT_OFFSET + block_index / 2] = 0xd5;
    bytes[LEGACY_TERRAIN_HEIGHTMAP_OFFSET + column_index] = 99;
    bytes[LEGACY_TERRAIN_BIOME_OFFSET + column_index * 4
        ..LEGACY_TERRAIN_BIOME_OFFSET + column_index * 4 + 4]
        .copy_from_slice(&[12, 0xab, 0xcd, 0xef]);
    let terrain = LegacyTerrain::parse(Bytes::from(bytes)).expect("legacy terrain");
    assert_eq!(terrain.block_id_at(1, 2, 3), Some(42));
    assert_eq!(terrain.block_data_at(1, 2, 3), Some(0x0a));
    assert_eq!(terrain.sky_light_at(1, 2, 3), Some(0x07));
    assert_eq!(terrain.block_light_at(1, 2, 3), Some(0x05));
    assert_eq!(terrain.height_at(1, 3), Some(99));
    assert_eq!(terrain.biome_color_at(1, 3), Some(0x00ab_cdef));
    assert_eq!(
        terrain.biome_sample_at(1, 3),
        Some(LegacyBiomeSample {
            biome_id: 12,
            red: 0xab,
            green: 0xcd,
            blue: 0xef
        })
    );
    assert!(LegacyTerrain::parse(Bytes::from_static(b"short")).is_err());
}

#[test]
fn legacy_subchunk_decodes_block_ids_metadata_and_light() {
    let mut bytes = vec![0; LEGACY_SUBCHUNK_WITH_LIGHT_VALUE_LEN];
    bytes[0] = 2;
    let index = LegacySubChunk::block_index(4, 5, 6).expect("block index");
    assert_eq!(index, 1_125);
    bytes[1 + index] = 7;
    bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + index / 2] = 0xc0;
    bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT / 2 + index / 2] = 0xe0;
    bytes[1 + LEGACY_SUBCHUNK_BLOCK_COUNT + LEGACY_SUBCHUNK_BLOCK_COUNT + index / 2] = 0xa0;
    let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse legacy subchunk");
    let SubChunkFormat::LegacySubChunk(legacy) = &subchunk.format else {
        panic!("expected legacy subchunk");
    };
    assert_eq!(legacy.version(), 2);
    assert_eq!(legacy.block_id_at(4, 5, 6), Some(7));
    assert_eq!(legacy.block_data_at(4, 5, 6), Some(0x0c));
    assert_eq!(legacy.sky_light_at(4, 5, 6), Some(0x0e));
    assert_eq!(legacy.block_light_at(4, 5, 6), Some(0x0a));
    assert_eq!(subchunk.legacy_block_id_at(4, 5, 6), Some(7));
}

#[test]
fn paletted_subchunk_v1_uses_single_storage_without_count_byte() {
    let mut bytes = build_paletted_subchunk(8, None, 4, 4);
    bytes.remove(1);
    bytes[0] = 1;
    let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse v1 palette");
    let SubChunkFormat::Paletted { version, storages } = subchunk.format else {
        panic!("expected v1 paletted subchunk");
    };
    assert_eq!(version, 1);
    assert_eq!(storages.len(), 1);
    assert_eq!(storages[0].indices.as_ref().expect("indices").len(), 4096);
}

#[test]
fn paletted_subchunk_decodes_supported_bits_per_block() {
    for bits_per_block in [0, 1, 2, 3, 4, 5, 6, 8, 16] {
        let bytes = build_paletted_subchunk(8, None, bits_per_block, 4);
        let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse");
        let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
            panic!("expected paletted subchunk for {bits_per_block} bits");
        };
        assert_eq!(storages.len(), 1);
        assert_eq!(storages[0].indices.as_ref().expect("indices").len(), 4096);
        assert_eq!(
            storages[0]
                .counts
                .as_ref()
                .expect("counts")
                .iter()
                .sum::<u16>(),
            4096
        );
    }
}

#[test]
fn paletted_subchunk_counts_only_drops_indices_but_keeps_counts() {
    let bytes = build_paletted_subchunk(8, None, 4, 4);
    let subchunk = parse_subchunk_with_mode(0, Bytes::from(bytes), SubChunkDecodeMode::CountsOnly)
        .expect("parse");
    let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
        panic!("expected paletted subchunk");
    };
    assert!(storages[0].indices.is_none());
    assert_eq!(
        storages[0]
            .counts
            .as_ref()
            .expect("counts-only retains counts")
            .iter()
            .sum::<u16>(),
        4096
    );
}

#[test]
fn surface_columns_keep_random_access_without_full_indices() {
    let bytes = build_paletted_subchunk(8, None, 4, 4);
    let full = parse_subchunk_with_mode(
        0,
        Bytes::from(bytes.clone()),
        SubChunkDecodeMode::FullIndices,
    )
    .expect("parse full indices");
    let surface =
        parse_subchunk_with_mode(0, Bytes::from(bytes), SubChunkDecodeMode::SurfaceColumns)
            .expect("parse surface columns");
    let SubChunkFormat::Paletted {
        storages: surface_storages,
        ..
    } = &surface.format
    else {
        panic!("expected surface paletted subchunk");
    };
    assert!(surface_storages[0].indices.is_none());
    assert!(surface_storages[0].packed_indices.is_some());
    assert!(surface_storages[0].counts.is_none());
    for (x, y, z) in [(0, 0, 0), (1, 2, 3), (15, 15, 15), (7, 9, 4)] {
        assert_eq!(
            full.block_state_at(x, y, z),
            surface.block_state_at(x, y, z)
        );
    }
}

#[test]
fn paletted_subchunk_v9_accepts_embedded_y_byte() {
    let bytes = build_paletted_subchunk(9, Some(-4), 4, 4);
    let subchunk = parse_subchunk(-4, Bytes::from(bytes)).expect("parse");
    let SubChunkFormat::Paletted { storages, .. } = subchunk.format else {
        panic!("expected paletted v9 subchunk");
    };
    assert_eq!(storages[0].states.len(), 4);
}

#[test]
fn paletted_subchunk_v9_accepts_positive_embedded_y_that_looks_like_storage_header() {
    let bytes = build_paletted_subchunk(9, Some(8), 4, 4);
    let subchunk = parse_subchunk(8, Bytes::from(bytes)).expect("parse");
    let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
        panic!("expected paletted v9 subchunk");
    };
    assert_eq!(storages[0].states.len(), 4);
    assert_eq!(
        subchunk.block_state_at(1, 2, 3).expect("block state").name,
        "minecraft:block_2"
    );
}

#[test]
fn paletted_subchunk_v9_falls_back_to_legacy_layout_without_embedded_y() {
    let bytes = build_paletted_subchunk(9, None, 4, 4);
    let subchunk = parse_subchunk(8, Bytes::from(bytes)).expect("parse");
    let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
        panic!("expected paletted v9 subchunk");
    };
    assert_eq!(storages[0].states.len(), 4);
    assert_eq!(
        subchunk.block_state_at(1, 2, 3).expect("block state").name,
        "minecraft:block_2"
    );
}

#[test]
fn paletted_subchunk_rejects_trailing_bytes_after_storage_payload() {
    let mut bytes = build_paletted_subchunk(8, None, 4, 4);
    bytes.push(0);
    assert!(parse_subchunk(0, Bytes::from(bytes)).is_err());
}

#[test]
fn malformed_known_subchunk_versions_are_errors_not_raw_records() {
    for version in [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
        assert!(
            parse_subchunk(0, Bytes::from(vec![version])).is_err(),
            "SubChunk V{version}"
        );
    }

    let unknown = parse_subchunk(0, Bytes::from_static(&[10, 1, 0])).expect("unknown raw");
    assert!(matches!(unknown.format, SubChunkFormat::Raw { .. }));
}

#[test]
fn block_state_lookup_uses_xz_plane_storage_order() {
    let bytes = build_paletted_subchunk(8, None, 4, 8);
    let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse");
    assert_eq!(block_storage_index(1, 2, 3), 306);
    let state = subchunk.block_state_at(1, 2, 3).expect("block state");
    assert_eq!(
        state.name,
        format!("minecraft:block_{}", block_storage_index(1, 2, 3) % 8)
    );
}

#[test]
fn visible_block_state_lookup_uses_top_non_air_storage() {
    let subchunk = parse_subchunk(
        0,
        Bytes::from(build_two_storage_paletted_subchunk(
            "minecraft:stone",
            "minecraft:copper_block",
        )),
    )
    .expect("parse layered subchunk");
    assert_eq!(
        subchunk
            .block_state_at(1, 2, 3)
            .expect("storage zero state")
            .name,
        "minecraft:stone"
    );
    let visible = subchunk
        .visible_block_states_at(1, 2, 3)
        .map(|state| state.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(visible, ["minecraft:copper_block", "minecraft:stone"]);
    assert_eq!(
        subchunk
            .visible_block_state_at(1, 2, 3)
            .expect("visible state")
            .name,
        "minecraft:copper_block"
    );
}

#[test]
fn visible_surface_state_iterator_reports_palette_positions() {
    let mut bytes = vec![8, 3];
    append_test_palette_storage(
        &mut bytes,
        &["minecraft:air", "minecraft:water"],
        |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
    );
    append_test_palette_storage(
        &mut bytes,
        &["minecraft:air", "minecraft:short_grass"],
        |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
    );
    append_test_palette_storage(
        &mut bytes,
        &["minecraft:air", "minecraft:stone"],
        |x, y, z| u16::from((x, y, z) == (1, 2, 3)),
    );
    let subchunk = parse_subchunk(0, Bytes::from(bytes)).expect("parse layered subchunk");
    let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
        panic!("expected paletted subchunk");
    };
    assert_eq!(storages.len(), 3);
    let visible_entries = subchunk
        .visible_block_surface_states_at(1, 2, 3)
        .map(|entry| (entry.storage_index, entry.state.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        visible_entries,
        [
            (2, "minecraft:stone"),
            (1, "minecraft:short_grass"),
            (0, "minecraft:water")
        ]
    );
}

#[test]
fn paletted_subchunk_v9_decodes_zero_bit_secondary_storage_without_palette_len() {
    let mut bytes = vec![9, 2, 4];
    append_test_palette_storage(
        &mut bytes,
        &["minecraft:air", "minecraft:stone"],
        |x, y, z| u16::from((x, y, z) == (4, 2, 4)),
    );
    append_zero_bit_palette_storage(&mut bytes, "minecraft:gold_block");
    let subchunk = parse_subchunk(4, Bytes::from(bytes)).expect("parse v9 layered subchunk");
    let SubChunkFormat::Paletted { storages, .. } = &subchunk.format else {
        panic!("expected paletted subchunk");
    };
    assert_eq!(storages.len(), 2);
    assert_eq!(storages[1].states.len(), 1);
    assert_eq!(storages[1].counts.as_deref(), Some(&[4096][..]));
    assert_eq!(
        subchunk
            .block_state_at(4, 2, 4)
            .expect("storage zero state")
            .name,
        "minecraft:stone"
    );
    assert_eq!(
        subchunk
            .visible_block_state_at(4, 2, 4)
            .expect("visible state")
            .name,
        "minecraft:gold_block"
    );
}

#[test]
fn chunk_get_block_reads_decoded_paletted_subchunk() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let key = ChunkKey::subchunk(pos, 0);
    let chunk = Chunk {
        pos,
        version: Some(8),
        records: vec![ChunkRecord {
            key,
            value: Bytes::from(build_paletted_subchunk(8, None, 4, 8)),
        }],
    };
    let state = chunk.get_block(1, 2, 3).expect("block state");
    assert_eq!(state.name, "minecraft:block_2");
}

#[test]
fn unique_id_is_not_used_as_raw_actorprefix_suffix() {
    let unique_id = 0x0000_0002_1234_5678_i64;
    let storage = ActorUid::from_unique_id(unique_id);
    let expected_numeric = (u64::from(0_u32.wrapping_sub(2)) << 32) | 0x1234_5678;
    assert_eq!(storage.raw_storage_bytes(), expected_numeric.to_be_bytes());
    assert_ne!(storage.raw_storage_bytes(), unique_id.to_le_bytes());
}

#[test]
fn actor_uid_matches_real_bedrock_actorprefix_vectors() {
    let vectors = [
        (
            -206_158_405_104_i64,
            [0x00, 0x00, 0x00, 0x30, 0x00, 0x00, 0x62, 0x10],
        ),
        (
            -214_747_652_446_i64,
            [0x00, 0x00, 0x00, 0x32, 0x00, 0x0a, 0xde, 0xa2],
        ),
    ];

    for (unique_id, actorprefix_suffix) in vectors {
        assert_eq!(
            ActorUid::from_unique_id(unique_id).raw_storage_bytes(),
            actorprefix_suffix
        );
    }
}

fn build_paletted_subchunk(
    version: u8,
    embedded_y: Option<i8>,
    bits_per_block: u8,
    palette_len: usize,
) -> Vec<u8> {
    let palette_len = if bits_per_block == 0 { 1 } else { palette_len };
    let mut bytes = vec![version, 1];
    if let Some(y) = embedded_y {
        bytes.push(y as u8);
    }
    bytes.push(bits_per_block << 1);
    let values_per_word = 32_usize
        .checked_div(usize::from(bits_per_block))
        .unwrap_or(4096);
    let mut words = vec![0_u32; packed_word_count(bits_per_block)];
    if bits_per_block != 0 {
        for block_index in 0..4096 {
            let value = u32::try_from(block_index % palette_len).expect("palette index");
            let word_index = block_index / values_per_word;
            let bit_offset = (block_index % values_per_word) * usize::from(bits_per_block);
            words[word_index] |= value << bit_offset;
        }
    }
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    if bits_per_block != 0 {
        bytes.extend_from_slice(
            &i32::try_from(palette_len)
                .expect("palette length")
                .to_le_bytes(),
        );
    }
    for index in 0..palette_len {
        let tag = NbtTag::Compound(IndexMap::from([
            (
                "name".to_string(),
                NbtTag::String(format!("minecraft:block_{index}")),
            ),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
    }
    bytes
}

fn append_zero_bit_palette_storage(bytes: &mut Vec<u8>, name: &str) {
    bytes.push(0);
    let tag = NbtTag::Compound(IndexMap::from([
        ("name".to_string(), NbtTag::String(name.to_string())),
        ("states".to_string(), NbtTag::Compound(IndexMap::new())),
        ("version".to_string(), NbtTag::Int(1)),
    ]));
    bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
}

fn build_two_storage_paletted_subchunk(lower_name: &str, upper_name: &str) -> Vec<u8> {
    let mut bytes = vec![8, 2];
    append_test_palette_storage(&mut bytes, &["minecraft:air", lower_name], |x, y, z| {
        u16::from((x, y, z) == (1, 2, 3))
    });
    append_test_palette_storage(&mut bytes, &["minecraft:air", upper_name], |x, y, z| {
        u16::from((x, y, z) == (1, 2, 3))
    });
    bytes
}

fn append_test_palette_storage(
    bytes: &mut Vec<u8>,
    palette: &[&str],
    value_at: impl Fn(u8, u8, u8) -> u16,
) {
    let bits_per_block = 1_u8;
    let values_per_word = usize::from(32 / bits_per_block);
    let mut words = vec![0_u32; packed_word_count(bits_per_block)];
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            for local_y in 0..16_u8 {
                let value = value_at(local_x, local_y, local_z);
                if value == 0 {
                    continue;
                }
                let block_index = block_storage_index(local_x, local_y, local_z);
                let word_index = block_index / values_per_word;
                let bit_offset = (block_index % values_per_word) * usize::from(bits_per_block);
                words[word_index] |= u32::from(value) << bit_offset;
            }
        }
    }
    bytes.push(bits_per_block << 1);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(
        &i32::try_from(palette.len())
            .expect("test palette length")
            .to_le_bytes(),
    );
    for name in palette {
        let tag = NbtTag::Compound(IndexMap::from([
            ("name".to_string(), NbtTag::String((*name).to_string())),
            ("states".to_string(), NbtTag::Compound(IndexMap::new())),
            ("version".to_string(), NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(&serialize_root_nbt(&tag).expect("serialize palette"));
    }
}

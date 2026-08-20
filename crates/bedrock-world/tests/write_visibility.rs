#![cfg(feature = "bedrock-leveldb")]

use bedrock_world::{
    BedrockWorld, BedrockWorldCreateOptions, ChunkKey, ChunkPos, ChunkRecordTag, Dimension,
};
use bytes::Bytes;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_world_path() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "bedrock-world-write-visibility-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn committed_chunk_anchor_is_immediately_visible_without_flush() {
    let path = temporary_world_path();
    let world = BedrockWorld::create_blocking(&path, BedrockWorldCreateOptions::new("visibility", 7))
        .expect("create world");
    let pos = ChunkPos {
        x: 37,
        z: -29,
        dimension: Dimension::Overworld,
    };
    assert!(!world.chunk_exists_blocking(pos).expect("initial presence"));

    let mut transaction = world.transaction();
    transaction.put_raw_key(
        ChunkKey::new(pos, ChunkRecordTag::Version).encode(),
        Bytes::from_static(&[40]),
    );
    transaction.commit().expect("commit chunk anchor");

    // No flush/compact/sleep is allowed between commit and these reads. The LevelDB WAL overlay is
    // part of the current database view and public BedrockWorld exact reads must observe it now.
    assert!(world.chunk_exists_blocking(pos).expect("presence after commit"));
    let chunk = world.get_chunk_blocking(pos).expect("read committed chunk");
    assert!(chunk.records.iter().any(|record| {
        record.key.tag == ChunkRecordTag::Version && record.value.as_ref() == [40]
    }));

    drop(world);
    fs::remove_dir_all(&path).expect("remove temporary world");
}

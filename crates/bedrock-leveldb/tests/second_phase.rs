use bedrock_leveldb::{
    Db, LevelDbOpenOptions, NativeCacheOptions, ReadOptions, Result, WriteOptions,
};
use bytes::Bytes;

#[test]
fn duplicate_batch_keys_share_payload_without_losing_input_order() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = Db::open(
        dir.path(),
        LevelDbOpenOptions {
            cache: NativeCacheOptions {
                data_capacity: 1024 * 1024,
                index_capacity: 1024 * 1024,
                file_capacity: 8,
                shards: 4,
            },
            ..LevelDbOpenOptions::default()
        },
    )?;
    db.put(
        Bytes::from_static(b"alpha"),
        Bytes::from_static(b"one"),
        WriteOptions::default(),
    )?;
    db.put(
        Bytes::from_static(b"beta"),
        Bytes::from_static(b"two"),
        WriteOptions::default(),
    )?;
    db.flush()?;

    let keys = vec![
        Bytes::from_static(b"beta"),
        Bytes::from_static(b"alpha"),
        Bytes::from_static(b"alpha"),
        Bytes::from_static(b"missing"),
        Bytes::from_static(b"beta"),
    ];
    let first = db.get_many_owned(keys.clone(), ReadOptions::default())?;
    assert_eq!(first[0].as_deref(), Some(b"two".as_slice()));
    assert_eq!(first[1].as_deref(), Some(b"one".as_slice()));
    assert_eq!(first[2].as_deref(), Some(b"one".as_slice()));
    assert!(first[3].is_none());
    assert_eq!(first[4].as_deref(), Some(b"two".as_slice()));

    let before = db.cache_stats();
    let second = db.get_many_owned(keys, ReadOptions::default())?;
    assert_eq!(second, first);
    let after = db.cache_stats();
    assert!(after.index_hits >= before.index_hits);
    assert!(after.data_hits >= before.data_hits);
    assert!(after.open_handles <= 8);
    Ok(())
}

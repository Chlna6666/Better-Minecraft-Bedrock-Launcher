use bedrock_world::{
    MemoryStorage, PartitionedWorldStorage, StorageReadOptions, StorageVisitorControl,
    WorldExecutor, WorldStorage,
};

#[test]
fn memory_partitioned_scan_returns_worker_local_reduction() {
    let storage = MemoryStorage::new();
    storage.put(b"a", b"1").expect("put a");
    storage.put(b"b", b"2").expect("put b");
    let (outcome, partitions) = storage
        .scan_keys_partitioned(
            StorageReadOptions::default(),
            Vec::<Vec<u8>>::new,
            |keys, key| {
                keys.push(key.to_vec());
                Ok(StorageVisitorControl::Continue)
            },
        )
        .expect("partitioned scan");
    assert_eq!(outcome.visited, 2);
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].len(), 2);
}

#[test]
fn world_executor_uses_exact_worker_count_without_coordinator_thread() {
    let executor = WorldExecutor::new(3).expect("executor");
    assert_eq!(executor.worker_count(), 3);
}

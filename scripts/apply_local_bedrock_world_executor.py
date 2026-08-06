from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def replace_regex(text: str, pattern: str, replacement: str, label: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{label}: expected one regex match, found {count}")
    return updated


def patch_storage() -> None:
    path = "crates/bedrock-world/src/storage.rs"
    text = read(path)

    trait_marker = '''}

#[derive(Debug, Clone, Default)]
/// In-memory storage backend for tests and synthetic tools.
pub struct MemoryStorage'''
    partitioned_trait = '''}

/// Storage backend capable of table-parallel scans with worker-local reduction state.
///
/// Unlike [`WorldStorage::for_each_key`], this API never serializes successful
/// visitor calls through one shared mutable closure. Each backend worker owns one
/// `T`; callers merge the returned partitions after the scan.
pub trait PartitionedWorldStorage: WorldStorage {
    /// Scans visible keys with one independently initialized reduction value per worker.
    fn scan_keys_partitioned<T, I, F>(
        &self,
        options: StorageReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(StorageScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync;
}

#[derive(Debug, Clone, Default)]
/// In-memory storage backend for tests and synthetic tools.
pub struct MemoryStorage'''
    text = replace_once(text, trait_marker, partitioned_trait, "insert partitioned storage trait")

    memory_marker = '''}

/// Terrain payload length used by old Pocket Edition `chunks.dat` files before'''
    memory_impl = '''}

impl PartitionedWorldStorage for MemoryStorage {
    fn scan_keys_partitioned<T, I, F>(
        &self,
        options: StorageReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(StorageScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
    {
        let mut partition = init();
        let outcome = self.for_each_key(options, &mut |key| visitor(&mut partition, key))?;
        Ok((outcome, vec![partition]))
    }
}

/// Terrain payload length used by old Pocket Edition `chunks.dat` files before'''
    text = replace_once(text, memory_marker, memory_impl, "insert memory partitioned impl")

    old_cache = '''                compression_policy: bedrock_leveldb::CompressionPolicy::Zlib,
                cache_size: 64 * 1024 * 1024,
                write_buffer_size: 4 * 1024 * 1024,'''
    new_cache = '''                compression_policy: bedrock_leveldb::CompressionPolicy::Zlib,
                cache: if read_only {
                    bedrock_leveldb::NativeCacheOptions {
                        data_capacity: 32 * 1024 * 1024,
                        index_capacity: 64 * 1024 * 1024,
                        file_capacity: 256,
                        shards: 16,
                    }
                } else {
                    bedrock_leveldb::NativeCacheOptions::default()
                },
                write_buffer_size: 4 * 1024 * 1024,'''
    text = replace_once(text, old_cache, new_cache, "switch world backend cache configuration")

    enabled_marker = '''    #[cfg(feature = "backend-bedrock-leveldb")]
    const fn write_options() -> bedrock_leveldb::WriteOptions {'''
    enabled_impl = '''    #[cfg(feature = "backend-bedrock-leveldb")]
    impl PartitionedWorldStorage for BedrockLevelDbStorage {
        fn scan_keys_partitioned<T, I, F>(
            &self,
            options: StorageReadOptions,
            init: I,
            visitor: F,
        ) -> Result<(StorageScanOutcome, Vec<T>)>
        where
            T: Send,
            I: Fn() -> T + Send + Sync,
            F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
        {
            let visitor_error = Arc::new(std::sync::Mutex::new(None));
            let visitor_error_for_scan = Arc::clone(&visitor_error);
            let scan_result = self.db.for_each_key_partitioned(
                to_leveldb_read_options(options),
                init,
                move |partition, key| match visitor(partition, key) {
                    Ok(StorageVisitorControl::Continue) => {
                        Ok(bedrock_leveldb::VisitorControl::Continue)
                    }
                    Ok(StorageVisitorControl::Stop) => {
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                    Err(error) => {
                        if let Ok(mut slot) = visitor_error_for_scan.lock()
                            && slot.is_none()
                        {
                            *slot = Some(error);
                        }
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                },
            );
            if let Ok(mut slot) = visitor_error.lock()
                && let Some(error) = slot.take()
            {
                return Err(error);
            }
            let (outcome, partitions) = scan_result.map_err(map_leveldb_error)?;
            Ok((to_storage_outcome(outcome), partitions))
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    const fn write_options() -> bedrock_leveldb::WriteOptions {'''
    text = replace_once(text, enabled_marker, enabled_impl, "insert LevelDB partitioned impl")

    disabled_marker = '''    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl WorldStorage for BedrockLevelDbStorage {'''
    disabled_impl = '''    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl PartitionedWorldStorage for BedrockLevelDbStorage {
        fn scan_keys_partitioned<T, I, F>(
            &self,
            _options: StorageReadOptions,
            _init: I,
            _visitor: F,
        ) -> Result<(StorageScanOutcome, Vec<T>)>
        where
            T: Send,
            I: Fn() -> T + Send + Sync,
            F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
        {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }
    }

    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl WorldStorage for BedrockLevelDbStorage {'''
    text = replace_once(text, disabled_marker, disabled_impl, "insert disabled partitioned impl")
    write(path, text)


def patch_world() -> None:
    path = "crates/bedrock-world/src/world.rs"
    text = read(path)
    text = text.replace("        Mutex,\n", "        Mutex, OnceLock,\n", 1)

    old_auto = '''            Self::Auto => std::thread::available_parallelism()
                .map_or(1, usize::from)
                .min(work_items.max(1)),'''
    new_auto = '''            Self::Auto => default_world_worker_budget().min(work_items.max(1)),'''
    text = replace_once(text, old_auto, new_auto, "cap automatic world workers")

    executor_marker = '''#[derive(Debug, Clone, Default)]
/// Options controlling world scan operations.
pub struct WorldScanOptions'''
    executor_code = '''/// Persistent decode executor shared by world operations with the same worker budget.
pub struct WorldExecutor {
    worker_count: usize,
    pool: rayon::ThreadPool,
}

impl std::fmt::Debug for WorldExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorldExecutor")
            .field("worker_count", &self.worker_count)
            .finish_non_exhaustive()
    }
}

impl WorldExecutor {
    /// Creates a fixed persistent world executor.
    pub fn new(worker_count: usize) -> Result<Self> {
        let worker_count = worker_count.clamp(1, MAX_WORLD_THREADS);
        let pool = ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("bedrock-world-worker-{index}"))
            .build()
            .map_err(|error| {
                BedrockWorldError::ConcurrentWrite(format!(
                    "failed to build persistent world executor: {error}"
                ))
            })?;
        Ok(Self { worker_count, pool })
    }

    /// Number of worker threads owned by this executor.
    #[must_use]
    pub const fn worker_count(&self) -> usize {
        self.worker_count
    }
}

fn default_world_worker_budget() -> usize {
    let logical = std::thread::available_parallelism().map_or(1, usize::from);
    logical.div_ceil(2).clamp(2, 6)
}

fn world_executor(worker_count: usize) -> Result<Arc<WorldExecutor>> {
    static EXECUTORS: OnceLock<Mutex<HashMap<usize, Arc<WorldExecutor>>>> = OnceLock::new();
    let worker_count = worker_count.clamp(1, MAX_WORLD_THREADS);
    let executors = EXECUTORS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(executors) = executors.lock()
        && let Some(executor) = executors.get(&worker_count)
    {
        return Ok(Arc::clone(executor));
    }
    let executor = Arc::new(WorldExecutor::new(worker_count)?);
    let mut executors = executors.lock().map_err(|_| {
        BedrockWorldError::ConcurrentWrite("world executor registry poisoned".to_string())
    })?;
    Ok(Arc::clone(
        executors
            .entry(worker_count)
            .or_insert_with(|| Arc::clone(&executor)),
    ))
}

#[derive(Debug, Clone, Default)]
/// Options controlling world scan operations.
pub struct WorldScanOptions'''
    text = replace_once(text, executor_marker, executor_code, "insert WorldExecutor")

    # Expose the concrete storage handle without cloning or erasing it.
    storage_marker = '''    /// Returns the underlying raw storage backend.
    pub fn storage(&self) -> &dyn WorldStorage {
        self.storage.storage()
    }
'''
    storage_replacement = '''    /// Returns the underlying raw storage backend.
    pub fn storage(&self) -> &dyn WorldStorage {
        self.storage.storage()
    }

    /// Returns the concrete storage handle used by this world.
    pub const fn storage_backend(&self) -> &S {
        &self.storage
    }
'''
    text = replace_once(text, storage_marker, storage_replacement, "expose concrete storage backend")

    text = text.replace("let pool = world_pool(worker_count)?;\n        pool.scope", "let executor = world_executor(worker_count)?;\n        executor.pool.scope", 1)
    text = text.replace("let pool = world_pool(worker_count)?;\n            let decoded = pool.install", "let executor = world_executor(worker_count)?;\n            let decoded = executor.pool.install", 1)

    text = replace_regex(
        text,
        r"\nfn world_pool\(worker_count: usize\) -> Result<rayon::ThreadPool> \{.*?\n\}\n",
        "\n",
        "remove legacy world_pool",
    )

    old_test = '''    fn world_threading_validates_fixed_range_and_auto_is_not_capped_to_eight() {
        let expected_auto = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(10_000);'''
    new_test = '''    fn world_threading_uses_bounded_desktop_background_budget() {
        let expected_auto = default_world_worker_budget().min(10_000);'''
    text = replace_once(text, old_test, new_test, "update world worker budget test")
    write(path, text)


def patch_exports_and_versions() -> None:
    path = "crates/bedrock-world/src/bedrock_world.rs"
    text = read(path)
    text = text.replace(
        "StorageScanOutcome, StorageScanProgress, StorageThreadingOptions, StorageVisitorControl,\n    WorldStorage, backend::BedrockLevelDbStorage,",
        "StorageScanOutcome, StorageScanProgress, StorageThreadingOptions, StorageVisitorControl,\n    PartitionedWorldStorage, WorldStorage, backend::BedrockLevelDbStorage,",
        1,
    )
    text = text.replace(
        "WorldFormat, WorldFormatHint, WorldPipelineOptions, WorldScanOptions, WorldScanProgress,\n    WorldStorageHandle, WorldThreadingOptions, WorldTransaction,",
        "WorldExecutor, WorldFormat, WorldFormatHint, WorldPipelineOptions, WorldScanOptions,\n    WorldScanProgress, WorldStorageHandle, WorldThreadingOptions, WorldTransaction,",
        1,
    )
    write(path, text)

    cargo_path = "crates/bedrock-world/Cargo.toml"
    cargo = read(cargo_path)
    cargo = replace_once(cargo, 'version = "0.4.0"', 'version = "0.5.0"', "bump bedrock-world")
    write(cargo_path, cargo)


def patch_renderer() -> None:
    path = "crates/bedrock-render/src/renderer/pipeline.rs"
    text = read(path)
    text = text.replace(
        "StorageCachePolicy, StoragePipelineOptions, StorageReadOptions, StorageScanMode,\n    StorageThreadingOptions, StorageVisitorControl, SubChunk, SubChunkDecodeMode,",
        "PartitionedWorldStorage, StorageCachePolicy, StoragePipelineOptions, StorageReadOptions,\n    StorageScanMode, StorageThreadingOptions, StorageVisitorControl, SubChunk, SubChunkDecodeMode,",
        1,
    )

    text = replace_regex(
        text,
        r"\n/// Request used to probe renderable chunks for a group of map tiles\..*?\n/// Direct LevelDB-backed render source for map tile metadata and sessions\.",
        "\n/// Direct LevelDB-backed render source for map tile metadata and sessions.",
        "remove manifest probe public types",
    )

    text = text.replace(
        "    world: Arc<BedrockWorld<Arc<dyn WorldStorage>>>,",
        "    world: Arc<BedrockWorld<BedrockLevelDbStorage>>,",
        1,
    )
    old_open = '''        let storage: Arc<dyn WorldStorage> = Arc::new(
            BedrockLevelDbStorage::open_read_only_best_effort(world_path.join("db"))?,
        );
        let world = Arc::new(BedrockWorld::from_storage(
            world_path.clone(),
            storage,
            WorldOpenOptions::default(),
        ));'''
    new_open = '''        let storage =
            BedrockLevelDbStorage::open_read_only_best_effort(world_path.join("db"))?;
        let world = Arc::new(BedrockWorld::from_typed_storage(
            world_path.clone(),
            storage,
            WorldOpenOptions::default(),
        ));'''
    text = replace_once(text, old_open, new_open, "use typed LevelDB world")

    text = replace_regex(
        text,
        r"\n    /// Probes renderable chunks for requested tiles using direct LevelDB key scans\..*?\n    #\[cfg\(feature = \"async\"\)\]\n    pub async fn probe_tile_manifest_async\(.*?\n    \}\n",
        "\n",
        "remove manifest probe methods",
    )

    old_scan = '''        let mut positions = BTreeSet::new();
        let scan_options = StorageReadOptions {'''
    new_scan = '''        let scan_options = StorageReadOptions {'''
    text = replace_once(text, old_scan, new_scan, "remove shared occupancy set")

    old_visitor = '''        let scan_result = self.world.storage().for_each_key(scan_options, &mut |key| {
            if options
                .cancel
                .as_ref()
                .is_some_and(WorldCancelFlag::is_cancelled)
            {
                return Ok(StorageVisitorControl::Stop);
            }
            if let bedrock_world::BedrockDbKey::Chunk(chunk_key) =
                bedrock_world::BedrockDbKey::decode(key)
            {
                let position = chunk_key.pos;
                if chunk_key.tag.is_render_chunk_record()
                    && region.is_none_or(|region| render_chunk_region_contains(region, position))
                {
                    positions.insert(position);
                }
            }
            Ok(StorageVisitorControl::Continue)
        });
        match scan_result {
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
        if options
            .cancel
            .as_ref()
            .is_some_and(WorldCancelFlag::is_cancelled)
        {
            return Err(BedrockRenderError::Cancelled);
        }
        Ok(positions.into_iter().collect())'''
    new_visitor = '''        let (_, partitions) = self
            .world
            .storage_backend()
            .scan_keys_partitioned(
                scan_options,
                || Vec::<ChunkPos>::with_capacity(4096),
                |positions, key| {
                    if options
                        .cancel
                        .as_ref()
                        .is_some_and(WorldCancelFlag::is_cancelled)
                    {
                        return Ok(StorageVisitorControl::Stop);
                    }
                    if let bedrock_world::BedrockDbKey::Chunk(chunk_key) =
                        bedrock_world::BedrockDbKey::decode(key)
                    {
                        let position = chunk_key.pos;
                        if chunk_key.tag.is_render_chunk_record()
                            && region
                                .is_none_or(|region| render_chunk_region_contains(region, position))
                        {
                            positions.push(position);
                        }
                    }
                    Ok(StorageVisitorControl::Continue)
                },
            )?;
        if options
            .cancel
            .as_ref()
            .is_some_and(WorldCancelFlag::is_cancelled)
        {
            return Err(BedrockRenderError::Cancelled);
        }
        let total_positions = partitions.iter().map(Vec::len).sum();
        let mut positions = Vec::with_capacity(total_positions);
        positions.extend(partitions.into_iter().flatten());
        positions.sort_unstable();
        positions.dedup();
        Ok(positions)'''
    text = replace_once(text, old_visitor, new_visitor, "use partitioned occupancy reduction")
    write(path, text)

    cargo_path = "crates/bedrock-render/Cargo.toml"
    cargo = read(cargo_path)
    cargo = replace_once(cargo, 'version = "0.3.4"', 'version = "0.5.0"', "bump bedrock-render")
    write(cargo_path, cargo)


def add_tests() -> None:
    path = "crates/bedrock-world/tests/partitioned_executor.rs"
    (ROOT / path).write_text(
        '''use bedrock_world::{
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
''',
        encoding="utf-8",
    )


def main() -> None:
    patch_storage()
    patch_world()
    patch_exports_and_versions()
    patch_renderer()
    add_tests()


if __name__ == "__main__":
    main()

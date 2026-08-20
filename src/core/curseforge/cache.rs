use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use url::Url;

const CACHE_FORMAT_VERSION: u8 = 1;
const MEMORY_MAX_ENTRIES: usize = 128;
const MEMORY_MAX_BYTES: usize = 16 * 1024 * 1024;
const DISK_MAX_ENTRIES: usize = 256;
const DISK_MAX_BYTES: u64 = 32 * 1024 * 1024;
const DISK_PRUNE_INTERVAL: u64 = 16;

static MEMORY_CACHE: LazyLock<Mutex<MemoryCache>> =
    LazyLock::new(|| Mutex::new(MemoryCache::default()));
static REQUEST_LOCKS: LazyLock<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CACHE_PRUNE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub(super) struct CachePolicy {
    pub fresh_for: Duration,
    pub stale_for: Duration,
}

#[derive(Clone)]
struct MemoryEntry {
    body: Arc<[u8]>,
    fetched_at: u64,
    last_used: u64,
}

#[derive(Default)]
struct MemoryCache {
    entries: HashMap<String, MemoryEntry>,
    total_bytes: usize,
    clock: u64,
}

#[derive(Serialize, Deserialize)]
struct DiskMetadata {
    version: u8,
    key: String,
    fetched_at: u64,
}

struct CacheRecord {
    body: Arc<[u8]>,
    fetched_at: u64,
}

pub(super) fn policy_for(url: &Url) -> CachePolicy {
    let path = url.path();
    if path.ends_with("/categories") || path.contains("/games/") && path.ends_with("/versions") {
        CachePolicy::hours(12, 24 * 7)
    } else if path.ends_with("/description") {
        CachePolicy::hours(6, 24 * 7)
    } else if path.ends_with("/mods/search") {
        CachePolicy::minutes(10, 24 * 60)
    } else if path.ends_with("/files") {
        CachePolicy::minutes(30, 3 * 24 * 60)
    } else {
        CachePolicy::hours(1, 3 * 24)
    }
}

impl CachePolicy {
    const fn minutes(fresh_minutes: u64, stale_minutes: u64) -> Self {
        Self {
            fresh_for: Duration::from_secs(fresh_minutes * 60),
            stale_for: Duration::from_secs(stale_minutes * 60),
        }
    }

    const fn hours(fresh_hours: u64, stale_hours: u64) -> Self {
        Self::minutes(fresh_hours * 60, stale_hours * 60)
    }
}

pub(super) async fn request_guard(key: &str) -> OwnedMutexGuard<()> {
    let request_lock = {
        let mut locks = request_locks();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = Arc::new(AsyncMutex::new(()));
            locks.insert(key.to_string(), Arc::downgrade(&lock));
            lock
        }
    };
    request_lock.lock_owned().await
}

pub(super) async fn load(key: &str, max_age: Duration) -> Option<Arc<[u8]>> {
    if let Some(record) = memory_cache().get(key) {
        return (record_age(record.fetched_at) <= max_age).then_some(record.body);
    }

    let record = load_disk(key).await?;
    if record_age(record.fetched_at) > max_age {
        return None;
    }
    memory_cache().insert(key.to_string(), record.body.clone(), record.fetched_at);
    Some(record.body)
}

pub(super) async fn store(key: &str, body: Arc<[u8]>) -> Result<(), String> {
    let fetched_at = unix_timestamp();
    memory_cache().insert(key.to_string(), body.clone(), fetched_at);

    let path = cache_path(key);
    let parent = path
        .parent()
        .ok_or_else(|| "CurseForge cache path has no parent".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| format!("create CurseForge cache directory failed: {error}"))?;
    let metadata = serde_json::to_vec(&DiskMetadata {
        version: CACHE_FORMAT_VERSION,
        key: key.to_string(),
        fetched_at,
    })
    .map_err(|error| format!("serialize CurseForge cache metadata failed: {error}"))?;
    let mut encoded = Vec::with_capacity(metadata.len() + 1 + body.len());
    encoded.extend_from_slice(&metadata);
    encoded.push(b'\n');
    encoded.extend_from_slice(&body);
    write_cache_file(&path, &encoded).await?;

    let write_number = CACHE_PRUNE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if write_number.is_multiple_of(DISK_PRUNE_INTERVAL) {
        prune_disk_cache(parent).await;
    }
    Ok(())
}

pub(super) async fn invalidate(key: &str) {
    memory_cache().remove(key);
    if let Err(error) = tokio::fs::remove_file(cache_path(key)).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::debug!("remove invalid CurseForge cache entry failed: {error}");
    }
}

impl MemoryCache {
    fn get(&mut self, key: &str) -> Option<CacheRecord> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(CacheRecord {
            body: entry.body.clone(),
            fetched_at: entry.fetched_at,
        })
    }

    fn insert(&mut self, key: String, body: Arc<[u8]>, fetched_at: u64) {
        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(previous.body.len());
        }
        self.total_bytes = self.total_bytes.saturating_add(body.len());
        self.entries.insert(
            key,
            MemoryEntry {
                body,
                fetched_at,
                last_used: self.clock,
            },
        );
        while self.entries.len() > MEMORY_MAX_ENTRIES || self.total_bytes > MEMORY_MAX_BYTES {
            let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove(&oldest_key);
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(entry.body.len());
        }
    }
}

async fn load_disk(key: &str) -> Option<CacheRecord> {
    let path = cache_path(key);
    let encoded = tokio::fs::read(&path).await.ok()?;
    let newline = encoded.iter().position(|byte| *byte == b'\n')?;
    let metadata = serde_json::from_slice::<DiskMetadata>(&encoded[..newline]).ok()?;
    if metadata.version != CACHE_FORMAT_VERSION || metadata.key != key {
        invalidate(key).await;
        return None;
    }
    Some(CacheRecord {
        body: Arc::from(encoded[newline + 1..].to_vec()),
        fetched_at: metadata.fetched_at,
    })
}

async fn write_cache_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!(
        "{}.tmp",
        CACHE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&temporary, bytes)
        .await
        .map_err(|error| format!("write CurseForge cache failed: {error}"))?;
    if let Err(rename_error) = tokio::fs::rename(&temporary, path).await {
        if tokio::fs::try_exists(path).await.unwrap_or(false) {
            tokio::fs::remove_file(path)
                .await
                .map_err(|error| format!("replace CurseForge cache failed: {error}"))?;
            tokio::fs::rename(&temporary, path)
                .await
                .map_err(|error| format!("replace CurseForge cache failed: {error}"))?;
        } else {
            return Err(format!("rename CurseForge cache failed: {rename_error}"));
        }
    }
    Ok(())
}

async fn prune_disk_cache(root: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(root).await else {
        return;
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if metadata.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|value| value == "cache")
        {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_secs());
            files.push((entry.path(), metadata.len(), modified));
        }
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut total_bytes = files.iter().map(|(_, size, _)| *size).sum::<u64>();
    let mut remove_count = files.len().saturating_sub(DISK_MAX_ENTRIES);
    for (path, size, _) in files {
        if remove_count == 0 && total_bytes <= DISK_MAX_BYTES {
            break;
        }
        if tokio::fs::remove_file(path).await.is_ok() {
            total_bytes = total_bytes.saturating_sub(size);
            remove_count = remove_count.saturating_sub(1);
        }
    }
}

fn cache_path(key: &str) -> PathBuf {
    let hash = hex::encode(Sha256::digest(key.as_bytes()));
    crate::utils::file_ops::cache_subdir("curseforge_api").join(format!("{hash}.cache"))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn record_age(fetched_at: u64) -> Duration {
    Duration::from_secs(unix_timestamp().saturating_sub(fetched_at))
}

fn memory_cache() -> MutexGuard<'static, MemoryCache> {
    MEMORY_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn request_locks() -> MutexGuard<'static, HashMap<String, Weak<AsyncMutex<()>>>> {
    REQUEST_LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

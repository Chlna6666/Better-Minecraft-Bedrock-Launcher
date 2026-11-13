//! Shared, bounded display snapshots for startup warmup and Manage requests.
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use crate::core::minecraft::paths::{GamePathOptions, resolve_game_root};
use crate::tasks::runtime::blocking::run_io;

use super::data;
use super::state::{
    ManageAssetEntry, ManagePackSubtype, ManageScreenshotEntry, ManageServerEntry, ManageTab,
    ManageVersionConfig, ManagedVersionEntry,
};

mod memory;
mod observation;
mod request;
mod warmup;
mod watch;
pub(super) use observation::Observation;
use request::Request;
pub(crate) use warmup::prewarm;
pub(super) use watch::Debounce;

const MAX_ENTRIES: usize = 48;
const MAX_ROOTS: usize = 12;
const MAX_BYTES: usize = 16 * 1024 * 1024;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));
// Background warmup uses at most one slot, leaving room for an interactive miss.
static SCANS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(2));

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    root: PathBuf,
    kind: Kind,
    locale: String,
    user: Option<String>,
    source: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Kind {
    Mod,
    ResourcePack,
    BehaviorPack,
    SkinPack,
    Cape,
    World,
    Screenshot,
    Server,
    Statistics,
    Config,
    Users,
}

impl Kind {
    const COUNT: usize = 11;
    fn bit(self) -> u64 {
        1 << self as usize
    }
}

#[derive(Clone)]
enum Snapshot {
    Assets(Arc<[ManageAssetEntry]>),
    Screenshots(Arc<[ManageScreenshotEntry]>),
    Servers(Arc<[ManageServerEntry]>),
}

struct Entry {
    root: Arc<watch::Root>,
    kind: Kind,
    gate: tokio::sync::Mutex<()>,
    value: Mutex<Option<(u64, Snapshot)>>,
}

impl Entry {
    fn snapshot(&self) -> Option<Snapshot> {
        let state = self.value.lock().unwrap_or_else(|error| error.into_inner());
        state.as_ref().and_then(|(generation, value)| {
            (self.root.is_healthy() && *generation == self.root.generation(self.kind))
                .then(|| value.clone())
        })
    }
}

struct CachedEntry {
    entry: Arc<Entry>,
    accessed: Instant,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<Key, CachedEntry>,
    roots: HashMap<PathBuf, std::sync::Weak<watch::Root>>,
}

impl Cache {
    fn entry(&mut self, key: &Key) -> (Arc<Entry>, Vec<CachedEntry>) {
        if let Some(cached) = self.entries.get_mut(key)
            && cached.entry.root.is_healthy()
        {
            cached.accessed = Instant::now();
            return (cached.entry.clone(), Vec::new());
        }
        let mut retired = self.entries.remove(key).into_iter().collect::<Vec<_>>();
        self.roots.retain(|_, root| root.strong_count() != 0);
        let root = self.root(&key.root, key.kind == Kind::Mod);
        let entry = Arc::new(Entry {
            root,
            kind: key.kind,
            gate: tokio::sync::Mutex::new(()),
            value: Mutex::new(None),
        });
        self.entries.insert(
            key.clone(),
            CachedEntry {
                entry: entry.clone(),
                accessed: Instant::now(),
                bytes: 0,
            },
        );
        retired.extend(self.trim());
        (entry, retired)
    }

    fn root(&mut self, path: &std::path::Path, mods: bool) -> Arc<watch::Root> {
        self.roots.retain(|_, root| root.strong_count() != 0);
        let root = self
            .roots
            .get(path)
            .and_then(std::sync::Weak::upgrade)
            .filter(|root| root.is_healthy())
            .unwrap_or_else(|| Arc::new(watch::Root::new(path, mods)));
        self.roots.insert(path.to_path_buf(), Arc::downgrade(&root));
        root
    }

    fn trim(&mut self) -> Vec<CachedEntry> {
        let mut retired = Vec::new();
        let mut bytes = self
            .entries
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>();
        while self.entries.len() > MAX_ENTRIES || bytes > MAX_BYTES || self.root_count() > MAX_ROOTS
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.accessed)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                bytes = bytes.saturating_sub(entry.bytes);
                retired.push(entry);
            }
        }
        retired
    }

    fn root_count(&self) -> usize {
        self.entries
            .keys()
            .map(|key| &key.root)
            .collect::<HashSet<_>>()
            .len()
    }
}

async fn load(
    key: Key,
    scan: impl Future<Output = Result<Snapshot, String>>,
) -> Result<Snapshot, String> {
    let entry = lookup(key.clone()).await?;
    let _gate = entry.gate.lock().await;
    if let Some(value) = entry.snapshot() {
        return Ok(value);
    }
    let _slot = SCANS.acquire().await.map_err(|error| error.to_string())?;
    let watched = entry.root.clone();
    run_io(move || watched.refresh()).await?;
    let generation = entry.root.begin_scan(entry.kind);
    let started = Instant::now();
    let value = scan.await?;
    let bytes = value.estimated_bytes();
    tracing::debug!(kind = ?key.kind, root = %key.root.display(), bytes,
        elapsed_ms = started.elapsed().as_millis(), "Manage snapshot scanned");
    publish(key, entry.clone(), generation, value.clone(), bytes).await?;
    Ok(value)
}

async fn lookup(key: Key) -> Result<Arc<Entry>, String> {
    run_io(move || {
        let (entry, retired) = CACHE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(&key);
        // Native watcher teardown must happen outside the registry lock on an I/O worker.
        drop(retired);
        entry
    })
    .await
}

async fn publish(
    key: Key,
    entry: Arc<Entry>,
    generation: u64,
    value: Snapshot,
    bytes: usize,
) -> Result<(), String> {
    run_io(move || {
        let retired = {
            let mut cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
            if entry.root.is_healthy()
                && generation == entry.root.generation(entry.kind)
                && bytes <= MAX_BYTES
                && let Some(cached) = cache.entries.get_mut(&key)
                && Arc::ptr_eq(&cached.entry, &entry)
            {
                *entry
                    .value
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some((generation, value));
                cached.bytes = bytes;
                cached.accessed = Instant::now();
            }
            cache.trim()
        };
        drop(retired);
    })
    .await
}

pub async fn load_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    tab: ManageTab,
    subtype: ManagePackSubtype,
    user: Option<&str>,
    locale: &str,
) -> Result<Arc<[ManageAssetEntry]>, String> {
    let request = Request {
        version,
        config,
        tab,
        subtype,
        user,
        locale,
    };
    let key = request.key().await?;
    let expected = key.clone();
    let value = load(key, async {
        let entries = data::scan_assets(version, config, tab, subtype, user, locale)
            .await
            .map(|entries| Snapshot::Assets(Arc::from(entries)))?;
        request.validate(&expected).await?;
        Ok(entries)
    })
    .await?;
    match value {
        Snapshot::Assets(entries) => Ok(entries),
        _ => Err("资源缓存类型错误".into()),
    }
}

pub async fn load_screenshots(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    user: Option<&str>,
) -> Result<Arc<[ManageScreenshotEntry]>, String> {
    let request = Request {
        version,
        config,
        tab: ManageTab::Screenshot,
        subtype: ManagePackSubtype::Resource,
        user,
        locale: "",
    };
    let key = request.key().await?;
    let expected = key.clone();
    let value = load(key, async {
        let entries = data::scan_screenshots(version, config, user)
            .await
            .map(|entries| Snapshot::Screenshots(Arc::from(entries)))?;
        request.validate(&expected).await?;
        Ok(entries)
    })
    .await?;
    match value {
        Snapshot::Screenshots(entries) => Ok(entries),
        _ => Err("截图缓存类型错误".into()),
    }
}

pub async fn load_external_servers(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    user: Option<&str>,
) -> Result<Arc<[ManageServerEntry]>, String> {
    let request = Request {
        version,
        config,
        tab: ManageTab::Server,
        subtype: ManagePackSubtype::Resource,
        user,
        locale: "",
    };
    let key = request.key().await?;
    let expected = key.clone();
    let value = load(key, async {
        let entries = data::scan_external_servers(version, config, user)
            .await
            .map(|entries| Snapshot::Servers(Arc::from(entries)))?;
        request.validate(&expected).await?;
        Ok(entries)
    })
    .await?;
    match value {
        Snapshot::Servers(entries) => Ok(entries),
        _ => Err("服务器缓存类型错误".into()),
    }
}

pub(super) fn subscribe() -> tokio::sync::watch::Receiver<u64> {
    watch::subscribe()
}

pub(super) fn invalidate() {
    let cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    for entry in cache.entries.values() {
        entry.entry.root.invalidate();
    }
}

#[cfg(test)]
mod tests;

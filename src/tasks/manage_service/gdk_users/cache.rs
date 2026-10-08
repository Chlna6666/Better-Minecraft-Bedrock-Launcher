use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Instant;

use notify::event::{MetadataKind, ModifyKind};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::tasks::runtime::{BlockingTaskOptions, run_blocking};

use super::{GdkUserDirectory, scan_user_directories, user_directories};

const MAX_CACHED_ROOTS: usize = 8;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

#[derive(Default)]
struct Cache {
    entries: HashMap<PathBuf, (Arc<Entry>, Instant)>,
}

impl Cache {
    fn entry(&mut self, root: PathBuf) -> (Arc<Entry>, Option<Arc<Entry>>) {
        if let Some((entry, accessed)) = self.entries.get_mut(&root) {
            *accessed = Instant::now();
            return (Arc::clone(entry), None);
        }
        let evicted = if self.entries.len() >= MAX_CACHED_ROOTS {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, accessed))| *accessed)
                .map(|(path, _)| path.clone());
            oldest
                .and_then(|path| self.entries.remove(&path))
                .map(|(entry, _)| entry)
        } else {
            None
        };
        let entry = Arc::new(Entry::default());
        self.entries
            .insert(root, (Arc::clone(&entry), Instant::now()));
        (entry, evicted)
    }
}

#[derive(Default)]
struct Changes {
    generation: AtomicU64,
    healthy: AtomicBool,
    dirty: AtomicBool,
}

impl Changes {
    fn invalidate(&self) {
        self.dirty.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    fn mark_dirty(&self) {
        // Already-dirty trees need no additional atomic writes, tasks or queued events.
        if !self.dirty.load(Ordering::Acquire) && !self.dirty.swap(true, Ordering::AcqRel) {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn record(&self, root: &Path, event: notify::Result<Event>) -> bool {
        match event {
            Ok(event) => {
                let ignored = matches!(
                    event.kind,
                    EventKind::Access(_)
                        | EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime))
                );
                let structural = event.kind.is_create()
                    || event.kind.is_remove()
                    || matches!(event.kind, EventKind::Modify(ModifyKind::Name(_)));
                if event.need_rescan()
                    || (!ignored
                        && (event.paths.is_empty()
                            || event.paths.iter().any(|path| {
                                // The parent watch reports Users' own metadata changes,
                                // including ones caused by registering the recursive watch.
                                // Only replacement matters there; file activity is observed
                                // beneath Users by the recursive subscription.
                                // Windows also reports directory metadata on enumeration;
                                // it cannot change the latest regular-file timestamp.
                                // Probe only the notified path, never scan in this callback.
                                (path != root
                                    && path.starts_with(root)
                                    && (structural
                                        || self.dirty.load(Ordering::Acquire)
                                        || !fs::symlink_metadata(path)
                                            .is_ok_and(|metadata| metadata.is_dir())))
                                    || (structural && root.starts_with(path))
                            })))
                {
                    self.healthy.store(false, Ordering::Release);
                    self.mark_dirty();
                    return true;
                }
            }
            Err(_) => {
                self.healthy.store(false, Ordering::Release);
                self.mark_dirty();
                return true;
            }
        }
        false
    }
}

#[derive(Default)]
struct Entry {
    changes: Arc<Changes>,
    state: tokio::sync::Mutex<State>,
    watcher: Arc<tokio::sync::Mutex<Option<ActiveWatcher>>>,
    watch_generation: AtomicU64,
    completed: AtomicU64,
    #[cfg(test)]
    scans: AtomicU64,
}

#[derive(Default)]
struct State {
    snapshot: Option<(u64, Vec<GdkUserDirectory>)>,
    last_scan: Option<Vec<GdkUserDirectory>>,
}

struct ActiveWatcher {
    id: u64,
    _watcher: RecommendedWatcher,
}

type WatchSlot = tokio::sync::Mutex<Option<ActiveWatcher>>;

impl State {
    fn latest_scan(&self) -> Option<&Vec<GdkUserDirectory>> {
        self.snapshot
            .as_ref()
            .map(|(_, users)| users)
            .or(self.last_scan.as_ref())
    }
}

impl Entry {
    async fn read(&self, root: PathBuf) -> Result<Vec<GdkUserDirectory>, String> {
        let requested_scan = self.completed.load(Ordering::Acquire);
        let mut state = self.state.lock().await;
        // Requests already waiting for a scan share its result, even if ongoing game
        // writes prevent caching it. Later independent requests can scan the dirty tree.
        if self.completed.load(Ordering::Acquire) != requested_scan
            && let Some(users) = state.latest_scan()
        {
            return Ok(users.clone());
        }
        self.ensure_watcher(&root, &mut state).await;
        let generation = self.changes.generation.load(Ordering::Acquire);
        if self.changes.healthy.load(Ordering::Acquire)
            && !self.changes.dirty.load(Ordering::Acquire)
            && let Some((cached_generation, users)) = &state.snapshot
            && *cached_generation == generation
        {
            return Ok(users.clone());
        }
        self.changes.dirty.store(false, Ordering::Release);
        let generation = self.changes.generation.load(Ordering::Acquire);
        #[cfg(test)]
        self.scans.fetch_add(1, Ordering::Relaxed);
        let directories = run_blocking(
            BlockingTaskOptions::hidden("枚举 GDK 用户"),
            move || user_directories(&root),
        )
        .await?;
        let users = scan_user_directories(directories).await?;
        self.publish(&mut state, generation, &users);
        Ok(users)
    }

    fn publish(&self, state: &mut State, generation: u64, users: &[GdkUserDirectory]) {
        let reusable = self.changes.healthy.load(Ordering::Acquire)
            && !self.changes.dirty.load(Ordering::Acquire)
            && self.changes.generation.load(Ordering::Acquire) == generation;
        if reusable {
            state.snapshot = Some((generation, users.to_vec()));
            state.last_scan = None;
        } else {
            state.snapshot = None;
            state.last_scan = Some(users.to_vec());
        }
        self.completed.fetch_add(1, Ordering::Release);
    }

    async fn ensure_watcher(&self, root: &Path, state: &mut State) {
        if !self.changes.healthy.load(Ordering::Acquire) {
            // Changes during a monitoring gap cannot be represented by the old generation.
            state.snapshot = None;
            let watched_root = root.to_path_buf();
            let changes = Arc::clone(&self.changes);
            let mut slot = self.watcher.lock().await;
            let previous = slot.take();
            let id = self.watch_generation.fetch_add(1, Ordering::Relaxed);
            let watched_slot = Arc::downgrade(&self.watcher);
            *slot = match run_blocking(
                BlockingTaskOptions::hidden("监听 GDK 用户目录"),
                move || watch(watched_root, changes, previous, watched_slot, id),
            )
            .await
            {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    self.changes.healthy.store(false, Ordering::Release);
                    tracing::debug!(%error, "GDK user cache monitoring unavailable; scanning on demand");
                    None
                }
            };
        }
    }
}

fn watch(
    root: PathBuf,
    changes: Arc<Changes>,
    previous: Option<ActiveWatcher>,
    slot: Weak<WatchSlot>,
    id: u64,
) -> Result<ActiveWatcher, String> {
    // Watcher teardown/registration may block; both stay in AppRuntime's blocking task.
    drop(previous);
    changes.healthy.store(true, Ordering::Release);
    let observed = Arc::clone(&changes);
    let users = root.clone();
    let mut fired = false;
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            if fired || !observed.record(&users, event) {
                return;
            }
            // One relevant update ends this subscription. Native unwatch/drop must
            // run outside its callback thread; later events take only this fast exit.
            fired = true;
            let slot = slot.clone();
            if let Err(error) = crate::tasks::runtime::spawn_io(stop_watcher(slot, id)).map(drop) {
                tracing::debug!(%error, "GDK user watch teardown was not scheduled");
            }
        },
        Config::default(),
    )
    .map_err(|error| format!("创建 GDK 用户目录监听失败: {error}"))?;
    // Observe Users replacement without recursively watching game installation files.
    let parent = root
        .parent()
        .ok_or_else(|| "GDK 用户目录没有父目录".to_string())?;
    watcher
        .watch(parent, RecursiveMode::NonRecursive)
        .map_err(|error| format!("监听 GDK 用户目录失败: {error}"))?;
    match fs::metadata(&root) {
        Ok(_) => watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|error| format!("监听 GDK 用户数据失败: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("读取 GDK 用户目录状态失败: {error}")),
    }
    Ok(ActiveWatcher {
        id,
        _watcher: watcher,
    })
}

async fn stop_watcher(slot: Weak<WatchSlot>, id: u64) {
    let Some(slot) = slot.upgrade() else { return };
    let stopped = {
        let mut watcher = slot.lock().await;
        // An old callback may finish after a new request rearmed the subscription.
        if watcher.as_ref().is_some_and(|watcher| watcher.id == id) {
            watcher.take()
        } else {
            None
        }
    };
    if let Some(stopped) = stopped
        && let Err(error) = run_blocking(
            BlockingTaskOptions::hidden("暂停 GDK 用户监听"),
            move || {
                drop(stopped);
                Ok(())
            },
        )
        .await
    {
        tracing::debug!(%error, "GDK user watch teardown failed");
    }
}

pub(super) async fn load(root: PathBuf) -> Result<Vec<GdkUserDirectory>, String> {
    let (entry, evicted) = CACHE
        .lock()
        .map_err(|error| format!("GDK 用户缓存不可用: {error}"))?
        .entry(root.clone());
    // The last owner may tear down a native watcher. Never do that under the registry lock.
    if let Some(evicted) = evicted {
        run_blocking(
            BlockingTaskOptions::hidden("释放 GDK 用户缓存"),
            move || {
                drop(evicted);
                Ok(())
            },
        )
        .await?;
    }
    entry.read(root).await
}

pub(super) fn invalidate() -> Result<(), String> {
    let cache = CACHE
        .lock()
        .map_err(|error| format!("GDK 用户缓存不可用: {error}"))?;
    for (entry, _) in cache.entries.values() {
        entry.changes.invalidate();
    }
    Ok(())
}

#[cfg(test)]
mod tests;

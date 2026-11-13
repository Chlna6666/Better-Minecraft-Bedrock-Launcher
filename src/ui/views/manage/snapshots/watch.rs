use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::Kind;
use crate::utils::file_watch::Directories;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

static CHANGES: LazyLock<tokio::sync::watch::Sender<u64>> =
    LazyLock::new(|| tokio::sync::watch::channel(0).0);
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
static LAST_CHANGE: AtomicU64 = AtomicU64::new(0);

pub(in crate::ui::views::manage) struct Debounce {
    started: Instant,
}

impl Debounce {
    pub(in crate::ui::views::manage) fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
    pub(in crate::ui::views::manage) fn delay(&self) -> Duration {
        let last = *STARTED + Duration::from_millis(LAST_CHANGE.load(Ordering::Acquire));
        self.remaining(Instant::now(), last)
    }
    fn remaining(&self, now: Instant, last: Instant) -> Duration {
        let quiet = last + Duration::from_millis(350);
        let limit = self.started + Duration::from_secs(2);
        quiet.min(limit).saturating_duration_since(now)
    }
}

pub(super) fn subscribe() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

#[derive(Default)]
struct Changes {
    generations: [AtomicU64; Kind::COUNT],
    pending: AtomicU64,
    healthy: AtomicBool,
    rearm: AtomicBool,
    directories: Mutex<Directories>,
}

impl Changes {
    fn record(&self, event: notify::Result<Event>, scopes: &[PathBuf], mods: bool) {
        // Each dirty content kind needs only one invalidation per write burst.
        // Subsequent events skip metadata probes and additional UI notifications.
        let mask = match event {
            Ok(event) => self.event_mask(&event, scopes, mods),
            Err(_) => {
                self.healthy.store(false, Ordering::Release);
                u64::MAX
            }
        };
        if mask == 0 {
            return;
        }
        let milliseconds = STARTED.elapsed().as_millis() as u64;
        LAST_CHANGE.fetch_max(milliseconds, Ordering::AcqRel);
        let changed = mask & !self.pending.fetch_or(mask, Ordering::AcqRel);
        if changed != 0 {
            for (index, generation) in self.generations.iter().enumerate() {
                if changed & (1 << index) != 0 {
                    generation.fetch_add(1, Ordering::AcqRel);
                }
            }
            CHANGES.send_modify(|revision| *revision = revision.wrapping_add(1));
        }
    }

    fn event_mask(&self, event: &Event, scopes: &[PathBuf], mods: bool) -> u64 {
        if !event.need_rescan() && (ignored(event) || !in_scopes(event, scopes)) {
            return 0;
        }
        let mut directories = self
            .directories
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        directories.observe(event);
        if structural(event)
            && event
                .paths
                .iter()
                .any(|path| scopes.iter().any(|scope| scope.starts_with(path)))
        {
            self.rearm.store(true, Ordering::Release);
        }
        let mask = if mods {
            mod_changes(event, scopes)
        } else {
            changed_kinds(event, scopes)
        };
        let pending = self.pending.load(Ordering::Acquire) & mask == mask;
        if mask == 0
            || (pending && event.paths.iter().all(|path| directories.contains(path)))
            || (!pending && !relevant(event, scopes, &mut directories))
        {
            return 0;
        }
        mask
    }
}

fn ignored(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Access(_) | EventKind::Modify(notify::event::ModifyKind::Metadata(_))
    )
}

fn structural(event: &Event) -> bool {
    event.kind.is_create()
        || event.kind.is_remove()
        || matches!(
            event.kind,
            EventKind::Modify(notify::event::ModifyKind::Name(_))
        )
}

fn in_scopes(event: &Event, scopes: &[PathBuf]) -> bool {
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            scopes
                .iter()
                .any(|scope| path.starts_with(scope) || scope.starts_with(path))
        })
}

pub(super) struct Root {
    changes: Arc<Changes>,
    scopes: Vec<PathBuf>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    mods: bool,
}

impl Root {
    pub(super) fn new(root: &Path, mods: bool) -> Self {
        let scopes = if mods {
            vec![
                root.join("mods"),
                root.join("BLoader/mods"),
                root.join("config.json"),
            ]
        } else {
            vec![root.to_path_buf()]
        };
        Self {
            // No snapshot can be published before refresh() registers monitoring.
            // Starting optimistic allows concurrent initial requests to share an entry.
            changes: Arc::new(Changes {
                healthy: AtomicBool::new(true),
                ..Default::default()
            }),
            scopes,
            watcher: Mutex::new(None),
            mods,
        }
    }

    // Called on an I/O worker before a miss. Rearm nearest-parent watches after a
    // previously absent scope is created, with no gap in the old subscription.
    pub(super) fn refresh(&self) {
        let mut slot = self
            .watcher
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let rearm = self.changes.rearm.swap(false, Ordering::AcqRel);
        if slot.is_some() && self.is_healthy() && !rearm {
            return;
        }
        let changes = self.changes.clone();
        let scopes = self.scopes.clone();
        changes.healthy.store(true, Ordering::Release);
        let observed = changes.clone();
        let mods = self.mods;
        let watcher = RecommendedWatcher::new(
            move |event| observed.record(event, &scopes, mods),
            Config::default(),
        )
        .and_then(|mut watcher| {
            register(&mut watcher, &self.scopes)?;
            Ok(watcher)
        });
        let next = match watcher {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                changes.healthy.store(false, Ordering::Release);
                tracing::debug!(%error, "Manage monitoring unavailable; scanning on demand");
                None
            }
        };
        *slot = next;
    }

    pub(super) fn generation(&self, kind: Kind) -> u64 {
        self.changes.generations[kind as usize].load(Ordering::Acquire)
    }
    pub(super) fn generations(&self) -> [u64; Kind::COUNT] {
        std::array::from_fn(|index| self.changes.generations[index].load(Ordering::Acquire))
    }
    pub(super) fn is_healthy(&self) -> bool {
        self.changes.healthy.load(Ordering::Acquire)
    }
    pub(super) fn begin_scan(&self, kind: Kind) -> u64 {
        self.changes
            .pending
            .fetch_and(!kind.bit(), Ordering::AcqRel);
        self.generation(kind)
    }
    pub(super) fn invalidate(&self) {
        for generation in &self.changes.generations {
            generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn relevant(event: &Event, scopes: &[PathBuf], directories: &mut Directories) -> bool {
    if event.need_rescan() {
        return true;
    }
    // Enumeration/access notifications must not invalidate the scan that caused them.
    if ignored(event) {
        return false;
    }
    let structural = structural(event);
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            (structural || !directories.is_directory(path))
                && scopes
                    .iter()
                    .any(|scope| path.starts_with(scope) || scope.starts_with(path))
        })
}

fn changed_kinds(event: &Event, scopes: &[PathBuf]) -> u64 {
    // Game logs and unrelated settings do not change the displayed asset lists.
    if event.need_rescan() || event.paths.is_empty() {
        return u64::MAX;
    }
    let mut mask = 0;
    for path in &event.paths {
        for part in path.components() {
            for (name, kind) in [
                ("minecraftWorlds", Kind::World),
                ("resource_packs", Kind::ResourcePack),
                ("behavior_packs", Kind::BehaviorPack),
                ("skin_packs", Kind::SkinPack),
                ("capes", Kind::Cape),
                ("Screenshots", Kind::Screenshot),
                ("minecraftpe", Kind::Server),
                ("external_servers.txt", Kind::Server),
            ] {
                if part.as_os_str().eq_ignore_ascii_case(name) {
                    mask |= kind.bit();
                }
            }
        }
    }
    if mask == 0
        && (event.kind.is_create()
            || event.kind.is_remove()
            || matches!(
                event.kind,
                EventKind::Modify(notify::event::ModifyKind::Name(_))
            ))
        && event.paths.iter().any(|path| structural_data(path, scopes))
    {
        return u64::MAX;
    }
    mask
}

fn structural_data(path: &Path, scopes: &[PathBuf]) -> bool {
    scopes.iter().any(|scope| scope.starts_with(path))
        || path.file_name().is_some_and(|name| {
            ["Users", "games", "com.mojang"]
                .iter()
                .any(|part| name.eq_ignore_ascii_case(part))
        })
        || path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.eq_ignore_ascii_case("Users"))
}

fn mod_changes(event: &Event, scopes: &[PathBuf]) -> u64 {
    if event.need_rescan() || event.paths.is_empty() {
        return u64::MAX;
    }
    event.paths.iter().fold(0, |mask, path| {
        mask | if scopes.last().is_some_and(|config| config == path) {
            Kind::Config.bit()
        } else if scopes.iter().take(2).any(|mods| path.starts_with(mods)) {
            Kind::Mod.bit()
        } else if scopes.iter().any(|scope| scope.starts_with(path)) {
            Kind::Mod.bit() | Kind::Config.bit()
        } else {
            0
        }
    })
}

fn register(watcher: &mut RecommendedWatcher, scopes: &[PathBuf]) -> notify::Result<()> {
    let mut paths = Vec::new();
    for scope in scopes {
        let existing = scope
            .ancestors()
            .find(|path| path.is_dir())
            .ok_or_else(|| notify::Error::generic("no watchable ancestor"))?;
        let mode = if existing == scope {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        if !paths.contains(&existing.to_path_buf()) {
            watcher.watch(existing, mode)?;
            paths.push(existing.to_path_buf());
        }
        if existing == scope
            && let Some(parent) = scope.parent()
            && !paths.contains(&parent.to_path_buf())
        {
            watcher.watch(parent, RecursiveMode::NonRecursive)?;
            paths.push(parent.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, ModifyKind};
    #[test]
    fn dirty_roots_coalesce_write_bursts_and_keep_monitoring_errors() {
        let changes = Changes::default();
        let scopes = vec![PathBuf::from("game")];
        for _ in 0..1000 {
            changes.record(
                Ok(
                    Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                        .add_path("game/minecraftWorlds/world/db/file.ldb".into()),
                ),
                &scopes,
                false,
            );
        }
        assert_eq!(
            changes.generations[Kind::World as usize].load(Ordering::Acquire),
            1
        );
        assert_eq!(
            changes.generations[Kind::ResourcePack as usize].load(Ordering::Acquire),
            0
        );
        changes.record(Err(notify::Error::generic("watch failed")), &scopes, false);
        assert!(!changes.healthy.load(Ordering::Acquire));
        assert_eq!(
            changed_kinds(
                &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                    .add_path("game/logs/latest.log".into()),
                &scopes
            ),
            0
        );
    }
    #[test]
    fn watch_filters_access_and_siblings_but_observes_missing_parents() {
        let scopes = vec![PathBuf::from("root/Users/user/games")];
        let mut directories = Directories::default();
        assert!(relevant(
            &Event::new(EventKind::Create(CreateKind::Folder)).add_path("root/Users".into()),
            &scopes,
            &mut directories
        ));
        assert!(!relevant(
            &Event::new(EventKind::Access(AccessKind::Read)).add_path(scopes[0].clone()),
            &scopes,
            &mut directories
        ));
        assert!(!relevant(
            &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                .add_path("root/other/file".into()),
            &scopes,
            &mut directories
        ));
        assert!(relevant(
            &Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
                .add_path(scopes[0].join("manifest.json")),
            &scopes,
            &mut directories
        ));
    }

    #[test]
    fn debounce_waits_for_quiet_and_caps_continuous_writes() {
        let start = Instant::now();
        let debounce = Debounce { started: start };
        assert_eq!(
            debounce.remaining(start + Duration::from_millis(300), start),
            Duration::from_millis(50)
        );
        assert_eq!(
            debounce.remaining(
                start + Duration::from_millis(400),
                start + Duration::from_millis(300)
            ),
            Duration::from_millis(250)
        );
        assert!(
            debounce
                .remaining(
                    start + Duration::from_millis(700),
                    start + Duration::from_millis(300)
                )
                .is_zero()
        );
        assert!(
            debounce
                .remaining(
                    start + Duration::from_secs(2),
                    start + Duration::from_secs(2)
                )
                .is_zero()
        );
    }

    #[test]
    fn repeated_directory_and_access_events_never_advance_generations() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("resource_packs/pack");
        std::fs::create_dir_all(&directory).unwrap();
        let changes = Changes::default();
        let scopes = vec![temporary.path().to_path_buf()];
        for _ in 0..1000 {
            changes.record(
                Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(directory.clone())),
                &scopes,
                false,
            );
            changes.record(
                Ok(Event::new(EventKind::Access(AccessKind::Read))
                    .add_path(directory.join("manifest.json"))),
                &scopes,
                false,
            );
        }
        assert!(
            changes
                .generations
                .iter()
                .all(|generation| generation.load(Ordering::Acquire) == 0)
        );
        assert_eq!(changes.pending.load(Ordering::Acquire), 0);
    }
}

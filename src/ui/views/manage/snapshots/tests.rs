use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn test_key(root: &std::path::Path) -> Key {
    Key {
        root: root.into(),
        kind: Kind::Server,
        locale: String::new(),
        user: None,
        source: "Uwp:Release".into(),
    }
}

fn servers(name: &str) -> Snapshot {
    Snapshot::Servers(Arc::from([ManageServerEntry {
        key: "server:0".into(),
        index: 0,
        name: name.to_owned().into(),
        address: "localhost".into(),
        port: 19132,
        file_path: "external_servers.txt".into(),
        line_number: 1,
    }]))
}

#[test]
fn cache_separates_locale_user_and_kind_and_bounds_entries_and_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let mut cache = Cache::default();
    let first = test_key(directory.path());
    let (original, _) = cache.entry(&first);
    assert!(original.root.is_healthy());
    let (same, _) = cache.entry(&first);
    assert!(Arc::ptr_eq(&original, &same));
    for index in 0..MAX_ENTRIES + 4 {
        let key = Key {
            locale: format!("locale-{index}"),
            user: Some(index.to_string()),
            ..first.clone()
        };
        cache.entry(&key);
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert!(!cache.entries.contains_key(&first));
    let (entry, _) = cache.entry(&first);
    assert!(Arc::ptr_eq(&entry.root, &original.root));
    cache.entries.get_mut(&first).unwrap().bytes = MAX_BYTES + 1;
    drop(cache.trim());
    assert!(
        cache
            .entries
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>()
            <= MAX_BYTES
    );
    assert!(!cache.entries.contains_key(&first));
}

#[test]
fn cache_bounds_distinct_monitored_roots() {
    let directory = tempfile::tempdir().unwrap();
    let mut cache = Cache::default();
    for index in 0..MAX_ROOTS + 3 {
        cache.entry(&test_key(&directory.path().join(format!("game-{index}"))));
    }
    assert_eq!(cache.root_count(), MAX_ROOTS);
    assert_eq!(cache.entries.len(), MAX_ROOTS);
}

#[tokio::test]
async fn concurrent_requests_share_one_scan_and_the_same_allocation() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = test_key(directory.path());
    let scans = AtomicUsize::new(0);
    let scan = || async {
        scans.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(servers("cached"))
    };
    let (first, second) = tokio::join!(load(key.clone(), scan()), load(key.clone(), scan()));
    let Snapshot::Servers(first) = first.unwrap() else {
        panic!("expected servers")
    };
    let Snapshot::Servers(second) = second.unwrap() else {
        panic!("expected servers")
    };
    assert!(Arc::ptr_eq(&first, &second));
    let Snapshot::Servers(third) = load(key, scan()).await.unwrap() else {
        panic!("expected servers")
    };
    assert!(Arc::ptr_eq(&first, &third));
    assert_eq!(scans.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn invalidation_during_scan_prevents_reuse_and_errors_remain_retryable() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = test_key(directory.path());
    let changed = key.clone();
    load(key.clone(), async {
        CACHE.lock().unwrap().entries[&changed]
            .entry
            .root
            .invalidate();
        Ok(servers("old"))
    })
    .await
    .unwrap();
    assert!(
        CACHE.lock().unwrap().entries[&key]
            .entry
            .snapshot()
            .is_none()
    );
    assert!(
        load(key.clone(), async { Err("read failed".into()) })
            .await
            .is_err()
    );
    let Snapshot::Servers(fresh) = load(key, async { Ok(servers("fresh")) }).await.unwrap() else {
        panic!("expected servers")
    };
    assert_eq!(fresh[0].name.as_ref(), "fresh");
}

#[tokio::test]
async fn oversized_snapshots_are_returned_without_being_retained() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = test_key(directory.path());
    let large = servers(&"x".repeat(MAX_BYTES / 2));
    assert!(large.estimated_bytes() > MAX_BYTES);
    load(key.clone(), async { Ok(large) }).await.unwrap();
    assert!(
        CACHE.lock().unwrap().entries[&key]
            .entry
            .snapshot()
            .is_none()
    );
}

#[tokio::test]
async fn native_file_changes_invalidate_a_published_snapshot() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let key = test_key(directory.path());
    load(key.clone(), async { Ok(servers("before")) })
        .await
        .unwrap();
    let entry = CACHE.lock().unwrap().entries[&key].entry.clone();
    assert!(entry.snapshot().is_some());
    std::fs::write(directory.path().join("external_servers.txt"), "changed").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while entry.snapshot().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native file notification");
    let Snapshot::Servers(after) = load(key, async { Ok(servers("after")) }).await.unwrap() else {
        panic!("expected servers")
    };
    assert_eq!(after[0].name.as_ref(), "after");
}

#[tokio::test]
async fn native_read_bursts_reuse_the_snapshot_and_real_writes_still_invalidate() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("resource_packs/pack");
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("manifest.json");
    std::fs::write(&file, "{}").unwrap();
    let key = Key {
        kind: Kind::ResourcePack,
        ..test_key(directory.path())
    };
    let scans = AtomicUsize::new(0);
    let scan = || async {
        scans.fetch_add(1, Ordering::Relaxed);
        let _ = std::fs::read(&file).unwrap();
        Ok(Snapshot::Assets(Arc::from([])))
    };
    let Snapshot::Assets(first) = load(key.clone(), scan()).await.unwrap() else {
        panic!("expected assets")
    };
    for _ in 0..20 {
        let _ = std::fs::read_dir(&folder).unwrap().count();
        let _ = std::fs::metadata(&folder).unwrap();
        let _ = std::fs::read(&file).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let Snapshot::Assets(next) = load(key.clone(), scan()).await.unwrap() else {
            panic!("expected assets")
        };
        assert!(Arc::ptr_eq(&first, &next));
    }
    assert_eq!(scans.load(Ordering::Relaxed), 1);
    let entry = CACHE.lock().unwrap().entries[&key].entry.clone();
    std::fs::write(&file, "{\"changed\":true}").unwrap();
    wait_until_invalid(&entry).await;
    load(key, scan()).await.unwrap();
    assert_eq!(scans.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn world_writes_preserve_other_content_snapshots_in_the_same_root() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let world = directory.path().join("minecraftWorlds/world/db");
    std::fs::create_dir_all(&world).unwrap();
    let server_key = test_key(directory.path());
    let world_key = Key {
        kind: Kind::World,
        ..server_key.clone()
    };
    load(server_key.clone(), async { Ok(servers("unchanged")) })
        .await
        .unwrap();
    load(world_key.clone(), async {
        Ok(Snapshot::Assets(Arc::from([])))
    })
    .await
    .unwrap();
    let (server_entry, world_entry) = {
        let cache = CACHE.lock().unwrap();
        (
            cache.entries[&server_key].entry.clone(),
            cache.entries[&world_key].entry.clone(),
        )
    };
    std::fs::write(world.join("data.ldb"), "changed").unwrap();
    wait_until_invalid(&world_entry).await;
    assert!(server_entry.snapshot().is_some());
    load(server_key, async {
        panic!("world changes must not rescan the server list")
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unavailable_monitoring_never_reuses_a_snapshot() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let key = test_key(std::path::Path::new(""));
    let scans = AtomicUsize::new(0);
    for _ in 0..2 {
        load(key.clone(), async {
            scans.fetch_add(1, Ordering::Relaxed);
            Ok(servers("uncached"))
        })
        .await
        .unwrap();
    }
    assert_eq!(scans.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn creating_a_missing_root_rearms_monitoring_for_later_file_writes() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("game");
    let key = test_key(&root);
    load(key.clone(), async { Ok(servers("missing")) })
        .await
        .unwrap();
    let entry = CACHE.lock().unwrap().entries[&key].entry.clone();
    std::fs::create_dir(&root).unwrap();
    wait_until_invalid(&entry).await;
    load(key.clone(), async { Ok(servers("created")) })
        .await
        .unwrap();
    assert!(entry.snapshot().is_some());
    std::fs::write(root.join("external_servers.txt"), "new data").unwrap();
    wait_until_invalid(&entry).await;
    let Snapshot::Servers(after) = load(key, async { Ok(servers("updated")) }).await.unwrap()
    else {
        panic!("expected servers")
    };
    assert_eq!(after[0].name.as_ref(), "updated");
}

async fn wait_until_invalid(entry: &Entry) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while entry.snapshot().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native file notification");
}

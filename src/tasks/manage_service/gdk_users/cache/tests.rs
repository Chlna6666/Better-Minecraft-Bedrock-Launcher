use std::fs;
use std::time::Duration;

use notify::event::{AccessKind, DataChange, MetadataKind, ModifyKind};

use super::*;
use crate::tasks::manage_service::gdk_users::tests::write_file;

#[test]
fn cache_reuses_paths_and_evicts_the_least_recently_used_root() {
    let mut cache = Cache::default();
    for index in 0..MAX_CACHED_ROOTS {
        cache.entry(PathBuf::from(format!("root-{index}")));
    }
    let (first, _) = cache.entry(PathBuf::from("root-0"));
    let (same, _) = cache.entry(PathBuf::from("root-0"));
    assert!(Arc::ptr_eq(&first, &same));
    // Fixed ordering avoids relying on the platform clock's precision for eviction.
    let accessed = Instant::now();
    for (path, (_, time)) in &mut cache.entries {
        *time = if path == Path::new("root-0") {
            accessed + Duration::from_secs(1)
        } else {
            accessed
        };
    }

    let (_, evicted) = cache.entry(PathBuf::from("root-new"));

    assert!(evicted.is_some());
    assert_eq!(cache.entries.len(), MAX_CACHED_ROOTS);
    assert!(cache.entries.contains_key(Path::new("root-0")));
    assert!(cache.entries.contains_key(Path::new("root-new")));
}

#[test]
fn only_relevant_changes_invalidate_the_cache() {
    let changes = Changes::default();
    let root = Path::new("game/Users");
    changes.record(
        root,
        Ok(Event::new(EventKind::Access(AccessKind::Read)).add_path(root.join("100/file"))),
    );
    changes.record(
        root,
        Ok(
            Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                .add_path(PathBuf::from("game/other")),
        ),
    );
    assert!(!changes.record(
        root,
        Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.to_path_buf()))
    ));
    assert_eq!(changes.generation.load(Ordering::Acquire), 0);

    changes.record(
        root,
        Ok(
            Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                .add_path(root.join("100/settings/file")),
        ),
    );
    assert_eq!(changes.generation.load(Ordering::Acquire), 1);
    changes.record(root, Err(notify::Error::generic("watch failed")));
    assert!(!changes.healthy.load(Ordering::Acquire));
    assert_eq!(changes.generation.load(Ordering::Acquire), 1);
}

#[test]
fn write_bursts_do_not_lock_state_queue_work_or_count_every_event() {
    let entry = Entry::default();
    let _guard = entry.state.try_lock().unwrap();
    let root = Path::new("game/Users");
    for _ in 0..10_000 {
        entry.changes.record(
            root,
            Ok(Event::new(EventKind::Modify(ModifyKind::Metadata(
                MetadataKind::AccessTime,
            )))
            .add_path(root.join("100/db/file"))),
        );
    }
    assert!(!entry.changes.dirty.load(Ordering::Acquire));
    for _ in 0..10_000 {
        entry.changes.record(
            root,
            Ok(
                Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                    .add_path(root.join("100/db/file")),
            ),
        );
    }
    assert!(entry.changes.dirty.load(Ordering::Acquire));
    assert_eq!(entry.changes.generation.load(Ordering::Acquire), 1);
    assert_eq!(entry.scans.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn queued_requests_share_a_scan_invalidated_by_game_writes() {
    let entry = Entry::default();
    entry.changes.healthy.store(true, Ordering::Release);
    let mut state = entry.state.lock().await;
    let first = entry.read(PathBuf::from("game/Users"));
    let second = entry.read(PathBuf::from("game/Users"));
    futures_util::pin_mut!(first, second);
    assert!(futures_util::poll!(&mut first).is_pending());
    assert!(futures_util::poll!(&mut second).is_pending());
    entry.changes.mark_dirty();
    let users = [GdkUserDirectory {
        folder_name: "100".into(),
        has_worlds: true,
        has_screenshots: false,
        has_servers: false,
        last_modified: None,
    }];
    entry.publish(&mut state, 0, &users);
    assert!(state.snapshot.is_none());
    drop(state);

    let (first, second) = tokio::join!(first, second);

    assert_eq!(first.unwrap()[0].folder_name, "100");
    assert_eq!(second.unwrap()[0].folder_name, "100");
    assert_eq!(entry.scans.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn monitoring_recovery_rescans_instead_of_reusing_an_unobserved_snapshot() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let users = directory.path().join("Users");
    let modified = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    write_file(
        &users.join("100/settings/account.json"),
        b"current",
        modified,
    );
    let entry = Entry::default();
    entry.state.lock().await.snapshot = Some((
        0,
        vec![GdkUserDirectory {
            folder_name: "unobserved-old-user".into(),
            has_worlds: false,
            has_screenshots: false,
            has_servers: false,
            last_modified: None,
        }],
    ));

    let users = entry.read(users).await.unwrap();

    assert_eq!(users.len(), 1);
    assert_eq!(users[0].folder_name, "100");
    assert_eq!(users[0].last_modified, Some(modified));
}

#[tokio::test]
async fn first_update_stops_monitoring_until_the_next_read() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let users = directory.path().join("Users");
    let path = users.join("100/settings/account.json");
    let before = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    write_file(&path, b"before", before);
    let entry = Entry::default();
    entry.read(users.clone()).await.unwrap();
    assert!(entry.changes.healthy.load(Ordering::Acquire));
    let generation = entry.changes.generation.load(Ordering::Acquire);
    let after = before + Duration::from_secs(60);
    write_file(&path, b"after!", after);
    tokio::time::timeout(Duration::from_secs(5), async {
        while entry.watcher.lock().await.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        entry.changes.generation.load(Ordering::Acquire),
        generation + 1
    );
    assert!(!entry.changes.healthy.load(Ordering::Acquire));
    let latest = after + Duration::from_secs(60);
    for _ in 0..100 {
        write_file(&path, b"paused", latest);
    }
    assert_eq!(
        entry.changes.generation.load(Ordering::Acquire),
        generation + 1
    );
    assert_eq!(entry.scans.load(Ordering::Relaxed), 1);

    let snapshot = entry.read(users.clone()).await.unwrap();

    assert_eq!(snapshot[0].last_modified, Some(latest));
    assert!(entry.changes.healthy.load(Ordering::Acquire));
    assert!(entry.watcher.lock().await.is_some());
    assert_eq!(entry.scans.load(Ordering::Relaxed), 2);
    // A delayed cleanup from the previous subscription cannot remove the new one.
    stop_watcher(Arc::downgrade(&entry.watcher), 0).await;
    assert!(entry.watcher.lock().await.is_some());
    entry.read(users).await.unwrap();
    assert_eq!(entry.scans.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn missing_watch_parent_does_not_cache_an_unmonitored_empty_result() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let users = directory.path().join("missing/Users");
    let entry = Entry::default();

    assert!(entry.read(users.clone()).await.unwrap().is_empty());
    assert!(entry.state.lock().await.snapshot.is_none());
    fs::create_dir_all(users.join("100")).unwrap();
    let users = entry.read(users).await.unwrap();
    assert_eq!(users[0].folder_name, "100");
}

#[tokio::test]
async fn concurrent_requests_and_warm_hits_share_one_scan() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let users = directory.path().join("Users");
    for user in 0..4 {
        let settings = users.join(user.to_string()).join("settings");
        fs::create_dir_all(&settings).unwrap();
        for file in 0..256 {
            fs::write(settings.join(format!("{file}.json")), b"fixture").unwrap();
        }
    }
    let entry = Entry::default();
    let cold_started = Instant::now();
    let (first, second) = tokio::join!(entry.read(users.clone()), entry.read(users.clone()));
    let cold_elapsed = cold_started.elapsed();
    assert_eq!(first.unwrap().len(), 4);
    assert_eq!(second.unwrap().len(), 4);
    let warm_started = Instant::now();
    assert_eq!(entry.read(users).await.unwrap().len(), 4);
    let warm_elapsed = warm_started.elapsed();

    assert!(entry.changes.healthy.load(Ordering::Acquire));
    assert_eq!(entry.scans.load(Ordering::Relaxed), 1);
    println!(
        "GDK cache fixture: users=4 files=1024 cold_ms={:.3} warm_ms={:.3} scans=1",
        cold_elapsed.as_secs_f64() * 1000.0,
        warm_elapsed.as_secs_f64() * 1000.0
    );
}

use std::fs::{self, File, FileTimes};
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{
    scan_user_directories, scan_user_directory, sort_gdk_user_directories, user_directories,
};

pub(super) fn write_file(path: &Path, contents: &[u8], modified: SystemTime) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

#[test]
fn scan_uses_latest_nested_file_and_collects_tab_data_in_one_pass() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let older = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let latest = older + Duration::from_secs(60);
    write_file(
        &root.join("games/com.mojang/minecraftWorlds/world/db/000001.ldb"),
        b"world",
        older,
    );
    write_file(
        &root.join("games/com.mojang/Screenshots/image.png"),
        b"image",
        older,
    );
    write_file(
        &root.join("games/com.mojang/minecraftpe/external_servers.txt"),
        b"server",
        older,
    );
    let settings = root.join("settings/account.json");
    write_file(&settings, b"settings", latest);

    let user = scan_user_directory("200".to_string(), root);

    assert_eq!(user.folder_name, "200");
    assert!(user.has_worlds);
    assert!(user.has_screenshots);
    assert!(user.has_servers);
    assert_eq!(
        user.last_modified,
        Some(fs::metadata(settings).unwrap().modified().unwrap())
    );
}

#[test]
fn empty_directories_do_not_count_as_activity() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(
        directory
            .path()
            .join("games/com.mojang/minecraftWorlds/empty"),
    )
    .unwrap();

    let user = scan_user_directory("100".to_string(), directory.path());

    assert_eq!(user.last_modified, None);
    assert!(!user.has_worlds);
    assert!(!user.has_screenshots);
    assert!(!user.has_servers);
}

#[test]
fn empty_server_file_has_activity_but_no_servers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory
        .path()
        .join("games/com.mojang/minecraftpe/external_servers.txt");
    write_file(
        &path,
        b"",
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );

    let user = scan_user_directory("100".to_string(), directory.path());

    assert!(!user.has_servers);
    assert!(user.last_modified.is_some());
}

#[test]
fn enumeration_excludes_public_and_files_before_scanning() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["Public", "100", "200", "Shared"] {
        fs::create_dir(directory.path().join(name)).unwrap();
    }
    fs::write(directory.path().join("not-a-user"), b"file").unwrap();
    let mut users = user_directories(directory.path())
        .unwrap()
        .into_iter()
        .map(|(name, path)| scan_user_directory(name, &path))
        .collect::<Vec<_>>();
    users.reverse();
    sort_gdk_user_directories(&mut users);

    assert_eq!(
        users
            .iter()
            .map(|user| user.folder_name.as_str())
            .collect::<Vec<_>>(),
        ["100", "200", "Shared"]
    );
}

#[test]
fn absent_users_directory_returns_empty_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        user_directories(&directory.path().join("missing"))
            .unwrap()
            .is_empty()
    );
    fs::write(directory.path().join("file"), b"not a directory").unwrap();
    assert!(user_directories(&directory.path().join("file")).is_err());
}

#[tokio::test]
async fn concurrent_scans_match_serial_snapshots_in_stable_order() {
    crate::tasks::runtime::initialize_app_runtime().unwrap();
    let directory = tempfile::tempdir().unwrap();
    for (index, name) in ["Shared", "500", "400", "300", "200", "100"]
        .iter()
        .enumerate()
    {
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64);
        write_file(
            &directory.path().join(name).join("settings/account.json"),
            b"settings",
            modified,
        );
    }
    let directories = user_directories(directory.path()).unwrap();
    let mut serial = directories
        .iter()
        .map(|(name, path)| scan_user_directory(name.clone(), path))
        .collect::<Vec<_>>();
    sort_gdk_user_directories(&mut serial);

    let concurrent = scan_user_directories(directories).await.unwrap();

    assert_eq!(concurrent.len(), serial.len());
    for (actual, expected) in concurrent.iter().zip(&serial) {
        assert_eq!(actual.folder_name, expected.folder_name);
        assert_eq!(actual.last_modified, expected.last_modified);
        assert_eq!(actual.has_worlds, expected.has_worlds);
        assert_eq!(actual.has_screenshots, expected.has_screenshots);
        assert_eq!(actual.has_servers, expected.has_servers);
    }
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_not_followed() {
    let directory = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(directory.path(), directory.path().join("cycle")).unwrap();
    let user = scan_user_directory("100".to_string(), directory.path());
    assert_eq!(user.last_modified, None);
}

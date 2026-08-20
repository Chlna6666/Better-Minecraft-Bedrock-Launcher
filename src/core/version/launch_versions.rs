use std::cmp::Ordering;
use std::path::Path;
use std::sync::Arc;

use crate::core::minecraft::mod_loaders::InstalledModLoader;
use crate::core::version::game_info::GameInfo;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LaunchVersionEntry {
    pub(crate) folder: Arc<str>,
    pub(crate) name: Arc<str>,
    pub(crate) version: Arc<str>,
    pub(crate) manifest_version: Arc<str>,
    pub(crate) path: Arc<str>,
    pub(crate) kind: Arc<str>,
    pub(crate) custom_icon_path: Option<Arc<str>>,
    pub(crate) mod_loaders: Arc<[InstalledModLoader]>,
    pub(crate) game_info: GameInfo,
}

fn next_version_number(version: &str, cursor: &mut usize) -> Option<u64> {
    let bytes = version.as_bytes();
    let len = bytes.len();

    while *cursor < len {
        let byte = bytes[*cursor];
        if byte.is_ascii_digit() {
            break;
        }
        *cursor += 1;
    }

    if *cursor >= len {
        return None;
    }

    let start = *cursor;
    while *cursor < len && bytes[*cursor].is_ascii_digit() {
        *cursor += 1;
    }

    version[start..*cursor].parse::<u64>().ok()
}

pub(crate) fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    let mut left_cursor = 0;
    let mut right_cursor = 0;

    loop {
        let left_number = next_version_number(left, &mut left_cursor);
        let right_number = next_version_number(right, &mut right_cursor);

        match (left_number, right_number) {
            (Some(left_number), Some(right_number)) => match right_number.cmp(&left_number) {
                Ordering::Equal => continue,
                non_equal => return non_equal,
            },
            (Some(left_number), None) => {
                return if left_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Less
                };
            }
            (None, Some(right_number)) => {
                return if right_number == 0 {
                    Ordering::Equal
                } else {
                    Ordering::Greater
                };
            }
            (None, None) => return Ordering::Equal,
        }
    }
}

pub(crate) fn sort_launch_versions(versions: &mut [LaunchVersionEntry]) {
    versions.sort_by(|left, right| {
        right
            .game_info
            .total_sessions
            .cmp(&left.game_info.total_sessions)
            .then_with(|| compare_versions_desc(left.version.as_ref(), right.version.as_ref()))
            .then_with(|| left.folder.cmp(&right.folder))
    });
}

fn normalized_path_components(path: &str) -> Vec<String> {
    Path::new(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

pub(crate) fn version_folder_matches(left: &str, right: &str) -> bool {
    let left = normalized_path_components(left);
    let right = normalized_path_components(right);

    #[cfg(target_os = "windows")]
    {
        left.len() == right.len()
            && left
                .iter()
                .zip(right.iter())
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
    }

    #[cfg(not(target_os = "windows"))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(folder: &str, game_version: &str, sessions: u64) -> LaunchVersionEntry {
        LaunchVersionEntry {
            folder: Arc::from(folder),
            name: Arc::from(folder),
            version: Arc::from(game_version),
            manifest_version: Arc::from(game_version),
            path: Arc::from(folder),
            kind: Arc::from("Release"),
            custom_icon_path: None,
            mod_loaders: Arc::from([]),
            game_info: GameInfo {
                total_sessions: sessions,
                ..GameInfo::default()
            },
        }
    }

    #[test]
    fn sessions_take_priority_over_version() {
        let mut versions = vec![version("new", "1.21.0", 1), version("old", "1.20.0", 3)];
        sort_launch_versions(&mut versions);
        assert_eq!(versions[0].folder.as_ref(), "old");
    }

    #[test]
    fn zero_statistics_use_default_version_order() {
        let mut versions = vec![version("old", "1.20.0", 0), version("new", "1.21.0", 0)];
        sort_launch_versions(&mut versions);
        assert_eq!(versions[0].folder.as_ref(), "new");
    }

    #[test]
    fn folder_identity_normalizes_redundant_path_syntax() {
        assert!(version_folder_matches(
            "versions/./stable/",
            "versions/stable"
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn folder_identity_uses_windows_path_semantics() {
        assert!(version_folder_matches(
            r"C:\\Games\\BMCBL\\ZH-Test",
            "c:/games/bmcbl/zh-test/",
        ));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn folder_identity_preserves_case_on_unix() {
        assert!(!version_folder_matches(
            "versions/Stable",
            "versions/stable"
        ));
    }
}

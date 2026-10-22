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

fn candidate_list(versions: &[LaunchVersionEntry]) -> String {
    let mut candidates = versions
        .iter()
        .take(8)
        .map(|entry| format!("{}（{}）", entry.folder, entry.version))
        .collect::<Vec<_>>();
    if versions.len() > candidates.len() {
        candidates.push("等".to_string());
    }
    candidates.join("、")
}

/// Resolves a command-line version selector. Folder names win over display
/// names and game versions because folders are the stable instance identity.
pub(crate) fn select_launch_version<'a>(
    versions: &'a [LaunchVersionEntry],
    query: Option<&str>,
) -> Result<&'a LaunchVersionEntry, String> {
    let Some(query) = query.map(str::trim).filter(|query| !query.is_empty()) else {
        return versions
            .first()
            .ok_or_else(|| "未找到已安装的游戏版本".to_string());
    };

    if let Some(entry) = versions
        .iter()
        .find(|entry| version_folder_matches(&entry.folder, query))
    {
        return Ok(entry);
    }

    let matches = versions
        .iter()
        .filter(|entry| {
            entry.name.eq_ignore_ascii_case(query) || entry.version.eq_ignore_ascii_case(query)
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [entry] => Ok(entry),
        [] => Err(format!(
            "未找到版本“{query}”。可用版本：{}",
            candidate_list(versions)
        )),
        _ => Err(format!(
            "版本“{query}”匹配到多个实例，请改用版本目录名：{}",
            matches
                .iter()
                .map(|entry| entry.folder.to_string())
                .collect::<Vec<_>>()
                .join("、")
        )),
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
    fn named_version(folder: &str, name: &str, game_version: &str) -> LaunchVersionEntry {
        let mut entry = version(folder, game_version, 0);
        entry.name = Arc::from(name);
        entry
    }

    #[test]
    fn default_selection_uses_the_first_sorted_version() {
        let versions = vec![version("recent", "1.21.0", 9), version("old", "1.20.0", 1)];
        let selected = select_launch_version(&versions, None).expect("default selection");
        assert_eq!(selected.folder.as_ref(), "recent");
        assert!(select_launch_version(&[], None).is_err());
    }

    #[test]
    fn selection_prefers_the_exact_folder() {
        let versions = vec![
            named_version("stable", "1.21.0", "1.21.0"),
            named_version("preview", "1.21.0 Preview", "1.21.0.0"),
        ];
        let selected = select_launch_version(&versions, Some("stable")).expect("folder selection");
        assert_eq!(selected.folder.as_ref(), "stable");
    }

    #[test]
    fn selection_accepts_display_name_and_game_version() {
        let versions = vec![
            named_version("stable", "Stable", "1.21.0"),
            named_version("preview", "Preview", "1.21.1.0"),
        ];
        assert_eq!(
            select_launch_version(&versions, Some("preview"))
                .expect("display name selection")
                .folder
                .as_ref(),
            "preview"
        );
        assert_eq!(
            select_launch_version(&versions, Some("1.21.0"))
                .expect("game version selection")
                .folder
                .as_ref(),
            "stable"
        );
    }

    #[test]
    fn selection_reports_ambiguous_display_names() {
        let versions = vec![
            named_version("first", "Same Name", "1.21.0"),
            named_version("second", "Same Name", "1.21.1"),
        ];
        let error = select_launch_version(&versions, Some("Same Name"))
            .expect_err("ambiguous display name must fail");
        assert!(error.contains("多个实例"));
        assert!(error.contains("first"));
        assert!(error.contains("second"));
    }

    #[test]
    fn selection_lists_candidates_for_unknown_versions() {
        let versions = vec![named_version("stable", "Stable", "1.21.0")];
        let error = select_launch_version(&versions, Some("missing"))
            .expect_err("unknown version must fail");
        assert!(error.contains("missing"));
        assert!(error.contains("stable"));
    }
}

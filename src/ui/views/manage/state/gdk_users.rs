use std::time::SystemTime;

use gpui::SharedString;

use super::ManageTab;

/// GPUI-owned metadata snapshot used by Manage's GDK user picker.
#[derive(Clone, Debug)]
pub struct ManageGdkUser {
    /// GDK user directory name, including `Shared` when present.
    pub folder_name: SharedString,
    /// Whether this user's world directory contains a regular file.
    pub has_worlds: bool,
    /// Whether this user's screenshot directory contains a regular file.
    pub has_screenshots: bool,
    /// Whether this user's external server file is nonempty.
    pub has_servers: bool,
    /// Latest readable regular-file modification time anywhere in the user directory.
    /// `None` means no file timestamp was available, rather than a directory timestamp.
    pub last_modified: Option<SystemTime>,
}

impl ManageGdkUser {
    fn has_data_for(&self, tab: ManageTab) -> bool {
        match tab {
            ManageTab::Map => self.has_worlds,
            ManageTab::Screenshot => self.has_screenshots,
            ManageTab::Server => self.has_servers,
            _ => false,
        }
    }
}

pub(in crate::ui::views::manage) fn preferred_gdk_user(
    users: &[ManageGdkUser],
    tab: ManageTab,
) -> Option<SharedString> {
    users
        .iter()
        .filter(|user| user.last_modified.is_some())
        // Keep the stable directory order when multiple users have the same timestamp.
        .reduce(|latest, user| {
            if user.last_modified > latest.last_modified {
                user
            } else {
                latest
            }
        })
        .or_else(|| users.iter().find(|user| user.has_data_for(tab)))
        .or_else(|| {
            users
                .iter()
                .find(|user| !user.folder_name.as_ref().eq_ignore_ascii_case("shared"))
        })
        .or_else(|| users.first())
        .map(|user| user.folder_name.clone())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn user(name: &str, seconds: Option<u64>) -> ManageGdkUser {
        ManageGdkUser {
            folder_name: name.into(),
            has_worlds: false,
            has_screenshots: false,
            has_servers: false,
            last_modified: seconds
                .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        }
    }

    fn selected(users: &[ManageGdkUser], tab: ManageTab) -> Option<String> {
        preferred_gdk_user(users, tab).map(|name| name.to_string())
    }

    #[test]
    fn latest_file_selects_the_same_user_for_every_tab() {
        let mut older = user("100", Some(10));
        older.has_worlds = true;
        older.has_screenshots = true;
        older.has_servers = true;
        let users = [older, user("200", Some(30)), user("Shared", Some(20))];

        for tab in [
            ManageTab::Map,
            ManageTab::Screenshot,
            ManageTab::Server,
            ManageTab::ResourcePack,
        ] {
            assert_eq!(selected(&users, tab).as_deref(), Some("200"));
        }
    }

    #[test]
    fn shared_is_selected_when_its_file_is_latest() {
        let users = [user("100", Some(10)), user("Shared", Some(20))];
        assert_eq!(selected(&users, ManageTab::Map).as_deref(), Some("Shared"));
    }

    #[test]
    fn equal_timestamps_keep_stable_directory_order() {
        let users = [
            user("100", Some(20)),
            user("200", Some(20)),
            user("Shared", Some(20)),
        ];
        assert_eq!(selected(&users, ManageTab::Map).as_deref(), Some("100"));
    }

    #[test]
    fn unknown_timestamps_do_not_override_known_activity() {
        let users = [user("100", None), user("200", Some(10))];
        assert_eq!(selected(&users, ManageTab::Map).as_deref(), Some("200"));
    }

    #[test]
    fn missing_timestamps_preserve_data_aware_fallback() {
        let mut maps = user("200", None);
        maps.has_worlds = true;
        let mut screenshots = user("Shared", None);
        screenshots.has_screenshots = true;
        let users = [user("100", None), maps, screenshots];

        assert_eq!(selected(&users, ManageTab::Map).as_deref(), Some("200"));
        assert_eq!(
            selected(&users, ManageTab::Screenshot).as_deref(),
            Some("Shared")
        );
        assert_eq!(selected(&users, ManageTab::Server).as_deref(), Some("100"));
    }

    #[test]
    fn empty_and_shared_only_lists_have_defined_defaults() {
        assert_eq!(selected(&[], ManageTab::Map), None);
        assert_eq!(
            selected(&[user("Shared", None)], ManageTab::Map).as_deref(),
            Some("Shared")
        );
    }
}

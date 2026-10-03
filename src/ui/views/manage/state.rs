use gpui::{Entity, Global, SharedString};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::minecraft::paths::{BuildType, Edition};
use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManageTab {
    Statistics,
    Mod,
    ResourcePack,
    SkinPack,
    Map,
    Screenshot,
    Server,
}

impl ManageTab {
    pub const fn index(self) -> usize {
        match self {
            Self::Statistics => 0,
            Self::Mod => 1,
            Self::ResourcePack => 2,
            Self::SkinPack => 3,
            Self::Map => 4,
            Self::Screenshot => 5,
            Self::Server => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagePackSubtype {
    Resource,
    Behavior,
}

impl ManagePackSubtype {
    pub const fn index(self) -> usize {
        match self {
            Self::Resource => 0,
            Self::Behavior => 1,
        }
    }
}

// 13 * 32 ms chart stagger + 560 ms reveal needs ~976 ms; keep a safety margin so an
// unrelated targeted rerender cannot retire the last bars early.
const TAB_ANIMATION_WINDOW: Duration = Duration::from_millis(1_200);
const PACK_SUBTYPE_ANIMATION_WINDOW: Duration = Duration::from_millis(800);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManageAssetSortKey {
    Name,
    Date,
    Size,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManageAssetKind {
    Mod,
    ResourcePack,
    SkinPack,
    Map,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManageVersionConfig {
    pub enable_debug_console: bool,
    pub enable_redirection: bool,
    pub editor_mode: bool,
    pub disable_mod_loading: bool,
    pub lock_mouse_on_launch: bool,
    pub unlock_mouse_hotkey: SharedString,
    pub reduce_pixels: i32,
    pub vanilla_skin_pack_redirect: Option<SharedString>,
    pub shortcut_silent_launch: bool,
}

impl Default for ManageVersionConfig {
    fn default() -> Self {
        Self {
            enable_debug_console: false,
            enable_redirection: false,
            editor_mode: false,
            disable_mod_loading: false,
            lock_mouse_on_launch: false,
            unlock_mouse_hotkey: SharedString::from("ALT"),
            reduce_pixels: 20,
            vanilla_skin_pack_redirect: None,
            shortcut_silent_launch: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManageGdkUser {
    pub folder_name: SharedString,
    pub has_worlds: bool,
    pub has_screenshots: bool,
    pub has_servers: bool,
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

pub(super) fn preferred_gdk_user(users: &[ManageGdkUser], tab: ManageTab) -> Option<SharedString> {
    users
        .iter()
        .find(|user| user.has_data_for(tab))
        .or_else(|| {
            users
                .iter()
                .find(|user| !user.folder_name.as_ref().eq_ignore_ascii_case("shared"))
        })
        .or_else(|| users.first())
        .map(|user| user.folder_name.clone())
}

#[derive(Clone, Debug)]
pub struct ManageSkinPreviewEntry {
    pub display_name: SharedString,
    pub full_texture_path: SharedString,
    pub preview_path: Option<SharedString>,
    pub model_label: SharedString,
    pub geometry_path: Option<SharedString>,
    pub geometry_identifier: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub struct ManageAssetEntry {
    pub key: SharedString,
    pub folder_name: SharedString,
    pub display_name: SharedString,
    pub detail: Option<SharedString>,
    pub description: Option<SharedString>,
    pub file_path: SharedString,
    pub open_path: SharedString,
    pub icon_path: Option<SharedString>,
    pub modified_iso: Option<SharedString>,
    pub modified_label: Option<SharedString>,
    pub size_bytes: Option<u64>,
    pub size_label: Option<SharedString>,
    pub source: Option<SharedString>,
    pub edition: Option<SharedString>,
    pub gdk_user: Option<SharedString>,
    pub enabled: Option<bool>,
    pub mod_type: Option<SharedString>,
    pub inject_delay_ms: Option<u64>,
    pub resource_pack_count: Option<usize>,
    pub behavior_pack_count: Option<usize>,
    pub skin_count: Option<usize>,
    pub first_skin_full_texture_path: Option<SharedString>,
    pub first_skin_model_label: Option<SharedString>,
    pub skin_previews: Option<Arc<[ManageSkinPreviewEntry]>>,
    pub kind: ManageAssetKind,
}

#[derive(Clone, Debug)]
pub struct ManageScreenshotEntry {
    pub key: SharedString,
    pub image_path: SharedString,
    pub folder_path: SharedString,
    pub file_name: SharedString,
    pub capture_time_iso: Option<SharedString>,
    pub capture_time_label: Option<SharedString>,
    pub modified_iso: Option<SharedString>,
    pub modified_label: Option<SharedString>,
    pub size_bytes: Option<u64>,
    pub size_label: Option<SharedString>,
    pub gdk_user: Option<SharedString>,
}

#[derive(Clone, Debug)]
pub struct ManageServerEntry {
    pub key: SharedString,
    pub index: usize,
    pub name: SharedString,
    pub address: SharedString,
    pub port: u16,
    pub file_path: SharedString,
    pub line_number: usize,
}

#[derive(Clone, Debug)]
pub struct ManageServerMotdTarget {
    pub key: SharedString,
    pub address: SharedString,
    pub port: u16,
}

impl From<&ManageServerEntry> for ManageServerMotdTarget {
    fn from(entry: &ManageServerEntry) -> Self {
        Self {
            key: entry.key.clone(),
            address: entry.address.clone(),
            port: entry.port,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManageServerMotd {
    pub line_1: SharedString,
    pub line_2: Option<SharedString>,
    pub version: Option<SharedString>,
    pub players_online: Option<u32>,
    pub players_max: Option<u32>,
    pub latency_ms: Option<u128>,
}

#[derive(Clone, Debug)]
pub enum ManageServerMotdStatus {
    Loading,
    Online(ManageServerMotd),
    Offline(SharedString),
}

#[derive(Clone)]
pub struct ManagePageState {
    pub tab: ManageTab,
    pub tab_anim_seq: u64,
    pub tab_anim_from: ManageTab,
    pub tab_anim_started_at: Option<Instant>,
    pub versions_revision: u64,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub versions: Arc<[ManagedVersionEntry]>,
    pub selected_folder: Option<SharedString>,
    pub search_input: Option<Entity<InputState>>,
    pub search_query: SharedString,

    pub asset_search_query: SharedString,
    pub pack_subtype: ManagePackSubtype,
    pub pack_subtype_anim_seq: u64,
    pub pack_subtype_anim_from: ManagePackSubtype,
    pub pack_subtype_anim_started_at: Option<Instant>,
    pub asset_sort_key: ManageAssetSortKey,
    pub asset_sort_desc: bool,
    pub selected_asset_keys: Vec<SharedString>,

    pub version_config: ManageVersionConfig,
    pub version_config_loading: bool,
    pub version_config_error: Option<SharedString>,
    pub version_config_request_id: u64,

    pub gdk_users: Arc<[ManageGdkUser]>,
    pub selected_gdk_user: Option<SharedString>,
    pub gdk_users_loading: bool,
    pub gdk_users_error: Option<SharedString>,
    pub gdk_users_request_id: u64,

    pub assets: Arc<[ManageAssetEntry]>,
    pub assets_loaded: bool,
    pub assets_loading: bool,
    pub assets_error: Option<SharedString>,
    pub assets_request_id: u64,

    pub screenshot_search_query: SharedString,
    pub screenshots: Arc<[ManageScreenshotEntry]>,
    pub screenshots_loaded: bool,
    pub screenshots_loading: bool,
    pub screenshots_error: Option<SharedString>,
    pub screenshots_request_id: u64,

    pub server_search_query: SharedString,
    pub servers: Arc<[ManageServerEntry]>,
    pub servers_loaded: bool,
    pub servers_loading: bool,
    pub servers_error: Option<SharedString>,
    pub servers_request_id: u64,
    pub server_motd: Arc<HashMap<SharedString, ManageServerMotdStatus>>,
    pub server_motd_loading: bool,
    pub server_motd_request_id: u64,
}

impl ManagePageState {
    pub(super) fn selected_instance_revision(&self) -> ManagedInstanceRevision {
        ManagedInstanceRevision {
            folder: self.selected_folder.clone(),
            versions_revision: self.versions_revision,
        }
    }

    pub(super) fn tab_animation_active(&self, now: Instant) -> bool {
        self.tab_anim_seq != 0
            && self.tab_anim_from != self.tab
            && self.tab_anim_started_at.is_some_and(|started_at| {
                now.saturating_duration_since(started_at) <= TAB_ANIMATION_WINDOW
            })
    }

    pub(super) fn pack_subtype_animation_active(&self, now: Instant) -> bool {
        self.tab == ManageTab::ResourcePack
            && self.pack_subtype_anim_seq != 0
            && self.pack_subtype_anim_from != self.pack_subtype
            && self.pack_subtype_anim_started_at.is_some_and(|started_at| {
                now.saturating_duration_since(started_at) <= PACK_SUBTYPE_ANIMATION_WINDOW
            })
    }

    pub fn has_transient_requests(&self) -> bool {
        self.loading
            || self.version_config_loading
            || self.gdk_users_loading
            || self.assets_loading
            || self.screenshots_loading
            || self.servers_loading
            || self.server_motd_loading
    }

    pub fn reset_transient_requests(&mut self) {
        self.tab_anim_started_at = None;
        self.pack_subtype_anim_started_at = None;
        self.loading = false;
        self.version_config_loading = false;
        self.gdk_users_loading = false;
        self.assets_loading = false;
        self.screenshots_loading = false;
        self.servers_loading = false;
        self.server_motd_loading = false;

        self.version_config_request_id = self.version_config_request_id.wrapping_add(1);
        self.gdk_users_request_id = self.gdk_users_request_id.wrapping_add(1);
        self.assets_request_id = self.assets_request_id.wrapping_add(1);
        self.screenshots_request_id = self.screenshots_request_id.wrapping_add(1);
        self.servers_request_id = self.servers_request_id.wrapping_add(1);
        self.server_motd_request_id = self.server_motd_request_id.wrapping_add(1);
    }

    pub fn has_releasable_route_state(&self) -> bool {
        self.search_input.is_some()
            || self.has_transient_requests()
            || !self.selected_asset_keys.is_empty()
            || self.selected_asset_keys.capacity() != 0
            || !self.gdk_users.is_empty()
            || self.gdk_users_error.is_some()
            || self.assets_loaded
            || !self.assets.is_empty()
            || self.assets_error.is_some()
            || self.screenshots_loaded
            || !self.screenshots.is_empty()
            || self.screenshots_error.is_some()
            || self.servers_loaded
            || !self.servers.is_empty()
            || self.servers_error.is_some()
            || !self.server_motd.is_empty()
            || self.server_motd.capacity() != 0
    }

    pub fn release_route_state(&mut self) {
        self.reset_transient_requests();
        self.selected_asset_keys = Vec::new();
        self.gdk_users = Arc::from([]);
        self.gdk_users_error = None;
        self.assets = Arc::from([]);
        self.assets_loaded = false;
        self.assets_error = None;
        self.screenshots = Arc::from([]);
        self.screenshots_loaded = false;
        self.screenshots_error = None;
        self.servers = Arc::from([]);
        self.servers_loaded = false;
        self.servers_error = None;
        self.server_motd = Arc::new(HashMap::new());
    }
}

impl Default for ManagePageState {
    fn default() -> Self {
        Self {
            tab: ManageTab::Mod,
            tab_anim_seq: 0,
            tab_anim_from: ManageTab::Mod,
            tab_anim_started_at: None,
            versions_revision: 0,
            loaded: false,
            loading: false,
            error: None,
            versions: Arc::<[ManagedVersionEntry]>::from(Vec::new()),
            selected_folder: None,
            search_input: None,
            search_query: SharedString::from(""),
            asset_search_query: SharedString::from(""),
            pack_subtype: ManagePackSubtype::Resource,
            pack_subtype_anim_seq: 0,
            pack_subtype_anim_from: ManagePackSubtype::Resource,
            pack_subtype_anim_started_at: None,
            asset_sort_key: ManageAssetSortKey::Name,
            asset_sort_desc: false,
            selected_asset_keys: Vec::new(),
            version_config: ManageVersionConfig::default(),
            version_config_loading: false,
            version_config_error: None,
            version_config_request_id: 0,
            gdk_users: Arc::<[ManageGdkUser]>::from(Vec::new()),
            selected_gdk_user: None,
            gdk_users_loading: false,
            gdk_users_error: None,
            gdk_users_request_id: 0,
            assets: Arc::<[ManageAssetEntry]>::from(Vec::new()),
            assets_loaded: false,
            assets_loading: false,
            assets_error: None,
            assets_request_id: 0,
            screenshot_search_query: SharedString::from(""),
            screenshots: Arc::<[ManageScreenshotEntry]>::from(Vec::new()),
            screenshots_loaded: false,
            screenshots_loading: false,
            screenshots_error: None,
            screenshots_request_id: 0,
            server_search_query: SharedString::from(""),
            servers: Arc::<[ManageServerEntry]>::from(Vec::new()),
            servers_loaded: false,
            servers_loading: false,
            servers_error: None,
            servers_request_id: 0,
            server_motd: Arc::new(HashMap::new()),
            server_motd_loading: false,
            server_motd_request_id: 0,
        }
    }
}

impl Global for ManagePageState {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ManagedInstanceRevision {
    pub(super) folder: Option<SharedString>,
    pub(super) versions_revision: u64,
}

#[cfg(test)]
mod tests {
    use gpui::SharedString;

    use super::{
        ManageAssetEntry, ManageAssetKind, ManageGdkUser, ManagePageState, ManageScreenshotEntry,
        ManageTab, preferred_gdk_user,
    };
    use std::sync::Arc;

    #[test]
    fn reset_transient_requests_clears_loading_and_invalidates_results() {
        let mut state = ManagePageState {
            loading: true,
            version_config_loading: true,
            gdk_users_loading: true,
            assets_loading: true,
            screenshots_loading: true,
            servers_loading: true,
            server_motd_loading: true,
            version_config_request_id: 10,
            gdk_users_request_id: 20,
            assets_request_id: 30,
            screenshots_request_id: 40,
            servers_request_id: 50,
            server_motd_request_id: 60,
            ..ManagePageState::default()
        };

        state.reset_transient_requests();

        assert!(!state.has_transient_requests());
        assert_eq!(state.version_config_request_id, 11);
        assert_eq!(state.gdk_users_request_id, 21);
        assert_eq!(state.assets_request_id, 31);
        assert_eq!(state.screenshots_request_id, 41);
        assert_eq!(state.servers_request_id, 51);
        assert_eq!(state.server_motd_request_id, 61);
    }

    #[test]
    fn release_route_state_drops_loaded_instance_snapshots() {
        let mut state = ManagePageState::default();
        let assets: Arc<[ManageAssetEntry]> = vec![ManageAssetEntry {
            key: SharedString::from("asset"),
            folder_name: SharedString::from("folder"),
            display_name: SharedString::from("Asset"),
            detail: None,
            description: None,
            file_path: SharedString::from("asset_path"),
            open_path: SharedString::from("asset_path"),
            icon_path: None,
            modified_iso: None,
            modified_label: None,
            size_bytes: None,
            size_label: None,
            source: None,
            edition: None,
            gdk_user: None,
            enabled: None,
            mod_type: None,
            inject_delay_ms: None,
            resource_pack_count: None,
            behavior_pack_count: None,
            skin_count: None,
            first_skin_full_texture_path: None,
            first_skin_model_label: None,
            skin_previews: None,
            kind: ManageAssetKind::Mod,
        }]
        .into();
        let assets_weak = Arc::downgrade(&assets);
        state.assets = assets;
        state.assets_loaded = true;
        let screenshots: Arc<[ManageScreenshotEntry]> = vec![ManageScreenshotEntry {
            key: SharedString::from("screenshot"),
            image_path: SharedString::from("image"),
            folder_path: SharedString::from("folder"),
            file_name: SharedString::from("image.png"),
            capture_time_iso: None,
            capture_time_label: None,
            modified_iso: None,
            modified_label: None,
            size_bytes: None,
            size_label: None,
            gdk_user: None,
        }]
        .into();
        let screenshots_weak = Arc::downgrade(&screenshots);
        state.screenshots = screenshots;
        state.screenshots_loaded = true;
        state.selected_folder = Some(SharedString::from("folder"));
        state.loaded = true;

        assert!(state.has_releasable_route_state());
        state.release_route_state();

        assert!(assets_weak.upgrade().is_none());
        assert!(screenshots_weak.upgrade().is_none());
        assert!(!state.assets_loaded);
        assert!(!state.screenshots_loaded);
        assert!(state.assets.is_empty());
        assert!(state.screenshots.is_empty());
        assert!(state.loaded);
        assert_eq!(state.selected_folder.as_deref(), Some("folder"));
        assert!(!state.has_releasable_route_state());
    }

    #[test]
    fn selected_instance_revision_changes_after_same_folder_is_refreshed() {
        let mut state = ManagePageState {
            selected_folder: Some(SharedString::from("26.12")),
            versions_revision: 7,
            ..ManagePageState::default()
        };
        let previous = state.selected_instance_revision();

        state.versions_revision = 8;

        assert_ne!(state.selected_instance_revision(), previous);
    }

    #[test]
    fn preferred_gdk_user_has_data_for_active_tab() {
        let users = [
            ManageGdkUser {
                folder_name: SharedString::from("100"),
                has_worlds: false,
                has_screenshots: false,
                has_servers: false,
            },
            ManageGdkUser {
                folder_name: SharedString::from("200"),
                has_worlds: true,
                has_screenshots: false,
                has_servers: false,
            },
            ManageGdkUser {
                folder_name: SharedString::from("Shared"),
                has_worlds: false,
                has_screenshots: true,
                has_servers: false,
            },
        ];

        let map_user = preferred_gdk_user(&users, ManageTab::Map);
        let screenshot_user = preferred_gdk_user(&users, ManageTab::Screenshot);
        let server_user = preferred_gdk_user(&users, ManageTab::Server);

        assert_eq!(map_user.as_ref().map(SharedString::as_ref), Some("200"));
        assert_eq!(
            screenshot_user.as_ref().map(SharedString::as_ref),
            Some("Shared")
        );
        assert_eq!(server_user.as_ref().map(SharedString::as_ref), Some("100"));
    }
}

#[derive(Clone, Debug)]
pub struct ManagedModLoader {
    pub id: SharedString,
    pub name: SharedString,
    pub version: SharedString,
}

#[derive(Clone, Debug)]
pub struct ManagedVersionEntry {
    pub folder: SharedString,
    pub name: SharedString,
    pub version: SharedString,
    pub manifest_version: SharedString,
    pub path: SharedString,
    pub kind: SharedString,
    pub icon_path: Option<SharedString>,
    pub mod_loaders: Arc<[ManagedModLoader]>,
    pub game_info: crate::core::version::game_info::GameInfo,
}

impl ManagedVersionEntry {
    pub fn build_type(&self) -> BuildType {
        if self.kind.eq_ignore_ascii_case("gdk") {
            BuildType::Gdk
        } else {
            BuildType::Uwp
        }
    }

    pub fn edition(&self) -> Edition {
        if self.name.contains("Preview") || self.name.contains("Beta") {
            Edition::Preview
        } else {
            Edition::Release
        }
    }

    pub fn is_gdk(&self) -> bool {
        self.build_type() == BuildType::Gdk
    }

    pub fn is_preview(&self) -> bool {
        self.edition() == Edition::Preview
    }

    pub fn display_name(&self) -> SharedString {
        if !self.folder.is_empty() {
            return self.folder.clone();
        }
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if !self.version.is_empty() {
            return self.version.clone();
        }
        SharedString::from("Unknown")
    }
}

use gpui::{Entity, Global, SharedString};
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::minecraft::paths::{BuildType, Edition};
use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManageTab {
    Mod,
    ResourcePack,
    SkinPack,
    Map,
    Screenshot,
    Server,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagePackSubtype {
    Resource,
    Behavior,
}

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
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManageGdkUser {
    pub folder_name: SharedString,
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
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub versions: Vec<ManagedVersionEntry>,
    pub selected_folder: Option<SharedString>,
    pub search_input: Option<Entity<InputState>>,
    pub search_query: SharedString,

    pub asset_search_query: SharedString,
    pub pack_subtype: ManagePackSubtype,
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

impl Default for ManagePageState {
    fn default() -> Self {
        Self {
            tab: ManageTab::Mod,
            loaded: false,
            loading: false,
            error: None,
            versions: Vec::new(),
            selected_folder: None,
            search_input: None,
            search_query: SharedString::from(""),
            asset_search_query: SharedString::from(""),
            pack_subtype: ManagePackSubtype::Resource,
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

#[derive(Clone, Debug)]
pub struct ManagedVersionEntry {
    pub folder: SharedString,
    pub name: SharedString,
    pub version: SharedString,
    pub manifest_version: SharedString,
    pub path: SharedString,
    pub kind: SharedString,
    pub icon_path: Option<SharedString>,
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

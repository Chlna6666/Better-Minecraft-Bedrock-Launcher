use gpui::{Entity, Global, ScrollHandle, SharedString, Task};
use std::collections::BTreeMap;
use std::time::Instant;

use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsTab {
    Game,
    Launcher,
    Customization,
    Plugins,
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginSettingsSubTab {
    Readme,
    Permissions,
    Config,
    Logs,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PluginReadmeCacheKey {
    pub plugin_id: String,
    pub generation: u64,
    pub locale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PluginResourceCacheKey {
    pub plugin_id: String,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherDisplayMode {
    MinimizeOnLaunch,
    CloseOnLaunch,
    KeepVisible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LauncherConnectivityStatus {
    Pending,
    Loading,
    Success,
    Error,
}

#[derive(Clone, Debug)]
pub struct LauncherConnectivityItem {
    pub group_index: usize,
    pub item_index: usize,
    pub name: SharedString,
    pub url: SharedString,
    pub status: LauncherConnectivityStatus,
    pub latency_ms: Option<u64>,
    pub error: Option<SharedString>,
}

pub struct SettingsPageState {
    pub tab: SettingsTab,
    pub launcher_display_mode: LauncherDisplayMode,
    pub fix_uwp_minimize: bool,
    pub keep_downloaded_packages: bool,
    pub modify_appx_manifest: bool,
    pub language: SharedString,
    pub renderer_backend: SharedString,
    pub gpu_adapter_name: SharedString,
    pub gpu_adapter_options: Vec<SharedString>,
    pub debug: bool,
    pub stats_upload: bool,
    pub error_report_sentry_enabled: bool,
    pub error_report_sentry_auto: bool,
    pub update_channel_nightly: bool,
    pub auto_check_updates: bool,
    pub music_auto_play_on_startup: bool,
    pub download_multi_thread: bool,
    pub download_auto_thread_count: bool,
    pub download_max_threads: u32,
    pub download_proxy_type: SharedString,
    pub download_curseforge_api_source: SharedString,
    pub download_curseforge_api_base: SharedString,
    pub download_http_proxy_url: SharedString,
    pub download_socks_proxy_url: SharedString,
    pub download_curseforge_api_base_input: Option<Entity<InputState>>,
    pub download_http_proxy_url_input: Option<Entity<InputState>>,
    pub download_socks_proxy_url_input: Option<Entity<InputState>>,
    pub launcher_connectivity_open: bool,
    pub launcher_connectivity_running: bool,
    pub launcher_connectivity_req_id: u64,
    pub launcher_connectivity_items: Vec<LauncherConnectivityItem>,
    pub launcher_connectivity_task: Option<Task<()>>,
    pub launcher_connectivity_cancel_tx: Option<tokio::sync::watch::Sender<bool>>,
    pub theme_color: SharedString,
    pub background_option: SharedString,
    pub local_image_path: SharedString,
    pub network_image_url: SharedString,
    pub background_blur_preview: f32,
    pub background_blur: f32,
    pub glass_effect_enabled: bool,
    pub font_source: SharedString,
    pub local_font_path: SharedString,
    pub local_font_family: SharedString,
    pub system_font_family: SharedString,
    pub network_image_refresh_nonce: u64,
    pub network_image_refreshing: bool,
    pub network_image_refresh_started_at: Option<Instant>,
    pub network_image_refresh_target_url: SharedString,
    pub show_launch_animation: bool,
    pub theme_color_input: Option<Entity<InputState>>,
    pub local_image_path_input: Option<Entity<InputState>>,
    pub network_image_url_input: Option<Entity<InputState>>,
    pub theme_color_picker_popup_open: bool,
    pub theme_color_picker_drag_target: SharedString,
    pub theme_color_picker_drag_origin_x: f32,
    pub theme_color_picker_drag_origin_y: f32,
    pub theme_color_picker_drag_origin_hue: f32,
    pub theme_color_picker_drag_origin_saturation: f32,
    pub theme_color_picker_drag_origin_value: f32,
    pub theme_color_picker_drag_origin_alpha: f32,
    pub theme_color_picker_popup_anchor_x: f32,
    pub theme_color_picker_popup_anchor_y: f32,
    pub theme_color_persist_revision: u64,
    pub theme_color_persist_task_running: bool,
    pub about_sponsors_open: bool,
    pub about_sponsors_loading: bool,
    pub about_sponsors_error: Option<SharedString>,
    pub about_sponsors: Vec<AboutSponsorEntry>,
    pub about_sponsors_page: usize,
    pub about_sponsors_page_size: usize,
    pub about_sponsors_skeleton_phase: u8,
    pub about_sponsors_req_id: u64,
    pub about_dependencies_open: bool,
    pub about_dependencies_scroll_handle: ScrollHandle,
    pub about_agreement_open: bool,
    pub about_agreement_scroll_handle: ScrollHandle,
    pub font_restart_confirm_open: bool,
    pub selected_plugin_id: Option<SharedString>,
    pub plugin_sub_tab: PluginSettingsSubTab,
    pub plugin_config_draft: SharedString,
    pub plugin_config_loaded_for: Option<SharedString>,
    pub plugin_config_inputs: BTreeMap<String, Entity<InputState>>,
    pub plugin_config_inputs_for: Option<SharedString>,
    pub plugin_cached_generation: u64,
    pub plugin_cached_locale: SharedString,
    pub plugin_readme_cache: BTreeMap<PluginReadmeCacheKey, Option<String>>,
    pub plugin_config_cache: BTreeMap<PluginResourceCacheKey, Option<String>>,
    pub plugin_config_schema_cache: BTreeMap<PluginResourceCacheKey, Option<String>>,
    pub loaded: bool,
}

#[derive(Clone, Debug)]
pub struct AboutSponsorEntry {
    pub user_id: SharedString,
    pub name: SharedString,
    pub avatar_url: SharedString,
    pub all_sum_amount: SharedString,
}

impl Default for SettingsPageState {
    fn default() -> Self {
        let mut state = Self {
            tab: SettingsTab::Game,
            launcher_display_mode: LauncherDisplayMode::KeepVisible,
            fix_uwp_minimize: false,
            keep_downloaded_packages: false,
            modify_appx_manifest: false,
            language: SharedString::from(""),
            renderer_backend: SharedString::from(""),
            gpu_adapter_name: SharedString::from(""),
            gpu_adapter_options: Vec::new(),
            debug: false,
            stats_upload: false,
            error_report_sentry_enabled: false,
            error_report_sentry_auto: false,
            update_channel_nightly: false,
            auto_check_updates: false,
            music_auto_play_on_startup: false,
            download_multi_thread: false,
            download_auto_thread_count: false,
            download_max_threads: 1,
            download_proxy_type: SharedString::from(""),
            download_curseforge_api_source: SharedString::from(""),
            download_curseforge_api_base: SharedString::from(""),
            download_http_proxy_url: SharedString::from(""),
            download_socks_proxy_url: SharedString::from(""),
            download_curseforge_api_base_input: None,
            download_http_proxy_url_input: None,
            download_socks_proxy_url_input: None,
            launcher_connectivity_open: false,
            launcher_connectivity_running: false,
            launcher_connectivity_req_id: 0,
            launcher_connectivity_items: Vec::new(),
            launcher_connectivity_task: None,
            launcher_connectivity_cancel_tx: None,
            theme_color: SharedString::from(""),
            background_option: SharedString::from(""),
            local_image_path: SharedString::from(""),
            network_image_url: SharedString::from(""),
            background_blur_preview: 0.0,
            background_blur: 0.0,
            glass_effect_enabled: false,
            font_source: SharedString::from(""),
            local_font_path: SharedString::from(""),
            local_font_family: SharedString::from(""),
            system_font_family: SharedString::from(""),
            network_image_refresh_nonce: 0,
            network_image_refreshing: false,
            network_image_refresh_started_at: None,
            network_image_refresh_target_url: SharedString::from(""),
            show_launch_animation: false,
            theme_color_input: None,
            local_image_path_input: None,
            network_image_url_input: None,
            theme_color_picker_popup_open: false,
            theme_color_picker_drag_target: SharedString::from(""),
            theme_color_picker_drag_origin_x: 0.0,
            theme_color_picker_drag_origin_y: 0.0,
            theme_color_picker_drag_origin_hue: 0.0,
            theme_color_picker_drag_origin_saturation: 0.0,
            theme_color_picker_drag_origin_value: 0.0,
            theme_color_picker_drag_origin_alpha: 1.0,
            theme_color_picker_popup_anchor_x: 0.0,
            theme_color_picker_popup_anchor_y: 0.0,
            theme_color_persist_revision: 0,
            theme_color_persist_task_running: false,
            about_sponsors_open: false,
            about_sponsors_loading: false,
            about_sponsors_error: None,
            about_sponsors: Vec::new(),
            about_sponsors_page: 0,
            about_sponsors_page_size: 60,
            about_sponsors_skeleton_phase: 0,
            about_sponsors_req_id: 0,
            about_dependencies_open: false,
            about_dependencies_scroll_handle: ScrollHandle::new(),
            about_agreement_open: false,
            about_agreement_scroll_handle: ScrollHandle::new(),
            font_restart_confirm_open: false,
            selected_plugin_id: None,
            plugin_sub_tab: PluginSettingsSubTab::Readme,
            plugin_config_draft: SharedString::from(""),
            plugin_config_loaded_for: None,
            plugin_config_inputs: BTreeMap::new(),
            plugin_config_inputs_for: None,
            plugin_cached_generation: 0,
            plugin_cached_locale: SharedString::from(""),
            plugin_readme_cache: BTreeMap::new(),
            plugin_config_cache: BTreeMap::new(),
            plugin_config_schema_cache: BTreeMap::new(),
            loaded: false,
        };

        state.apply_config_values(&crate::config::config::get_default_config());
        state.loaded = false;
        state
    }
}

impl Global for SettingsPageState {}

impl SettingsPageState {
    pub fn apply_config(&mut self, config: &crate::config::config::Config) {
        self.apply_config_values(config);
        self.loaded = true;
    }

    fn apply_config_values(&mut self, config: &crate::config::config::Config) {
        self.debug = config.launcher.debug;
        self.stats_upload = config.launcher.stats_upload;
        self.error_report_sentry_enabled = config.launcher.error_report_sentry_enabled;
        self.error_report_sentry_auto = config.launcher.error_report_sentry_auto;
        self.update_channel_nightly = matches!(
            config.launcher.update_channel,
            crate::config::config::UpdateChannel::Nightly
        );
        self.auto_check_updates = config.launcher.auto_check_updates;
        self.music_auto_play_on_startup = config.music.auto_play_on_startup;
        self.renderer_backend = SharedString::from(
            crate::config::config::normalize_renderer_backend(&config.launcher.renderer_backend),
        );
        self.gpu_adapter_name = SharedString::from(
            crate::config::config::normalize_gpu_adapter_name(&config.launcher.gpu_adapter_name),
        );
        self.download_multi_thread = config.launcher.download.multi_thread;
        self.download_auto_thread_count = config.launcher.download.auto_thread_count;
        self.download_max_threads = config.launcher.download.max_threads.clamp(1, 256);
        self.download_proxy_type =
            SharedString::from(match config.launcher.download.proxy.proxy_type {
                crate::config::config::ProxyType::None => "none",
                crate::config::config::ProxyType::System => "system",
                crate::config::config::ProxyType::Http => "http",
                crate::config::config::ProxyType::Socks5 => "socks5",
            });
        self.download_curseforge_api_source = SharedString::from(
            match config
                .launcher
                .download
                .curseforge_api_source
                .to_lowercase()
                .as_str()
            {
                "official" => "official",
                "custom" => "custom",
                _ => "mirror",
            },
        );
        self.download_curseforge_api_base =
            SharedString::from(config.launcher.download.curseforge_api_base.clone());
        self.download_http_proxy_url =
            SharedString::from(config.launcher.download.proxy.http_proxy_url.clone());
        self.download_socks_proxy_url =
            SharedString::from(config.launcher.download.proxy.socks_proxy_url.clone());
        self.launcher_connectivity_open = false;
        self.launcher_connectivity_running = false;
        self.launcher_connectivity_req_id = 0;
        self.launcher_connectivity_items.clear();
        self.theme_color = SharedString::from(config.custom_style.theme_color.clone());
        self.background_option = SharedString::from(config.custom_style.background_option.clone());
        self.local_image_path = SharedString::from(config.custom_style.local_image_path.clone());
        self.network_image_url = SharedString::from(config.custom_style.network_image_url.clone());
        let font_source =
            crate::config::config::normalize_font_source(&config.custom_style.font_source);
        let font_source = if font_source == crate::config::config::FONT_SOURCE_SYSTEM
            && !crate::utils::font_settings::is_system_font_family(
                &config.custom_style.system_font_family,
            ) {
            crate::config::config::FONT_SOURCE_DEFAULT.to_string()
        } else {
            font_source
        };
        self.font_source = SharedString::from(font_source);
        self.local_font_path = SharedString::from(config.custom_style.local_font_path.clone());
        self.local_font_family = SharedString::from(config.custom_style.local_font_family.clone());
        self.system_font_family =
            SharedString::from(config.custom_style.system_font_family.clone());
        let background_blur =
            crate::config::config::clamp_background_blur(config.custom_style.background_blur);
        self.background_blur_preview = background_blur;
        self.background_blur = background_blur;
        self.glass_effect_enabled = config.custom_style.glass_effect_enabled;
        self.network_image_refresh_nonce = 0;
        self.network_image_refreshing = false;
        self.network_image_refresh_started_at = None;
        self.network_image_refresh_target_url = SharedString::from("");
        self.show_launch_animation = config.custom_style.show_launch_animation;
        self.keep_downloaded_packages = config.game.keep_downloaded_game_package;
        self.fix_uwp_minimize = config.game.uwp_minimize_fix;
        self.modify_appx_manifest = config.game.modify_appx_manifest;
        self.language = SharedString::from(config.launcher.language.clone());
        self.launcher_display_mode = match config.game.launcher_visibility.as_str() {
            "keep" => LauncherDisplayMode::KeepVisible,
            "close" => LauncherDisplayMode::CloseOnLaunch,
            _ => LauncherDisplayMode::MinimizeOnLaunch,
        };
        self.font_restart_confirm_open = false;
    }

    pub fn open_font_restart_confirm(&mut self) {
        self.font_restart_confirm_open = true;
    }

    pub fn close_font_restart_confirm(&mut self) {
        self.font_restart_confirm_open = false;
    }

    pub fn open_about_dependencies(&mut self) {
        self.about_dependencies_open = true;
        self.about_dependencies_scroll_handle = ScrollHandle::new();
    }

    pub fn close_about_dependencies(&mut self) {
        self.about_dependencies_open = false;
    }

    pub fn release_launcher_connectivity_state(&mut self) {
        self.launcher_connectivity_open = false;
        self.launcher_connectivity_running = false;
        self.launcher_connectivity_req_id = self.launcher_connectivity_req_id.saturating_add(1);
        if let Some(cancel_tx) = self.launcher_connectivity_cancel_tx.take() {
            let _ = cancel_tx.send(true);
        }
        self.launcher_connectivity_items.clear();
        self.launcher_connectivity_task = None;
    }

    pub fn commit_background_blur_preview(&mut self) -> bool {
        let blur = crate::config::config::clamp_background_blur(self.background_blur_preview);
        self.background_blur_preview = blur;
        if (self.background_blur - blur).abs() <= f32::EPSILON {
            return false;
        }
        self.background_blur = blur;
        true
    }

    pub fn has_releasable_route_state(&self) -> bool {
        let blur = crate::config::config::clamp_background_blur(self.background_blur_preview);
        (self.background_blur - blur).abs() > f32::EPSILON
            || self.download_curseforge_api_base_input.is_some()
            || self.download_http_proxy_url_input.is_some()
            || self.download_socks_proxy_url_input.is_some()
            || self.launcher_connectivity_open
            || self.launcher_connectivity_running
            || self.launcher_connectivity_cancel_tx.is_some()
            || !self.launcher_connectivity_items.is_empty()
            || self.launcher_connectivity_task.is_some()
            || self.theme_color_input.is_some()
            || self.local_image_path_input.is_some()
            || self.network_image_url_input.is_some()
            || self.theme_color_picker_popup_open
            || !self.theme_color_picker_drag_target.as_ref().is_empty()
            || self.network_image_refreshing
            || self.network_image_refresh_started_at.is_some()
            || !self.network_image_refresh_target_url.as_ref().is_empty()
            || self.about_sponsors_open
            || self.about_sponsors_loading
            || self.about_sponsors_error.is_some()
            || !self.about_sponsors.is_empty()
            || self.about_sponsors_page != 0
            || self.about_sponsors_skeleton_phase != 0
            || self.about_dependencies_open
            || self.about_agreement_open
            || self.font_restart_confirm_open
            || self.selected_plugin_id.is_some()
            || self.plugin_sub_tab != PluginSettingsSubTab::Readme
            || !self.plugin_config_draft.as_ref().is_empty()
            || self.plugin_config_loaded_for.is_some()
            || !self.plugin_config_inputs.is_empty()
            || self.plugin_config_inputs_for.is_some()
            || self.plugin_cached_generation != 0
            || !self.plugin_cached_locale.as_ref().is_empty()
            || !self.plugin_readme_cache.is_empty()
            || !self.plugin_config_cache.is_empty()
            || !self.plugin_config_schema_cache.is_empty()
    }

    pub fn release_route_state(&mut self) {
        if let Some(cancel_tx) = self.launcher_connectivity_cancel_tx.take() {
            let _ = cancel_tx.send(true);
        }
        self.download_curseforge_api_base_input = None;
        self.download_http_proxy_url_input = None;
        self.download_socks_proxy_url_input = None;
        self.launcher_connectivity_open = false;
        self.launcher_connectivity_running = false;
        self.launcher_connectivity_req_id = self.launcher_connectivity_req_id.saturating_add(1);
        self.launcher_connectivity_items.clear();
        self.launcher_connectivity_task = None;
        self.theme_color_input = None;
        self.local_image_path_input = None;
        self.network_image_url_input = None;
        self.theme_color_picker_popup_open = false;
        self.theme_color_picker_drag_target = SharedString::from("");
        self.network_image_refreshing = false;
        self.network_image_refresh_started_at = None;
        self.network_image_refresh_target_url = SharedString::from("");
        self.about_sponsors_open = false;
        self.about_sponsors_loading = false;
        self.about_sponsors_error = None;
        self.about_sponsors.clear();
        self.about_sponsors.shrink_to_fit();
        self.about_sponsors_page = 0;
        self.about_sponsors_skeleton_phase = 0;
        self.about_sponsors_req_id = self.about_sponsors_req_id.saturating_add(1);
        self.about_dependencies_open = false;
        self.about_agreement_open = false;
        self.font_restart_confirm_open = false;
        self.selected_plugin_id = None;
        self.plugin_sub_tab = PluginSettingsSubTab::Readme;
        self.plugin_config_draft = SharedString::from("");
        self.plugin_config_loaded_for = None;
        self.plugin_config_inputs.clear();
        self.plugin_config_inputs_for = None;
        self.plugin_cached_generation = 0;
        self.plugin_cached_locale = SharedString::from("");
        self.plugin_readme_cache.clear();
        self.plugin_config_cache.clear();
        self.plugin_config_schema_cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{LauncherDisplayMode, SettingsPageState};

    #[test]
    fn font_restart_confirm_can_open_and_close() {
        let mut state = SettingsPageState::default();

        assert!(!state.font_restart_confirm_open);

        state.open_font_restart_confirm();
        assert!(state.font_restart_confirm_open);

        state.close_font_restart_confirm();
        assert!(!state.font_restart_confirm_open);
    }

    #[test]
    fn about_dependencies_modal_can_open_and_close() {
        let mut state = SettingsPageState::default();

        assert!(!state.about_dependencies_open);

        state.open_about_dependencies();
        assert!(state.about_dependencies_open);

        state.close_about_dependencies();
        assert!(!state.about_dependencies_open);
    }

    #[test]
    fn default_settings_state_has_no_releasable_route_state() {
        let state = SettingsPageState::default();

        assert!(!state.has_releasable_route_state());
    }

    #[test]
    fn modal_state_forces_settings_route_release() {
        let mut state = SettingsPageState::default();
        state.open_font_restart_confirm();

        assert!(state.has_releasable_route_state());
    }

    #[test]
    fn default_state_uses_config_defaults_without_marking_loaded() {
        let config = crate::config::config::get_default_config();
        let state = SettingsPageState::default();

        assert!(!state.loaded);
        assert_eq!(
            state.keep_downloaded_packages,
            config.game.keep_downloaded_game_package
        );
        assert_eq!(state.modify_appx_manifest, config.game.modify_appx_manifest);
        assert_eq!(state.auto_check_updates, config.launcher.auto_check_updates);
        assert_eq!(
            state.music_auto_play_on_startup,
            config.music.auto_play_on_startup
        );
        assert_eq!(state.theme_color.as_ref(), config.custom_style.theme_color);
        assert_eq!(
            state.background_option.as_ref(),
            config.custom_style.background_option
        );
        assert_eq!(
            state.launcher_display_mode,
            LauncherDisplayMode::KeepVisible
        );
    }

    #[test]
    fn apply_config_syncs_music_auto_play_setting() {
        let mut config = crate::config::config::get_default_config();
        config.music.auto_play_on_startup = false;
        let mut state = SettingsPageState::default();

        state.apply_config(&config);

        assert!(!state.music_auto_play_on_startup);
    }
}

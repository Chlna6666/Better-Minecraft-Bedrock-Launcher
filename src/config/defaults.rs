use super::config::{
    AppStateConfig, CURRENT_CONFIG_VERSION, Config, CustomStyle, DEFAULT_APPX_API,
    DEFAULT_ERROR_REPORT_SENTRY_DSN, DownloadConfig, FONT_SOURCE_DEFAULT, GameConfig, Launcher,
    OnlineConfig, ProxyConfig, ProxyType, UpdateChannel,
};

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_error_report_sentry_enabled() -> bool {
    true
}

pub fn default_error_report_sentry_dsn() -> String {
    DEFAULT_ERROR_REPORT_SENTRY_DSN.to_string()
}

pub fn default_glass_effect_enabled() -> bool {
    true
}

pub(super) fn default_config_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

pub(super) fn default_appx_api() -> String {
    DEFAULT_APPX_API.to_string()
}

pub(super) fn default_renderer_backend() -> String {
    "auto".to_string()
}

pub(super) fn default_proton_gdk_source() -> String {
    "roundmcdev".to_string()
}

pub(super) fn default_update_check_interval_minutes() -> u32 {
    60
}

pub(super) fn default_log_retention_days() -> u32 {
    7
}

pub(super) fn default_log_active_size_mb() -> u32 {
    16
}

pub(super) fn default_log_archive_files() -> u32 {
    64
}

pub(super) fn default_log_total_size_mb() -> u32 {
    256
}

pub(super) fn default_log_compression_level() -> i32 {
    3
}

pub fn default_gpu_adapter_name() -> String {
    "auto".to_string()
}

pub fn default_background_blur() -> f32 {
    0.0
}

pub fn default_font_source() -> String {
    FONT_SOURCE_DEFAULT.to_string()
}

pub fn default_theme_mode() -> String {
    super::config::THEME_MODE_LIGHT.to_string()
}

pub fn default_online_player_name() -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    suffix[..6].to_string()
}

pub fn get_default_config() -> Config {
    Config {
        config_version: CURRENT_CONFIG_VERSION,
        custom_style: CustomStyle {
            theme_color: "#a0d9b6".to_string(),
            theme_mode: default_theme_mode(),
            background_option: "default".to_string(),
            local_image_path: "".to_string(),
            network_image_url: "".to_string(),
            background_blur: default_background_blur(),
            show_launch_animation: true,
            glass_effect_enabled: default_glass_effect_enabled(),
            font_source: default_font_source(),
            local_font_path: "".to_string(),
            local_font_family: "".to_string(),
            system_font_family: "".to_string(),
        },
        launcher: Launcher {
            debug: false,
            debug_port: 0,
            language: "auto".to_string(),
            renderer_backend: default_renderer_backend(),
            gpu_adapter_name: default_gpu_adapter_name(),
            stats_upload: true,
            error_report_sentry_enabled: true,
            error_report_sentry_dsn: default_error_report_sentry_dsn(),
            error_report_sentry_auto: false,
            custom_appx_api: default_appx_api(),
            download: DownloadConfig {
                multi_thread: false,
                max_threads: 8,
                auto_thread_count: true,
                proxy: ProxyConfig {
                    proxy_type: ProxyType::None,
                    http_proxy_url: "".to_string(),
                    socks_proxy_url: "".to_string(),
                },
                curseforge_api_source: "mirror".to_string(),
                curseforge_api_base: "https://mod.mcimirror.top/curseforge".to_string(),
            },
            update_channel: UpdateChannel::Stable,
            auto_check_updates: true,
            check_on_start: true,
            update_check_interval_minutes: 60,
            log_management: Default::default(),
            #[cfg(target_os = "linux")]
            proton_gdk_source: default_proton_gdk_source(),
            #[cfg(target_os = "linux")]
            proton_gdk_runner: String::new(),
        },
        game: GameConfig {
            launcher_visibility: "keep".to_string(),
            keep_downloaded_game_package: false,
            modify_appx_manifest: true,
            uwp_minimize_fix: true,
        },
        online: OnlineConfig::default(),
        app_state: AppStateConfig::default(),
        agreement_accepted: false,
    }
}

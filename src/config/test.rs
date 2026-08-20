use super::config::{
    merge_json_values, normalize_gpu_adapter_name, normalize_renderer_backend, normalize_theme_mode,
};
use serde_json::json;

fn clear_config_cache_for_test() -> std::sync::MutexGuard<'static, ()> {
    super::storage::clear_config_cache_for_test()
}
#[test]
fn missing_glass_effect_enabled_defaults_to_true() {
    let config: super::config::Config = toml::from_str(
        r##"
        agreement_accepted = false

        [custom_style]
        theme_color = "#a0d9b6"
        background_option = "default"
        local_image_path = ""
        network_image_url = ""
        show_launch_animation = true

        [launcher]
        debug = false
        language = "auto"
        custom_appx_api = "https://data.mcappx.com/v2/bedrock.json"
        auto_check_updates = true
        check_on_start = false
        update_check_interval_minutes = 60

        [launcher.download]
        multi_thread = false
        max_threads = 8
        auto_thread_count = true

        [launcher.download.proxy]
        proxy_type = "none"
        http_proxy_url = ""
        socks_proxy_url = ""

        [game]
        launcher_visibility = "keep"
        keep_downloaded_game_package = false
        modify_appx_manifest = true
        uwp_minimize_fix = true
        "##,
    )
    .expect("legacy config should deserialize");

    assert!(config.custom_style.glass_effect_enabled);
    assert_eq!(config.custom_style.theme_mode, "light");
    assert_eq!(
        config.launcher.gpu_adapter_name,
        super::config::default_gpu_adapter_name()
    );
    assert!(config.launcher.error_report_sentry_enabled);
    assert_eq!(
        config.launcher.error_report_sentry_dsn,
        super::config::default_error_report_sentry_dsn()
    );
    assert_eq!(config.launcher.log_management.retention_days, 7);
    assert_eq!(config.online.player_name.len(), 6);
    assert!(
        config
            .online
            .player_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    );
    assert_eq!(config.online.game_ports, "7551");
    assert!(!config.online.disable_p2p);
}

#[test]
fn default_online_player_name_is_six_alphanumeric_characters() {
    let name = super::config::default_online_player_name();
    assert_eq!(name.len(), 6);
    assert!(
        name.chars()
            .all(|character| character.is_ascii_alphanumeric())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn default_proton_gdk_source_supports_game_login() {
    assert_eq!(
        super::config::get_default_config()
            .launcher
            .proton_gdk_source,
        "roundmcdev"
    );
}

#[test]
fn missing_log_management_uses_seven_day_retention() {
    let config: super::config::LogManagementConfig =
        toml::from_str("").expect("log management defaults should deserialize");

    assert_eq!(config.retention_days, 7);
}

#[test]
fn obsolete_music_section_triggers_config_cleanup() {
    assert!(super::storage::has_obsolete_music_section(
        "config_version = 1\n\n  [music]  \nvolume = 0.5\n"
    ));
    assert!(!super::storage::has_obsolete_music_section(
        "# [music]\n[plugin.music]\nenabled = true\n"
    ));
}

#[test]
fn config_roundtrip_drops_obsolete_music_settings() {
    let mut content = toml::to_string(&super::config::get_default_config())
        .expect("default config should serialize");
    content.push_str(
        "\n[music]\nauto_play_on_startup = true\nvolume = 0.5\nmuted = false\nplayback_mode = \"repeat\"\nlast_track_path = \"old.mp3\"\n",
    );

    let config: super::config::Config =
        toml::from_str(&content).expect("legacy music settings should be ignored while loading");
    let rewritten = toml::to_string(&config).expect("migrated config should serialize");

    assert!(!rewritten.contains("[music]"));
    assert!(!rewritten.contains("last_track_path"));
}

#[test]
fn log_management_migration_clamps_unsafe_values() {
    let mut config = super::config::get_default_config();
    config.launcher.log_management.retention_days = 0;
    config.launcher.log_management.active_file_size_mb = u32::MAX;
    config.launcher.log_management.max_archive_files = 0;
    config.launcher.log_management.max_total_size_mb = 1;
    config.launcher.log_management.compression_level = 99;

    let migrated = super::storage::normalize_log_management(&mut config, true);

    assert!(migrated);
    assert_eq!(config.launcher.log_management.retention_days, 1);
    assert_eq!(config.launcher.log_management.active_file_size_mb, 512);
    assert_eq!(config.launcher.log_management.max_archive_files, 1);
    assert_eq!(config.launcher.log_management.max_total_size_mb, 16);
    assert_eq!(config.launcher.log_management.compression_level, 9);
}

#[test]
fn merge_json_values_preserves_existing_nested_fields() {
    let mut current = json!({
        "launcher": {
            "debug": false,
            "download": {
                "max_threads": 8,
                "proxy": {
                    "proxy_type": "none",
                    "http_proxy_url": ""
                }
            }
        },
        "game": {
            "keep_downloaded_game_package": false
        }
    });
    let overlay = json!({
        "launcher": {
            "download": {
                "proxy": {
                    "proxy_type": "system"
                }
            }
        }
    });

    merge_json_values(&mut current, overlay);

    assert_eq!(current["launcher"]["debug"], false);
    assert_eq!(current["launcher"]["download"]["max_threads"], 8);
    assert_eq!(
        current["launcher"]["download"]["proxy"]["proxy_type"],
        "system"
    );
    assert_eq!(
        current["launcher"]["download"]["proxy"]["http_proxy_url"],
        ""
    );
    assert_eq!(current["game"]["keep_downloaded_game_package"], false);
}

#[test]
fn merge_json_values_inserts_new_fields() {
    let mut current = json!({
        "launcher": {
            "debug": false
        }
    });
    let overlay = json!({
        "custom_style": {
            "theme_color": "#a0d9b6"
        }
    });

    merge_json_values(&mut current, overlay);

    assert_eq!(current["launcher"]["debug"], false);
    assert_eq!(current["custom_style"]["theme_color"], "#a0d9b6");
}

#[test]
fn renderer_backend_normalization_migrates_legacy_dx11() {
    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(normalize_renderer_backend("dx11"), "dx12");
        assert_eq!(normalize_renderer_backend("directx11"), "dx12");
    }
    #[cfg(target_os = "linux")]
    {
        assert_eq!(normalize_renderer_backend("dx11"), "auto");
        assert_eq!(normalize_renderer_backend("directx11"), "auto");
    }
    assert_eq!(normalize_renderer_backend("vulkan"), "vulkan");
    assert_eq!(normalize_renderer_backend("nova-vulkan"), "vulkan");
    assert_eq!(
        normalize_renderer_backend("nova-dx12"),
        if cfg!(target_os = "linux") {
            "auto"
        } else {
            "dx12"
        }
    );
}

#[test]
fn gpu_adapter_name_normalization_keeps_real_device_names() {
    assert_eq!(normalize_gpu_adapter_name(""), "auto");
    assert_eq!(normalize_gpu_adapter_name(" auto "), "auto");
    assert_eq!(normalize_gpu_adapter_name("low-power"), "auto");
    assert_eq!(
        normalize_gpu_adapter_name(" NVIDIA GeForce RTX 4060 "),
        "NVIDIA GeForce RTX 4060"
    );
}

#[test]
fn theme_mode_normalization_defaults_to_light() {
    assert_eq!(normalize_theme_mode("dark"), "dark");
    assert_eq!(normalize_theme_mode("DARK"), "dark");
    assert_eq!(normalize_theme_mode("light"), "light");
    assert_eq!(normalize_theme_mode(""), "light");
    assert_eq!(normalize_theme_mode("system"), "light");
}

#[test]
fn resolved_error_report_sentry_dsn_uses_default_when_enabled() {
    let mut launcher = super::config::get_default_config().launcher;
    launcher.error_report_sentry_dsn.clear();

    assert_eq!(
        super::config::resolved_error_report_sentry_dsn(&launcher).as_deref(),
        Some(super::config::DEFAULT_ERROR_REPORT_SENTRY_DSN)
    );
}

#[test]
fn resolved_error_report_sentry_dsn_is_none_when_disabled() {
    let mut launcher = super::config::get_default_config().launcher;
    launcher.error_report_sentry_enabled = false;

    assert_eq!(
        super::config::resolved_error_report_sentry_dsn(&launcher),
        None
    );
}

#[test]
fn error_report_sentry_auto_requires_enabled_reporting() {
    let mut launcher = super::config::get_default_config().launcher;
    launcher.error_report_sentry_auto = true;
    assert!(super::config::error_report_sentry_auto_enabled(&launcher));

    launcher.error_report_sentry_enabled = false;
    assert!(!super::config::error_report_sentry_auto_enabled(&launcher));
}

#[test]
fn legacy_check_on_start_migrates_to_auto_check_updates() {
    let mut config = super::config::get_default_config();
    config.launcher.auto_check_updates = true;
    config.launcher.check_on_start = false;

    let migrated = super::storage::normalize_update_check_settings(&mut config, false, true);

    assert!(migrated);
    assert!(!config.launcher.auto_check_updates);
    assert!(!config.launcher.check_on_start);
}

#[test]
fn legacy_default_appx_api_migrates_to_accelerated_mirror() {
    let mut config = super::config::get_default_config();
    config.launcher.custom_appx_api = super::config::LEGACY_DEFAULT_APPX_API.to_string();

    assert!(super::storage::normalize_appx_api(&mut config));
    assert_eq!(
        config.launcher.custom_appx_api,
        super::config::DEFAULT_APPX_API
    );
}

#[test]
fn incorrect_mirror_appx_api_migrates_to_mcappx_endpoint() {
    let mut config = super::config::get_default_config();
    config.launcher.custom_appx_api = super::config::INCORRECT_MIRROR_APPX_API.to_string();

    assert!(super::storage::normalize_appx_api(&mut config));
    assert_eq!(
        config.launcher.custom_appx_api,
        super::config::DEFAULT_APPX_API
    );
}

#[test]
fn custom_appx_api_is_preserved_during_migration() {
    let mut config = super::config::get_default_config();
    config.launcher.custom_appx_api = "https://example.invalid/versions.json".to_string();

    assert!(!super::storage::normalize_appx_api(&mut config));
    assert_eq!(
        config.launcher.custom_appx_api,
        "https://example.invalid/versions.json"
    );
}

#[test]
fn auto_check_updates_is_authoritative_when_both_fields_exist() {
    let mut config = super::config::get_default_config();
    config.launcher.auto_check_updates = false;
    config.launcher.check_on_start = true;

    let migrated = super::storage::normalize_update_check_settings(&mut config, true, true);

    assert!(migrated);
    assert!(!config.launcher.auto_check_updates);
    assert!(!config.launcher.check_on_start);
}

#[test]
fn read_config_requires_startup_initialized_cache() {
    let _guard = clear_config_cache_for_test();

    let error = super::config::read_config().expect_err("read_config should require cache init");

    assert!(error.to_string().contains("not initialized"));
}

#[test]
fn update_config_requires_startup_initialized_cache() {
    let guard = clear_config_cache_for_test();
    drop(guard);

    let error = super::config::update_config(|config| {
        config.launcher.debug = true;
    })
    .expect_err("update_config should require cache init");

    assert!(error.to_string().contains("not initialized"));
}

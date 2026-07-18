use super::config::{
    clamp_music_volume, merge_json_values, normalize_gpu_adapter_name, normalize_renderer_backend,
    normalize_theme_mode,
};
use crate::music::MusicPlaybackMode;
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
    assert!(config.music.auto_play_on_startup);
    assert_eq!(config.music.volume, super::defaults::default_music_volume());
    assert_eq!(config.music.playback_mode, MusicPlaybackMode::Repeat);
    assert!(config.online.player_name.starts_with("BMCBL_USER_"));
    assert_eq!(config.online.player_name.len(), "BMCBL_USER_".len() + 6);
    assert_eq!(config.online.game_ports, "7551");
    assert!(!config.online.disable_p2p);
    assert!(config.online.no_tun);
}

#[test]
fn default_online_player_name_has_random_suffix() {
    let name = super::config::default_online_player_name();
    assert!(name.starts_with("BMCBL_USER_"));
    assert_eq!(name.len(), "BMCBL_USER_".len() + 6);
    assert!(
        name["BMCBL_USER_".len()..]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric())
    );
}

#[test]
fn music_volume_clamps_invalid_values() {
    assert_eq!(clamp_music_volume(-1.0), 0.0);
    assert_eq!(clamp_music_volume(2.0), 1.0);
    assert_eq!(
        clamp_music_volume(f32::NAN),
        super::defaults::default_music_volume()
    );
}

#[test]
fn music_settings_migration_clamps_invalid_volume() {
    let mut config = super::config::get_default_config();
    config.music.volume = 8.0;

    let migrated = super::storage::normalize_music_settings(&mut config, true);

    assert!(migrated);
    assert_eq!(config.music.volume, 1.0);
}

#[test]
fn music_playback_mode_serializes_as_lowercase_toml() {
    #[derive(serde::Deserialize, serde::Serialize)]
    struct Wrapper {
        playback_mode: MusicPlaybackMode,
    }

    let encoded = toml::to_string(&Wrapper {
        playback_mode: MusicPlaybackMode::Shuffle,
    })
    .expect("mode should serialize");
    assert!(encoded.contains("playback_mode = \"shuffle\""));

    let decoded: Wrapper =
        toml::from_str("playback_mode = \"repeat\"").expect("mode should deserialize");
    assert_eq!(decoded.playback_mode, MusicPlaybackMode::Repeat);
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
    assert_eq!(normalize_renderer_backend("dx11"), "dx12");
    assert_eq!(normalize_renderer_backend("directx11"), "dx12");
    assert_eq!(normalize_renderer_backend("vulkan"), "vulkan");
    assert_eq!(normalize_renderer_backend("nova-vulkan"), "vulkan");
    assert_eq!(normalize_renderer_backend("nova-dx12"), "dx12");
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

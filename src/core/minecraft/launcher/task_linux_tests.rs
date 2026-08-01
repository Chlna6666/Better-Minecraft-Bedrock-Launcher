use super::{
    BLOADER_LEGACY_ORIGINAL_DEBUG_CONSOLE_KEY, BLOADER_LEGACY_STDIO_WORKAROUND_KEY,
    BLOADER_PROCESS_CAPTURE_BLOCKER_MARKER, BLOADER_PROCESS_CAPTURE_DIRECTORY,
    BLOADER_PROCESS_STDOUT_CAPTURE_NAME, LaunchRequest, classify_runner_failure,
    configure_bloader_linux_stdio_workaround, incompatible_proton_prefix_needs_backup,
    install_roundmcdev_bloader_mod, normalize_runner_output_line, proton_game_input_is_ready,
    proton_wine_prefix_path, remove_legacy_roundmcdev_preload, request_uses_preview_data,
    roundmcdev_prefix_is_ready, runner_supports_winegdk_login, sanitize_instance_folder_name,
    wine_z_path,
};
use crate::core::linux_runtime::RunnerKind;
use std::path::{Path, PathBuf};

fn temporary_test_directory(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bmcbl-{name}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn instance_folder_name_cannot_escape_prefix_root() {
    assert_eq!(sanitize_instance_folder_name("."), "default");
    assert_eq!(sanitize_instance_folder_name(".."), "default");
    assert_eq!(sanitize_instance_folder_name("../preview"), ".._preview");
    assert_eq!(sanitize_instance_folder_name("folder/name"), "folder_name");
}

#[test]
fn instance_folder_name_preserves_safe_ascii_names() {
    assert_eq!(
        sanitize_instance_folder_name("1.21_preview-2"),
        "1.21_preview-2"
    );
    assert_eq!(sanitize_instance_folder_name(""), "default");
}

#[test]
fn absolute_linux_path_is_converted_to_wine_z_drive() {
    let converted = wine_z_path(Path::new("/home/user/GameInputRedist.msi"));
    assert_eq!(
        converted.ok().and_then(|path| path.into_string().ok()),
        Some(r"Z:\home\user\GameInputRedist.msi".to_string())
    );
}

#[test]
fn relative_path_is_rejected_for_wine_z_drive() {
    assert!(wine_z_path(Path::new("GameInputRedist.msi")).is_err());
}

#[test]
fn missing_i386_loader_is_reported_as_actionable_runner_failure() {
    let failure = classify_runner_failure("/lib/ld-linux.so.2: could not open");
    assert!(
        failure
            .as_deref()
            .is_some_and(|message| message.contains("WoW64 runner"))
    );
}

#[test]
fn unimplemented_combase_api_recommends_compatible_runner() {
    let failure = classify_runner_failure(
        "wine: Call to unimplemented function combase.dll.RoOriginateErrorW, aborting",
    );
    assert!(
        failure
            .as_deref()
            .is_some_and(|message| message.contains("RoundMCDev"))
    );
}

#[test]
fn unrelated_runner_output_is_not_a_fatal_failure() {
    assert!(classify_runner_failure("fsync: up and running.").is_none());
}

#[test]
fn protonfixes_external_launcher_warning_is_not_reported_as_unit_test() {
    assert_eq!(
        normalize_runner_output_line(
            "ProtonFixes[1] WARN: Skipping fix execution. We are probably running a unit test."
        ),
        "ProtonFixes: 外部启动器模式，跳过游戏专用 fixes"
    );
}

#[test]
fn preview_launch_request_uses_preview_gdk_data_root() {
    let request = LaunchRequest::new(
        "1.21-preview",
        "Minecraft Preview",
        "1.21.0",
        "/games/MinecraftWindowsBeta",
    );

    assert!(request_uses_preview_data(&request));
}

#[test]
fn release_launch_request_uses_release_gdk_data_root() {
    let request = LaunchRequest::new("1.21-release", "Minecraft", "1.21.0", "/games/Minecraft");

    assert!(!request_uses_preview_data(&request));
}

#[test]
fn only_roundmcdev_umu_runner_uses_winegdk_login() {
    assert!(runner_supports_winegdk_login(RunnerKind::Umu));
    assert!(!runner_supports_winegdk_login(RunnerKind::Proton));
    assert!(!runner_supports_winegdk_login(RunnerKind::Wine));
}

#[test]
fn proton_compatibility_data_uses_pfx_as_wine_prefix() {
    let compatibility_path = Path::new("/data/bmcbl/prefixes/26.33");

    assert_eq!(
        proton_wine_prefix_path(compatibility_path),
        compatibility_path.join("pfx")
    );
}

#[test]
fn wine_created_prefix_without_proton_metadata_requires_backup()
-> Result<(), Box<dyn std::error::Error>> {
    let prefix = temporary_test_directory("wine-prefix");
    std::fs::create_dir_all(prefix.join("pfx"))?;
    std::fs::write(prefix.join("pfx/system.reg"), b"WINE REGISTRY Version 2\n")?;

    assert!(incompatible_proton_prefix_needs_backup(&prefix));

    std::fs::remove_dir_all(prefix)?;
    Ok(())
}

#[test]
fn proton_managed_prefix_does_not_require_backup() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = temporary_test_directory("proton-prefix");
    std::fs::create_dir_all(prefix.join("pfx"))?;
    std::fs::write(prefix.join("pfx/system.reg"), b"WINE REGISTRY Version 2\n")?;
    std::fs::write(prefix.join("version"), b"10-32\n")?;

    assert!(!incompatible_proton_prefix_needs_backup(&prefix));

    std::fs::remove_dir_all(prefix)?;
    Ok(())
}

#[test]
fn roundmcdev_prefix_is_ready_only_after_wineboot_files_are_present()
-> Result<(), Box<dyn std::error::Error>> {
    let prefix = temporary_test_directory("roundmcdev-prefix-ready");
    std::fs::create_dir_all(&prefix)?;

    assert!(!roundmcdev_prefix_is_ready(&prefix));

    std::fs::create_dir_all(prefix.join("drive_c/windows/system32"))?;
    std::fs::write(prefix.join("system.reg"), b"WINE REGISTRY Version 2\n")?;
    std::fs::write(prefix.join("user.reg"), b"WINE REGISTRY Version 2\n")?;

    assert!(roundmcdev_prefix_is_ready(&prefix));

    std::fs::remove_dir_all(prefix)?;
    Ok(())
}

#[test]
fn roundmcdev_prefix_rejects_invalid_wine_registry() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = temporary_test_directory("roundmcdev-prefix-invalid-registry");
    std::fs::create_dir_all(prefix.join("drive_c/windows/system32"))?;
    std::fs::write(prefix.join("system.reg"), b"not a Wine registry")?;
    std::fs::write(prefix.join("user.reg"), b"WINE REGISTRY Version 2\n")?;

    assert!(!roundmcdev_prefix_is_ready(&prefix));

    std::fs::remove_dir_all(prefix)?;
    Ok(())
}

#[tokio::test]
async fn game_input_requires_native_files_and_registry() -> Result<(), Box<dyn std::error::Error>> {
    let prefix = temporary_test_directory("game-input");
    let native_directory = prefix.join("pfx/drive_c/Program Files/Microsoft GameInput/x64");
    std::fs::create_dir_all(&native_directory)?;
    std::fs::write(
        native_directory.join("GameInputRedist.dll"),
        b"native game input",
    )?;
    std::fs::write(
        native_directory.join("GameInputRedistService.exe"),
        b"native game input service",
    )?;
    std::fs::write(prefix.join("pfx/system.reg"), b"WINE REGISTRY Version 2\n")?;

    assert!(!proton_game_input_is_ready(&prefix).await?);

    std::fs::write(
        prefix.join("pfx/system.reg"),
        b"WINE REGISTRY Version 2\nGameInput Redist Service\n",
    )?;
    assert!(proton_game_input_is_ready(&prefix).await?);

    std::fs::remove_dir_all(prefix)?;
    Ok(())
}

#[test]
fn roundmcdev_game_patch_uses_bloader_native_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("roundmcdev-native-mod");
    let source_dll = game_directory.join("source-mcpatcher_core.dll");
    std::fs::create_dir_all(&game_directory)?;
    std::fs::write(&source_dll, b"patch")?;

    install_roundmcdev_bloader_mod(&game_directory, &source_dll, "Release10-32")?;

    let manifest_path = game_directory.join("mods/roundmcdev-game-patch/manifest.json");
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
    assert_eq!(manifest["entry"], "mcpatcher_core.dll");
    assert_eq!(manifest["type"], "native");
    assert_eq!(manifest["version"], "Release10-32");
    assert_eq!(manifest["required"], true);
    assert_eq!(manifest["notify_success"], false);
    assert_eq!(
        std::fs::read(game_directory.join("mods/roundmcdev-game-patch/mcpatcher_core.dll"))?,
        b"patch"
    );

    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

#[test]
fn roundmcdev_game_patch_removes_legacy_preload_copy() -> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("roundmcdev-legacy-preload");
    let legacy_path = game_directory.join("preload/mcpatcher_core.dll");
    std::fs::create_dir_all(
        legacy_path
            .parent()
            .ok_or("legacy preload path should have a parent")?,
    )?;
    std::fs::write(&legacy_path, b"old patch version")?;

    remove_legacy_roundmcdev_preload(&game_directory)?;

    assert!(!legacy_path.exists());
    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

#[test]
fn bloader_0_2_11_without_console_blocks_recursive_capture()
-> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("bloader-stdio-workaround");
    std::fs::create_dir_all(&game_directory)?;
    std::fs::write(
        game_directory.join("config.json"),
        br#"{"enable_debug_console":false,"default_locale":"zh_CN"}"#,
    )?;

    assert!(configure_bloader_linux_stdio_workaround(
        &game_directory,
        "0.2.11.0"
    )?);

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(game_directory.join("config.json"))?)?;
    assert_eq!(config["enable_debug_console"], false);
    assert_eq!(config["default_locale"], "zh_CN");
    let capture_path = game_directory
        .join(BLOADER_PROCESS_CAPTURE_DIRECTORY)
        .join(BLOADER_PROCESS_STDOUT_CAPTURE_NAME);
    assert!(capture_path.is_dir());
    assert!(
        capture_path
            .join(BLOADER_PROCESS_CAPTURE_BLOCKER_MARKER)
            .is_file()
    );

    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

#[test]
fn bloader_0_2_11_restores_debug_console_after_legacy_workaround()
-> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("bloader-stdio-restore");
    std::fs::create_dir_all(&game_directory)?;
    std::fs::write(
        game_directory.join("config.json"),
        format!(
            r#"{{"enable_debug_console":true,"{BLOADER_LEGACY_STDIO_WORKAROUND_KEY}":true,"{BLOADER_LEGACY_ORIGINAL_DEBUG_CONSOLE_KEY}":false}}"#
        ),
    )?;

    assert!(configure_bloader_linux_stdio_workaround(
        &game_directory,
        "0.2.11.0"
    )?);

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(game_directory.join("config.json"))?)?;
    assert_eq!(config["enable_debug_console"], false);
    assert!(config.get(BLOADER_LEGACY_STDIO_WORKAROUND_KEY).is_none());
    assert!(
        config
            .get(BLOADER_LEGACY_ORIGINAL_DEBUG_CONSOLE_KEY)
            .is_none()
    );

    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

#[test]
fn bloader_0_2_11_with_console_keeps_native_capture_enabled()
-> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("bloader-stdio-console");
    std::fs::create_dir_all(&game_directory)?;
    std::fs::write(
        game_directory.join("config.json"),
        br#"{"enable_debug_console":true}"#,
    )?;

    assert!(!configure_bloader_linux_stdio_workaround(
        &game_directory,
        "0.2.11.0"
    )?);
    let capture_path = game_directory
        .join(BLOADER_PROCESS_CAPTURE_DIRECTORY)
        .join(BLOADER_PROCESS_STDOUT_CAPTURE_NAME);
    assert!(!capture_path.exists());

    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

#[test]
fn fixed_bloader_removes_linux_process_capture_blocker() -> Result<(), Box<dyn std::error::Error>> {
    let game_directory = temporary_test_directory("bloader-stdio-fixed");
    std::fs::create_dir_all(&game_directory)?;
    std::fs::write(
        game_directory.join("config.json"),
        br#"{"enable_debug_console":false}"#,
    )?;
    assert!(configure_bloader_linux_stdio_workaround(
        &game_directory,
        "0.2.11.0"
    )?);

    assert!(!configure_bloader_linux_stdio_workaround(
        &game_directory,
        "0.2.12.0"
    )?);
    let capture_path = game_directory
        .join(BLOADER_PROCESS_CAPTURE_DIRECTORY)
        .join(BLOADER_PROCESS_STDOUT_CAPTURE_NAME);
    assert!(!capture_path.exists());

    std::fs::remove_dir_all(game_directory)?;
    Ok(())
}

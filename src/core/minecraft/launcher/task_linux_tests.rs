use super::{
    LaunchRequest, classify_runner_failure, incompatible_proton_prefix_needs_backup,
    normalize_runner_output_line, proton_game_input_is_ready, request_uses_preview_data,
    sanitize_instance_folder_name, wine_z_path,
};
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

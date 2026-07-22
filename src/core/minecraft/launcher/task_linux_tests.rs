use super::{
    classify_runner_failure, normalize_runner_output_line, sanitize_instance_folder_name,
    wine_z_path,
};
use std::path::Path;

#[test]
fn instance_folder_name_cannot_escape_prefix_root() {
    assert_eq!(sanitize_instance_folder_name("."), "default");
    assert_eq!(sanitize_instance_folder_name(".."), "default");
    assert_eq!(sanitize_instance_folder_name("../preview"), "___preview");
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
            .is_some_and(|message| message.contains("LukasPAH Custom"))
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

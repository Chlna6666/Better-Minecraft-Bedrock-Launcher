use tauri::command;

// 全局常量
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_LICENSE: &str = env!("CARGO_PKG_LICENSE");
const TAURI_SDK_VERSION: &str = tauri::VERSION;
const GIT_COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");
const BUILD_TIME: &str = env!("BUILD_TIME");

pub fn get_version() -> &'static str {
    APP_VERSION
}

pub fn get_license() -> &'static str {
    APP_LICENSE
}

pub fn get_tauri_version() -> &'static str {
    TAURI_SDK_VERSION
}

pub fn get_build_info() -> String {
    format!(
        "App Version: {}\nGit Commit: {}\nBuild Time: {}",
        APP_VERSION, GIT_COMMIT_HASH, BUILD_TIME
    )
}

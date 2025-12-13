use crate::utils::app_info::{get_build_info, get_license, get_tauri_version, get_version};
use tauri::command;

#[command]
pub fn get_app_version() -> &'static str {
    get_version()
}

#[command]
pub fn get_app_license() -> &'static str {
    get_license()
}

#[command]
pub fn get_tauri_sdk_version() -> &'static str {
    get_tauri_version()
}

#[command]
pub fn get_full_build_info() -> String {
    get_build_info()
}

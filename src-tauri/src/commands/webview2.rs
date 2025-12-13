use crate::utils::webview2_manager;

#[tauri::command]
/// 前端调用：获取 WebView2 Runtime 版本，
/// 如果未检测到则返回 "Unknown"
pub fn get_webview2_version() -> String {
    webview2_manager::detect_webview2_runtime().unwrap_or_else(|| "Unknown".to_string())
}

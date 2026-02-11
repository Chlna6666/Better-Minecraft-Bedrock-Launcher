use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tracing::error;
use crate::StartupImportState;


pub fn minimize_launcher_window(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        if let Err(e) = window.minimize() {
            error!("最小化窗口失败: {:?}", e);
        }
    }
}

pub fn close_launcher_window(app: &AppHandle) {
    if let Some(window) = app.get_window("main") {
        if let Err(e) = window.close() {
            error!("关闭窗口失败: {:?}", e);
        }
    } else {
        error!("未找到名为 'main' 的窗口");
    }
}


#[tauri::command]
pub fn get_startup_import_file(state: tauri::State<StartupImportState>) -> Option<String> {
    let mut lock = state.0.lock().unwrap();
    lock.take()
}

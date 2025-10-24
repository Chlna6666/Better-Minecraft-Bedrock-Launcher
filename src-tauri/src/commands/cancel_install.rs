// cancel.rs
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::command;
use crate::core::minecraft::appx::extract_zip::CANCEL_EXTRACT;

/// 全局取消标志
pub static CANCEL_INSTALL: Lazy<AtomicBool> = Lazy::new(Default::default);

/// 供前端调用：一律取消
#[command]
pub fn cancel_install() {
    CANCEL_EXTRACT.store(true, Ordering::SeqCst);
}

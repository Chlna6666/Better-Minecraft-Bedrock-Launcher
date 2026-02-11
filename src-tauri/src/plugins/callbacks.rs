use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashSet;
use std::ffi::CStr;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tracing::{debug, error, info, warn};

// 全局 AppHandle
static GLOBAL_APP_HANDLE: OnceCell<AppHandle> = OnceCell::new();

// 🔥 新增：用于记录已知的 (插件名, 事件名) 组合，防止重复刷屏
static KNOWN_EVENTS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

pub fn set_global_app_handle(handle: AppHandle) {
    let _ = GLOBAL_APP_HANDLE.set(handle);
}

// 辅助函数
unsafe fn ptr_to_str<'a>(ptr: *const std::ffi::c_char, default: &'a str) -> &'a str {
    if ptr.is_null() {
        return default;
    }
    CStr::from_ptr(ptr).to_str().unwrap_or(default)
}

// -------- Host 回调函数 --------

pub extern "C" fn host_log(name_ptr: *const std::ffi::c_char, level: u32, msg: *const std::ffi::c_char) {
    unsafe {
        let plugin_name = ptr_to_str(name_ptr, "UnknownPlugin");
        let str_msg = ptr_to_str(msg, "");

        match level {
            0 => debug!("[{}] {}", plugin_name, str_msg),
            1 => info!("[{}] {}", plugin_name, str_msg),
            2 => warn!("[{}] {}", plugin_name, str_msg),
            3 => error!("[{}] {}", plugin_name, str_msg),
            _ => info!("[{}] {}", plugin_name, str_msg),
        }
    }
}

pub extern "C" fn host_send_event(name_ptr: *const std::ffi::c_char, event: *const std::ffi::c_char, payload: *const std::ffi::c_char) {
    unsafe {
        let plugin_name = ptr_to_str(name_ptr, "UnknownPlugin");
        let event_str = ptr_to_str(event, "unknown");
        let payload_str = ptr_to_str(payload, "{}");

        // 🔥 优化逻辑: 只在第一次遇到 "插件+事件" 组合时打印 Debug
        let key = format!("{}::{}", plugin_name, event_str);

        // 使用代码块限制锁的范围
        let should_log = {
            let mut known = KNOWN_EVENTS.lock().unwrap();
            if !known.contains(&key) {
                known.insert(key);
                true // 第一次遇到，允许打印
            } else {
                false // 已经打印过，跳过
            }
        };

        if should_log {
            // 只有第一次会显示这条日志
            debug!("[{}] 首次检测到事件输出: '{}'", plugin_name, event_str);
        }

        // 正常的事件发送逻辑不受影响
        if let Some(handle) = GLOBAL_APP_HANDLE.get() {
            let _ = handle.emit(event_str, payload_str);
        } else {
            warn!("Global AppHandle not set, failed to emit event: {}", event_str);
        }
    }
}
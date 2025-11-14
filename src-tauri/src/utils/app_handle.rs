use once_cell::sync::Lazy;
use std::sync::Mutex;
use tauri::AppHandle;

pub static GLOBAL_APP: Lazy<Mutex<Option<AppHandle>>> = Lazy::new(|| Mutex::new(None));

pub fn set_global_app(app: AppHandle) {
    *GLOBAL_APP.lock().unwrap() = Some(app);
}

use tauri::command;
use tracing::debug;

use crate::tasks::task_manager::{get_snapshot, cancel_task as tm_cancel_task};

#[command]
pub fn get_task_status(task_id: String) -> Result<serde_json::Value, String> {
    match get_snapshot(&task_id) {
        Some(snapshot) => serde_json::to_value(snapshot).map_err(|e| e.to_string()),
        None => Err("task not found".to_string()),
    }
}

#[command]
pub fn cancel_task(task_id: String) -> Result<(), String> {
    debug!("cancel_task command called for {}", task_id);
    tm_cancel_task(&task_id);
    Ok(())
}

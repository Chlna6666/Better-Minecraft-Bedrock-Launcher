use std::process::Command;
use tauri::command;

#[command]
pub async fn open_path(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| format!("failed to open {}: {}", path, e))?;
    Ok(())
}

use crate::plugins::manager::{read_plugin_script_file, scan_plugins, PluginManifest};

#[tauri::command]
pub fn get_plugins_list(app_handle: tauri::AppHandle) -> Vec<PluginManifest> {
    scan_plugins(&app_handle)
}

#[tauri::command]
pub fn load_plugin_script(plugin_name: String, entry_path: String) -> Result<String, String> {
    read_plugin_script_file(&plugin_name, &entry_path).map_err(|e| e.to_string())
}

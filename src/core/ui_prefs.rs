use crate::utils::file_ops;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadUiPrefs {
    pub search_query: String,
    pub channel_filter: String,
    pub page_size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapViewerWindowPrefs {
    pub width: f32,
    pub height: f32,
}

fn download_prefs_path() -> PathBuf {
    file_ops::cache_subdir("download_ui_prefs.json")
}

pub fn load_download_ui_prefs() -> Option<DownloadUiPrefs> {
    load_json_prefs(download_prefs_path())
}

pub fn save_download_ui_prefs(prefs: &DownloadUiPrefs) -> io::Result<()> {
    save_json_prefs(download_prefs_path(), prefs)
}

pub fn load_map_viewer_window_prefs() -> Option<MapViewerWindowPrefs> {
    load_json_prefs(map_viewer_window_prefs_path())
}

pub fn save_map_viewer_window_prefs(prefs: &MapViewerWindowPrefs) -> io::Result<()> {
    save_json_prefs(map_viewer_window_prefs_path(), prefs)
}

fn map_viewer_window_prefs_path() -> PathBuf {
    file_ops::cache_subdir("map_viewer_window_prefs.json")
}

fn load_json_prefs<T: DeserializeOwned>(path: PathBuf) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_json_prefs<T: Serialize>(path: PathBuf, prefs: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string(prefs)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, raw)?;
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(temp_path, path)?;
    Ok(())
}

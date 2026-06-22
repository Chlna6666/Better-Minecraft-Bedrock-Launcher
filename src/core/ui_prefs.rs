use crate::utils::file_ops;
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

fn prefs_path() -> PathBuf {
    file_ops::bmcbl_subdir("cache").join("download_ui_prefs.json")
}

pub fn load_download_ui_prefs() -> Option<DownloadUiPrefs> {
    let path = prefs_path();
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_download_ui_prefs(prefs: &DownloadUiPrefs) -> io::Result<()> {
    let path = prefs_path();
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

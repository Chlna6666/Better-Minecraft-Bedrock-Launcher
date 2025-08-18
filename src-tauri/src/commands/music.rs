use std::fs;
use std::path::PathBuf;
#[tauri::command]
pub fn read_music_directory(directory: &str) -> Result<Vec<String>, String> {
    let path = PathBuf::from(directory).canonicalize().map_err(|e| e.to_string())?;
    let mut files = Vec::new();

    if path.exists() && path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_path = entry.path();

            if let Some(extension) = file_path.extension() {
                if matches!(
                    extension.to_string_lossy().to_ascii_lowercase().as_str(),
                    "m4a" | "mp3" | "wav" | "flac" | "ogg" | "aac"
                ) {
                    let abs_path = file_path.canonicalize().map_err(|e| e.to_string())?;
                    let path_str = abs_path.to_string_lossy().to_string();

                    // 去掉 Windows 的 \\?\ 前缀
                    let clean_path = if path_str.starts_with(r"\\?\") {
                        path_str.trim_start_matches(r"\\?\").to_string()
                    } else {
                        path_str
                    };

                    files.push(clean_path);
                }
            }
        }
    } else {
        return Err("Directory not found or not accessible".into());
    }

    Ok(files)
}


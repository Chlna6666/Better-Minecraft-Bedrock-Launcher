use std::fs;
use std::path::{Path, PathBuf};

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn bmcbl_dir() -> PathBuf {
    exe_dir().join("BMCBL")
}

pub fn bmcbl_subdir<P: AsRef<Path>>(rel: P) -> PathBuf {
    bmcbl_dir().join(rel)
}

pub fn create_initial_directories() {
    let root = bmcbl_dir();
    let dirs = [
        root.clone(),
        bmcbl_subdir("logs"),
        bmcbl_subdir("plugins"),
        bmcbl_subdir("config"),
        bmcbl_subdir("music"),
        bmcbl_subdir("versions"),
        bmcbl_subdir("downloads"),
    ];

    for dir in dirs {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("Failed to create directory '{}': {}", dir.display(), e);
        }
    }
}

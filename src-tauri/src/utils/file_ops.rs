use std::fs;

pub fn create_initial_directories() {
    let dirs = [
        "BMCBL",
        "BMCBL/logs",
        "BMCBL/plugins",
        "BMCBL/config",
        "BMCBL/music",
    ];

    for dir in &dirs {
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("Failed to create directory '{}': {}", dir, e);
        }
    }
}

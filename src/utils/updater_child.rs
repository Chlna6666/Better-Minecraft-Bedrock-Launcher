use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{error, info};

pub fn run_updater_child(src: &Path, dst: &Path, timeout: Duration) -> Result<()> {
    info!(
        "run_updater_child start src='{}' dst='{}' timeout={}s",
        src.display(),
        dst.display(),
        timeout.as_secs()
    );

    if !src.exists() {
        error!("source file does not exist: {}", src.display());
        return Err(anyhow::anyhow!(
            "source file does not exist: {}",
            src.display()
        ));
    }

    let staged_update = stage_update_file(src, dst)?;
    let start = Instant::now();
    loop {
        match std::fs::remove_file(dst) {
            Ok(_) => info!("deleted old target: {}", dst.display()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    info!("target missing, ready to replace");
                } else {
                    error!("cannot delete target: {} ; err={}", dst.display(), e);
                    if start.elapsed() > timeout {
                        return Err(anyhow::anyhow!(
                            "timeout waiting for target to release: {}",
                            e
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }

        if let Err(error) = std::fs::rename(&staged_update, dst) {
            error!("rename staged update failed: {}", error);
            if start.elapsed() > timeout {
                return Err(anyhow::anyhow!("timeout replacing executable: {}", error));
            }
            std::thread::sleep(Duration::from_millis(500));
            continue;
        } else {
            info!(
                "rename replace ok: {} -> {}",
                staged_update.display(),
                dst.display()
            );
        }

        info!("spawning new exe: {}", dst.display());
        let mut command = Command::new(dst);
        if let Some(parent) = dst.parent() {
            command.current_dir(parent);
        }
        match command.spawn() {
            Ok(_) => info!("spawned new program"),
            Err(e) => return Err(anyhow::anyhow!("failed to spawn new exe: {}", e)),
        }

        info!("updater child done");
        return Ok(());
    }
}

fn stage_update_file(src: &Path, dst: &Path) -> Result<PathBuf> {
    let target_dir = dst
        .parent()
        .ok_or_else(|| anyhow::anyhow!("target executable has no parent: {}", dst.display()))?;
    fs::create_dir_all(target_dir)?;

    let target_name = dst
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("target executable has no file name: {}", dst.display()))?
        .to_string_lossy();
    let staged_update = target_dir.join(format!(
        ".{}.update-{}.tmp",
        target_name,
        std::process::id()
    ));

    match fs::remove_file(&staged_update) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow::anyhow!(
                "remove stale staged update {} failed: {}",
                staged_update.display(),
                error
            ));
        }
    }

    let bytes = fs::copy(src, &staged_update).map_err(|error| {
        anyhow::anyhow!(
            "stage update copy failed: {} -> {}: {}",
            src.display(),
            staged_update.display(),
            error
        )
    })?;
    info!(
        "staged update file: {} -> {} ({} bytes)",
        src.display(),
        staged_update.display(),
        bytes
    );

    Ok(staged_update)
}

pub fn clean_old_versions() {
    let downloads_dir = crate::utils::file_ops::downloads_dir();
    if !downloads_dir.exists() {
        return;
    }

    let pid = std::process::id();
    let entries = match fs::read_dir(downloads_dir) {
        Ok(e) => e,
        Err(e) => {
            info!("clean_old_versions: read_dir failed: {}", e);
            return;
        }
    };

    for entry_res in entries {
        let Ok(entry) = entry_res else { continue };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name.starts_with("updater_runner_") && file_name.ends_with(".exe") {
            if let Some(pid_str) = file_name
                .strip_prefix("updater_runner_")
                .and_then(|s| s.strip_suffix(".exe"))
            {
                if pid_str == pid.to_string() {
                    continue;
                }
            }
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !["exe", "msi", "zip", "7z", "bin"].contains(&ext.as_str()) {
            continue;
        }

        match fs::remove_file(&path) {
            Ok(_) => info!("removed old version file: {}", path.display()),
            Err(e) => info!("remove failed: {} ; err={}", path.display(), e),
        }
    }
}

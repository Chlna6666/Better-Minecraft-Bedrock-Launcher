use crate::error::{LevelDbError, Result};
use crate::manifest::Manifest;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const REMOVE_ATTEMPTS: usize = 3;

pub(crate) fn files(root: &Path, manifest: &Manifest) -> Result<Vec<PathBuf>> {
    let live_tables = manifest.table_numbers.iter().copied().collect::<HashSet<_>>();
    let live_logs = [manifest.log_number, manifest.prev_log_number]
        .into_iter()
        .filter(|number| *number != 0)
        .collect::<HashSet<_>>();
    let mut obsolete = Vec::new();
    for entry in fs::read_dir(root)
        .map_err(|error| LevelDbError::io_at("scan obsolete database files", root, error))?
    {
        let entry = entry
            .map_err(|error| LevelDbError::io_at("read obsolete database entry", root, error))?;
        let path = entry.path();
        let Some(number) = file_number(&path) else {
            continue;
        };
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("ldb") if !live_tables.contains(&number) => obsolete.push(path),
            Some("log") if !live_logs.contains(&number) => obsolete.push(path),
            _ => {}
        }
    }
    Ok(obsolete)
}

pub(crate) fn remove_with_retry(paths: &[PathBuf]) {
    for path in paths {
        for attempt in 1..=REMOVE_ATTEMPTS {
            match fs::remove_file(path) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) if attempt != REMOVE_ATTEMPTS => {
                    log::debug!(
                        "retrying obsolete file removal {} after attempt {}: {}",
                        path.display(),
                        attempt,
                        error
                    );
                    std::thread::yield_now();
                }
                Err(error) => log::warn!(
                    "failed to remove obsolete file {} after {} attempts: {}",
                    path.display(),
                    REMOVE_ATTEMPTS,
                    error
                ),
            }
        }
    }
}

fn file_number(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse().ok()
}

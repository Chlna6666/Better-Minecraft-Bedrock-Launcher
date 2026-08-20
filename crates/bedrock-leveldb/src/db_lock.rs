use crate::error::{LevelDbError, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

pub(crate) struct DatabaseLock {
    file: File,
}

impl DatabaseLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        let path = root.join("LOCK");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| LevelDbError::io_at("open database writer lock", &path, error))?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file }),
            Err(error) if is_lock_contended(&error) => {
                Err(LevelDbError::database_locked(path))
            }
            Err(error) => Err(LevelDbError::io_at(
                "acquire database writer lock",
                path,
                error,
            )),
        }
    }
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    if error.kind() == ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // `LockFileEx` reports ERROR_LOCK_VIOLATION rather than WouldBlock.
        return error.raw_os_error() == Some(33);
    }
    #[cfg(not(windows))]
    false
}

impl Drop for DatabaseLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            log::warn!("failed to release database writer lock: {error}");
        }
    }
}

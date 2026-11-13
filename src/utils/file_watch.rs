//! Bounded directory classification for coarse native file notifications.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use notify::{Event, EventKind, event::ModifyKind};

const MAX_DIRECTORIES: usize = 128;

#[derive(Default)]
pub(crate) struct Directories {
    paths: VecDeque<PathBuf>,
    #[cfg(test)]
    probes: usize,
}

impl Directories {
    pub(crate) fn observe(&mut self, event: &Event) {
        if event.need_rescan() {
            self.paths.clear();
        } else if event.kind.is_create()
            || event.kind.is_remove()
            || matches!(event.kind, EventKind::Modify(ModifyKind::Name(_)))
        {
            self.paths
                .retain(|known| !event.paths.iter().any(|path| known.starts_with(path)));
        }
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|known| known == path)
    }

    pub(crate) fn is_directory(&mut self, path: &Path) -> bool {
        if self.contains(path) {
            return true;
        }
        #[cfg(test)]
        {
            self.probes += 1;
        }
        if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
            return false;
        }
        if self.paths.len() == MAX_DIRECTORIES {
            self.paths.pop_front();
        }
        self.paths.push_back(path.to_path_buf());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::CreateKind;

    #[test]
    fn repeated_directory_events_probe_once_and_replacement_is_reclassified() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("content");
        std::fs::create_dir(&path).unwrap();
        let mut directories = Directories::default();
        for _ in 0..1000 {
            assert!(directories.is_directory(&path));
        }
        assert_eq!(directories.probes, 1);
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "now a file").unwrap();
        directories
            .observe(&Event::new(EventKind::Create(CreateKind::File)).add_path(path.clone()));
        assert!(!directories.is_directory(&path));
    }

    #[test]
    fn remembered_directories_are_bounded() {
        let temporary = tempfile::tempdir().unwrap();
        let mut directories = Directories::default();
        for index in 0..MAX_DIRECTORIES + 1 {
            let path = temporary.path().join(index.to_string());
            std::fs::create_dir(&path).unwrap();
            assert!(directories.is_directory(&path));
        }
        assert_eq!(directories.paths.len(), MAX_DIRECTORIES);
        assert!(!directories.contains(&temporary.path().join("0")));
    }
}

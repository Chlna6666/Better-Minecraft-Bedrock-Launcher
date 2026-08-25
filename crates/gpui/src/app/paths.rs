use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::channel::oneshot;

use crate::PathPromptOptions;

use super::App;

impl App {
    /// Return the full path of the current application bundle.
    pub fn app_path(&self) -> Result<PathBuf> {
        self.platform.app_path()
    }

    /// Return the path of an auxiliary executable in the application bundle.
    pub fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.platform.path_for_auxiliary_executable(name)
    }

    /// Prompt the user to select existing paths.
    pub fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.platform.prompt_for_paths(options)
    }

    /// Prompt the user to select a new file path.
    pub fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.platform.prompt_for_new_path(directory, suggested_name)
    }

    /// Reveal a path in the platform file manager.
    pub fn reveal_path(&self, path: &Path) {
        self.platform.reveal_path(path);
    }

    /// Open a path with the system default application.
    pub fn open_with_system(&self, path: &Path) {
        self.platform.open_with_system(path);
    }

    /// Return whether the file picker can mix files and directories.
    pub fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.platform.can_select_mixed_files_and_dirs()
    }
}

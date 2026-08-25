use std::path::PathBuf;

use super::App;

impl App {
    /// Bring the application to the foreground.
    pub fn activate(&self, should_ignore_other_apps: bool) {
        self.platform.activate(should_ignore_other_apps);
    }

    /// Hide the application at the platform level.
    pub fn hide(&self) {
        self.platform.hide();
    }

    /// Hide other applications at the platform level.
    pub fn hide_other_apps(&self) {
        self.platform.hide_other_apps();
    }

    /// Unhide other applications at the platform level.
    pub fn unhide_other_apps(&self) {
        self.platform.unhide_other_apps();
    }

    /// Restart the application.
    pub fn restart(&mut self) {
        self.restart_observers
            .clone()
            .retain(&(), |observer| observer(self));
        self.platform.restart(self.restart_path.take());
    }

    /// Set the executable path used when restarting the application.
    pub fn set_restart_path(&mut self, path: PathBuf) {
        self.restart_path = Some(path);
    }
}

use std::path::{Path, PathBuf};

use smallvec::SmallVec;

use crate::{Menu, MenuItem, OwnedMenu};

use super::App;

impl App {
    /// Sets the menu bar for this application. This will replace any existing menu bar.
    pub fn set_menus(&self, menus: Vec<Menu>) {
        self.platform.set_menus(menus, &self.keymap.borrow());
    }

    /// Gets the menu bar for this application.
    pub fn menus(&self) -> Option<Vec<OwnedMenu>> {
        self.platform.menus()
    }

    /// Sets the right click menu for the app icon in the dock
    pub fn set_dock_menu(&self, menus: Vec<MenuItem>) {
        self.platform.set_dock_menu(menus, &self.keymap.borrow())
    }

    /// Performs the action associated with the given dock menu item, only used on Windows for now.
    pub fn perform_dock_menu_action(&self, action: usize) {
        self.platform.perform_dock_menu_action(action);
    }

    /// Adds given path to the bottom of the list of recent paths for the application.
    /// The list is usually shown on the application icon's context menu in the dock,
    /// and allows to open the recent files via that context menu.
    /// If the path is already in the list, it will be moved to the bottom of the list.
    pub fn add_recent_document(&self, path: &Path) {
        self.platform.add_recent_document(path);
    }

    /// Updates the jump list with the updated list of recent paths for the application, only used on Windows for now.
    /// Note that this also sets the dock menu on Windows.
    pub fn update_jump_list(
        &self,
        menus: Vec<MenuItem>,
        entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Vec<SmallVec<[PathBuf; 2]>> {
        self.platform.update_jump_list(menus, entries)
    }
}

use std::time::Instant;

use collections::FxHashMap;
use itertools::Itertools;

use crate::{AnyWindowHandle, Global, SharedString, WindowId};

use super::App;

#[doc(hidden)]
#[derive(Clone, PartialEq, Eq)]
pub struct SystemWindowTab {
    pub id: WindowId,
    pub title: SharedString,
    pub handle: AnyWindowHandle,
    pub last_active_at: Instant,
}

impl SystemWindowTab {
    /// Create a new instance of the window tab.
    pub fn new(title: SharedString, handle: AnyWindowHandle) -> Self {
        Self {
            id: handle.id,
            title,
            handle,
            last_active_at: Instant::now(),
        }
    }
}

/// A controller for managing window tabs.
#[derive(Default)]
pub struct SystemWindowTabController {
    visible: Option<bool>,
    tab_groups: FxHashMap<usize, Vec<SystemWindowTab>>,
}

impl Global for SystemWindowTabController {}

impl SystemWindowTabController {
    /// Create a new instance of the window tab controller.
    pub fn new() -> Self {
        Self {
            visible: None,
            tab_groups: FxHashMap::default(),
        }
    }

    /// Initialize the global window tab controller.
    pub fn init(cx: &mut App) {
        cx.set_global(SystemWindowTabController::new());
    }

    /// Get all tab groups.
    pub fn tab_groups(&self) -> &FxHashMap<usize, Vec<SystemWindowTab>> {
        &self.tab_groups
    }

    /// Get the next tab group window handle.
    pub fn next_tab_group_window(cx: &mut App, id: WindowId) -> Option<&AnyWindowHandle> {
        let controller = cx.global::<SystemWindowTabController>();
        let current_group = controller
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| group));

        let current_group = current_group?;
        let group_ids: Vec<_> = controller.tab_groups.keys().collect();
        let idx = group_ids.iter().position(|g| *g == current_group)?;
        let next_idx = (idx + 1) % group_ids.len();

        controller
            .tab_groups
            .get(group_ids[next_idx])
            .and_then(|tabs| {
                tabs.iter()
                    .max_by_key(|tab| tab.last_active_at)
                    .or_else(|| tabs.first())
                    .map(|tab| &tab.handle)
            })
    }

    /// Get the previous tab group window handle.
    pub fn previous_tab_group_window(cx: &mut App, id: WindowId) -> Option<&AnyWindowHandle> {
        let controller = cx.global::<SystemWindowTabController>();
        let current_group = controller
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| group));

        let current_group = current_group?;
        let group_ids: Vec<_> = controller.tab_groups.keys().collect();
        let idx = group_ids.iter().position(|g| *g == current_group)?;
        let prev_idx = if idx == 0 {
            group_ids.len() - 1
        } else {
            idx - 1
        };

        controller
            .tab_groups
            .get(group_ids[prev_idx])
            .and_then(|tabs| {
                tabs.iter()
                    .max_by_key(|tab| tab.last_active_at)
                    .or_else(|| tabs.first())
                    .map(|tab| &tab.handle)
            })
    }

    /// Get all tabs in the same window.
    pub fn tabs(&self, id: WindowId) -> Option<&Vec<SystemWindowTab>> {
        let tab_group = self
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| *group))?;

        self.tab_groups.get(&tab_group)
    }

    /// Initialize the visibility of the system window tab controller.
    pub fn init_visible(cx: &mut App, is_visible: bool) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        if controller.visible.is_none() {
            controller.visible = Some(is_visible);
        }
    }

    /// Get the visibility of the system window tab controller.
    pub fn is_visible(&self) -> bool {
        self.visible.unwrap_or(false)
    }

    /// Set the visibility of the system window tab controller.
    pub fn set_visible(cx: &mut App, is_visible: bool) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        controller.visible = Some(is_visible);
    }

    /// Update the last active of a window.
    pub fn update_last_active(cx: &mut App, id: WindowId) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        for windows in controller.tab_groups.values_mut() {
            for tab in windows.iter_mut() {
                if tab.id == id {
                    tab.last_active_at = Instant::now();
                }
            }
        }
    }

    /// Update the position of a tab within its group.
    pub fn update_tab_position(cx: &mut App, id: WindowId, ix: usize) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        for windows in controller.tab_groups.values_mut() {
            if let Some(current_pos) = windows.iter().position(|tab| tab.id == id) {
                if ix < windows.len() && current_pos != ix {
                    let window_tab = windows.remove(current_pos);
                    windows.insert(ix, window_tab);
                }
                break;
            }
        }
    }

    /// Update the title of a tab.
    pub fn update_tab_title(cx: &mut App, id: WindowId, title: SharedString) {
        let controller = cx.global::<SystemWindowTabController>();
        let tab = controller
            .tab_groups
            .values()
            .flat_map(|windows| windows.iter())
            .find(|tab| tab.id == id);

        if tab.map_or(true, |t| t.title == title) {
            return;
        }

        let controller = cx.global_mut::<SystemWindowTabController>();
        for windows in controller.tab_groups.values_mut() {
            for tab in windows.iter_mut() {
                if tab.id == id {
                    tab.title = title;
                    return;
                }
            }
        }
    }

    /// Insert a tab into a tab group.
    pub fn add_tab(cx: &mut App, id: WindowId, tabs: Vec<SystemWindowTab>) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tab) = tabs.clone().into_iter().find(|tab| tab.id == id) else {
            return;
        };

        let expected_tab_ids: Vec<_> = tabs
            .iter()
            .filter(|tab| tab.id != id)
            .map(|tab| tab.id)
            .sorted()
            .collect();

        let mut tab_group_id = None;
        for (group_id, group_tabs) in &controller.tab_groups {
            let tab_ids: Vec<_> = group_tabs.iter().map(|tab| tab.id).sorted().collect();
            if tab_ids == expected_tab_ids {
                tab_group_id = Some(*group_id);
                break;
            }
        }

        if let Some(tab_group_id) = tab_group_id {
            if let Some(tabs) = controller.tab_groups.get_mut(&tab_group_id) {
                tabs.push(tab);
            }
        } else {
            let new_group_id = controller.tab_groups.len();
            controller.tab_groups.insert(new_group_id, tabs);
        }
    }

    /// Remove a tab from a tab group.
    pub fn remove_tab(cx: &mut App, id: WindowId) -> Option<SystemWindowTab> {
        let controller = cx.global_mut::<SystemWindowTabController>();
        let mut removed_tab = None;

        controller.tab_groups.retain(|_, tabs| {
            if let Some(pos) = tabs.iter().position(|tab| tab.id == id) {
                removed_tab = Some(tabs.remove(pos));
            }
            !tabs.is_empty()
        });

        removed_tab
    }

    /// Move a tab to a new tab group.
    pub fn move_tab_to_new_window(cx: &mut App, id: WindowId) {
        let removed_tab = Self::remove_tab(cx, id);
        let controller = cx.global_mut::<SystemWindowTabController>();

        if let Some(tab) = removed_tab {
            let new_group_id = controller.tab_groups.keys().max().map_or(0, |k| k + 1);
            controller.tab_groups.insert(new_group_id, vec![tab]);
        }
    }

    /// Merge all tab groups into a single group.
    pub fn merge_all_windows(cx: &mut App, id: WindowId) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        let Some(initial_tabs) = controller.tabs(id) else {
            return;
        };

        let mut all_tabs = initial_tabs.clone();
        for tabs in controller.tab_groups.values() {
            all_tabs.extend(
                tabs.iter()
                    .filter(|tab| !initial_tabs.contains(tab))
                    .cloned(),
            );
        }

        controller.tab_groups.clear();
        controller.tab_groups.insert(0, all_tabs);
    }

    /// Selects the next tab in the tab group in the trailing direction.
    pub fn select_next_tab(cx: &mut App, id: WindowId) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tabs) = controller.tabs(id) else {
            return;
        };

        let current_index = tabs.iter().position(|tab| tab.id == id).unwrap();
        let next_index = (current_index + 1) % tabs.len();

        let _ = &tabs[next_index].handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    }

    /// Selects the previous tab in the tab group in the leading direction.
    pub fn select_previous_tab(cx: &mut App, id: WindowId) {
        let controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tabs) = controller.tabs(id) else {
            return;
        };

        let current_index = tabs.iter().position(|tab| tab.id == id).unwrap();
        let previous_index = if current_index == 0 {
            tabs.len() - 1
        } else {
            current_index - 1
        };

        let _ = &tabs[previous_index].handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    }
}

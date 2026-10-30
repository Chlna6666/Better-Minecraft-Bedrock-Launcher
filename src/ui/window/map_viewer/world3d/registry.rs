//! UI-thread registry of live 3D world editors, keyed by canonical world identity.

use super::types::World3dWorldKey;
use crate::ui::window::map_viewer::world3d::editor::World3dEditorView;
use gpui::{AnyWindowHandle, WeakEntity};
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Clone)]
pub(super) struct World3dRegistryEntry {
    pub(super) session_id: u64,
    pub(super) editor: WeakEntity<World3dEditorView>,
    pub(super) window: Option<AnyWindowHandle>,
    pub(super) world_name: String,
}

thread_local! {
    static WORLD3D_EDITORS: RefCell<BTreeMap<World3dWorldKey, World3dRegistryEntry>> =
        RefCell::new(BTreeMap::new());
}

pub(super) fn register_world3d_editor(key: World3dWorldKey, entry: World3dRegistryEntry) {
    WORLD3D_EDITORS.with(|editors| {
        editors.borrow_mut().insert(key, entry);
    });
}

pub(super) fn set_world3d_editor_window(key: &World3dWorldKey, window: AnyWindowHandle) {
    WORLD3D_EDITORS.with(|editors| {
        if let Some(entry) = editors.borrow_mut().get_mut(key) {
            entry.window = Some(window);
        }
    });
}

pub(super) fn unregister_world3d_editor(key: &World3dWorldKey, session_id: u64) {
    WORLD3D_EDITORS.with(|editors| {
        let remove = editors
            .borrow()
            .get(key)
            .is_some_and(|entry| entry.session_id == session_id);
        if remove {
            editors.borrow_mut().remove(key);
        }
    });
}

pub(super) fn world3d_editor_for(key: &World3dWorldKey) -> Option<World3dRegistryEntry> {
    WORLD3D_EDITORS.with(|editors| editors.borrow().get(key).cloned())
}

#[must_use]
pub(super) fn world3d_editor_open(key: &World3dWorldKey) -> bool {
    world3d_editor_for(key).is_some_and(|entry| entry.editor.upgrade().is_some())
}

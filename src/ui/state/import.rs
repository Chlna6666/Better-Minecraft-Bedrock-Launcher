use gpui::{App, AppContext as _, BorrowAppContext as _, Entity, Global, SharedString, Window};

use crate::launch::ImportLaunchContext;
use crate::ui::components::modal::ModalDismissHandle;
use crate::ui::window::import::ImportWindowView;
use crate::ui::window::import::{ImportWindowTarget, view::ImportPresentation};

#[derive(Default)]
pub struct ImportCompletionState {
    pub generation: u64,
    pub version_folder: Option<SharedString>,
}

impl Global for ImportCompletionState {}

#[derive(Clone)]
pub struct ImportOverlayEntry {
    pub view: Entity<ImportWindowView>,
    pub dismiss: ModalDismissHandle,
}

#[derive(Default)]
pub struct ImportOverlayState {
    pub active: Option<ImportOverlayEntry>,
}

impl Global for ImportOverlayState {}

pub fn publish_import_completion(version_folder: SharedString, cx: &mut App) {
    cx.update_global(|state: &mut ImportCompletionState, _cx| {
        state.generation = state.generation.wrapping_add(1);
        state.version_folder = Some(version_folder);
    });
}

pub fn show_import_overlay(
    import_context: ImportLaunchContext,
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| {
        ImportWindowView::new(
            import_context,
            target,
            ImportPresentation::Overlay,
            window,
            cx,
        )
    });
    let entry = ImportOverlayEntry {
        view,
        dismiss: ModalDismissHandle::new(),
    };
    cx.update_global(|state: &mut ImportOverlayState, cx| {
        state.active = Some(entry);
        cx.refresh_windows();
    });
}

pub fn dismiss_import_overlay(cx: &mut App) {
    let dismiss = cx
        .try_global::<ImportOverlayState>()
        .and_then(|state| state.active.as_ref().map(|entry| entry.dismiss.clone()));
    if let Some(dismiss) = dismiss {
        dismiss.dismiss(cx);
    }
}

pub fn clear_import_overlay(cx: &mut App) {
    cx.update_global(|state: &mut ImportOverlayState, cx| {
        state.active = None;
        cx.refresh_windows();
    });
}

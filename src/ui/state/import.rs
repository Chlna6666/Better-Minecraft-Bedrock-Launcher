use gpui::{App, AppContext as _, BorrowAppContext as _, Entity, Global, SharedString, Window};
use std::path::PathBuf;

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
    pub owner_window_id: u64,
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
    show_import_overlay_batch(vec![import_context.file_path], target, window, cx);
}

pub fn show_import_overlay_batch(
    file_paths: Vec<PathBuf>,
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    if file_paths.is_empty() {
        return;
    }
    let owner_window_id = window.window_handle().window_id().as_u64();
    // A dropdown from the page underneath must not survive into a newly opened Import modal,
    // but dropdowns belonging to other windows are independent.
    cx.update_global(
        |overlay: &mut crate::ui::components::dropdown::DropdownOverlayState, _cx| {
            overlay.clear_for_window(owner_window_id);
        },
    );

    // Some native backends can deliver one OS multi-selection as several logical drops.
    // Merge those paths into the already mounted import view instead of replacing the active
    // overlay and losing the files parsed by the previous event.
    let active_view = cx.try_global::<ImportOverlayState>().and_then(|state| {
        state.active.as_ref().and_then(|entry| {
            (entry.owner_window_id == owner_window_id).then(|| entry.view.clone())
        })
    });
    if let Some(active_view) = active_view {
        let _ = active_view.update(cx, |view, cx| view.append_paths(file_paths, cx));
        return;
    }

    let view = cx.new(|cx| {
        ImportWindowView::new_batch(
            file_paths,
            target,
            ImportPresentation::Overlay,
            window,
            cx,
        )
    });
    let entry = ImportOverlayEntry {
        view,
        dismiss: ModalDismissHandle::new(),
        owner_window_id,
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
    let owner_window_id = cx
        .try_global::<ImportOverlayState>()
        .and_then(|state| state.active.as_ref().map(|entry| entry.owner_window_id));
    if let Some(owner_window_id) = owner_window_id {
        cx.update_global(
            |overlay: &mut crate::ui::components::dropdown::DropdownOverlayState, _cx| {
                overlay.clear_for_window(owner_window_id);
            },
        );
    }
    cx.update_global(|state: &mut ImportOverlayState, cx| {
        state.active = None;
        cx.refresh_windows();
    });
}

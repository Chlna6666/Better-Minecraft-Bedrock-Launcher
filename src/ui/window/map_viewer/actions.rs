use super::model::ViewerMode;
use super::state::MapViewerBottomTab;
use gpui::{App, KeyBinding, actions};

actions!(
    map_viewer,
    [
        MapViewerCopyChunks,
        MapViewerExportChunksImage,
        MapViewerStartPastePreview,
        MapViewerRotatePastePreviewClockwise,
        MapViewerRotatePastePreviewCounterClockwise,
        MapViewerConfirmPastePreview,
        MapViewerCancelPastePreview,
        MapViewerUndoEdit,
        MapViewerRedoEdit,
        MapViewerOpenHistory,
        MapViewerCreateBackup
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-c", MapViewerCopyChunks, Some("MapViewer")),
        KeyBinding::new(
            "ctrl-shift-e",
            MapViewerExportChunksImage,
            Some("MapViewer"),
        ),
        KeyBinding::new("ctrl-v", MapViewerStartPastePreview, Some("MapViewer")),
        KeyBinding::new("r", MapViewerRotatePastePreviewClockwise, Some("MapViewer")),
        KeyBinding::new(
            "shift-r",
            MapViewerRotatePastePreviewCounterClockwise,
            Some("MapViewer"),
        ),
        KeyBinding::new("enter", MapViewerConfirmPastePreview, Some("MapViewer")),
        KeyBinding::new("escape", MapViewerCancelPastePreview, Some("MapViewer")),
        KeyBinding::new("ctrl-z", MapViewerUndoEdit, Some("MapViewer")),
        KeyBinding::new("ctrl-shift-z", MapViewerRedoEdit, Some("MapViewer")),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MapViewerAction {
    SetMode(ViewerMode),
    StepY(i32),
    ZoomBy(f32),
    ImportStructureFile,
    ToggleTopMore,
    ToggleLeftPanel,
    ToggleBottomPanel,
    SetBottomTab(MapViewerBottomTab),
    OpenRightNbt,
    OpenRightPreview3d,
    CloseMenus,
}

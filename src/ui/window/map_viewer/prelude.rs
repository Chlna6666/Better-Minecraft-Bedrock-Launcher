// Shared imports for map_viewer internal modules.
pub(super) use super::actions::{
    MapViewerAction, MapViewerCancelPastePreview, MapViewerConfirmPastePreview,
    MapViewerCopyChunks, MapViewerCreateBackup, MapViewerExportChunksImage, MapViewerOpenHistory,
    MapViewerRedoEdit, MapViewerRotatePastePreviewClockwise,
    MapViewerRotatePastePreviewCounterClockwise, MapViewerStartPastePreview, MapViewerUndoEdit,
};
pub(super) use super::canvas::{
    MapCanvasAction, MapCanvasSnapshot, MapCanvasView, ScreenPaintImage, TilePaintSnapshot,
    TilePaintSnapshotPatch, build_tile_paint_snapshot, patch_tile_paint_snapshot,
    screen_image_viewports_transformable, selection_cursor_for_target,
    take_map_tile_paint_resources_unavailable,
};
pub(super) use super::layout::{
    CHROME_ELEVATED_ALPHA, CHROME_HAIRLINE_ALPHA, CHROME_ICON_SIZE, CHROME_SECTION_GAP,
    CHROME_SURFACE_ALPHA, CHROME_TAB_ICON_SIZE, CHROME_TOOLBAR_ICON_SIZE, IDE_LEFT_DOCK_WIDTH,
    IDE_STATUS_BAR_HEIGHT, center_stage_rect_for_layout,
};
pub(super) use super::map_history::{
    MapHistoryApplyOutcome, MapHistoryApplyProgress, MapHistoryCaptureSpec, MapHistoryEntry,
    MapHistoryEntryKind, MapHistoryEntryStatus, MapHistoryState, apply_redo_with_progress,
    apply_undo_with_progress, capture_before, capture_before_with_progress,
    capture_before_with_world_and_progress, complete_after, complete_after_with_progress,
    complete_after_with_world_and_progress, complete_failed, create_restore_protection_point,
    history_dir_for_world, list_history, restore_history_entry_with_progress,
};
pub(super) use super::menu_overlay::{MapMenuOverlaySnapshot, MapMenuOverlayView};
pub(super) use super::model::ChunkTransferProgress;
pub(super) use super::preview_3d::{
    Preview3dBuildStatus, Preview3dCamera, Preview3dDragMode, Preview3dDragState, Preview3dMesh,
    Preview3dModelRotation, Preview3dSelectionSignature, Preview3dSource, Preview3dState,
    Preview3dStatus, load_preview_3d_mesh_blocking_incremental,
    load_preview_3d_mesh_blocking_incremental_with_block_models,
    load_preview_3d_mesh_from_copied_chunk_blocking,
    load_preview_3d_mesh_from_mcstructure_blocking, preview_3d_bounds_depth,
    preview_3d_bounds_width,
};
pub(super) use super::preview_3d_obj::export_preview_3d_obj_with_materials_with_progress;
pub(super) use super::selection::{
    ChunkSelection, ExistingSelectionTarget, RightSelectionDrag, RightSelectionIntent,
    RightSelectionReleaseAction, SelectionPointerButton, SelectionResizeHandle,
    SelectionScreenBounds, chunk_from_block, existing_selection_target, right_selection_moved,
    right_selection_release_action,
};
pub(super) use super::state::{
    BOTTOM_PANEL_MIN_HEIGHT, DbTreeNode, DbTreeNodeKind, DbTreeState, DockDrag, DockDragState,
    EditorDocument, MIN_CENTER_HEIGHT, MIN_CENTER_WIDTH, MapViewerBottomTab, MapViewerRightPanel,
    MapViewerUiState, chunk_tree_nodes_for_tile, clamp_bottom_panel_height,
    clamp_right_panel_width,
};
pub(super) use super::tool_stripe::{MapToolStripeSnapshot, MapToolStripeView};
pub(super) use super::top_bar::{MapTopBarSnapshot, MapTopBarView};
pub(super) use crate::core::minecraft::entity_avatar::load_generated_entity_avatars_rgba;
pub(super) use crate::core::minecraft::map_info_cache::{
    MapInfoOverlaySnapshot, invalidate_map_info_tiles_for_chunks,
    load_cached_map_info_tiles_blocking, load_map_info_tiles_blocking,
};
pub(super) use crate::tasks::task_manager::{self, TaskSnapshot};
pub(super) use crate::ui::animation::request_animation_frame_if;
pub(super) use crate::ui::components::code_editor::{
    CodeEditor, CodeEditorEvent, CodeEditorLanguage, CodeEditorState,
};
pub(super) use crate::ui::components::context_menu::{
    ContextMenu, ContextMenuAnchor, ContextMenuEntry, ContextMenuGroup, ContextMenuItem,
    place_context_menu_at_anchor,
};
pub(super) use crate::ui::components::input::{Input, InputEvent, InputSize, InputState};
pub(super) use crate::ui::components::scroll::ScrollableElement as _;
pub(super) use crate::ui::components::split_pane::{SplitPaneAxis, split_handle, splitter_line};
pub(super) use crate::ui::components::toast;
pub(super) use crate::ui::state::theme::ThemeState;
pub(super) use crate::ui::theme::colors::{
    DarkColors, LightColors, ThemeColors, lerp_theme_colors,
};
pub(super) use crate::ui::views::manage::state::{ManageAssetEntry, ManagedVersionEntry};
pub(super) use crate::utils::file_ops;
pub(super) use crate::utils::file_picker::{
    pick_file_path_with_filter, pick_save_path_with_filter,
};
pub(super) use bedrock_render::{
    AtlasRenderOptions, BlockBoundaryRenderOptions, BlockVolumeRenderOptions, ChunkBounds,
    ChunkPos, ChunkRegion, DEFAULT_PALETTE_VERSION, DecodedTileImage, Dimension, ImageFormat,
    MapRenderSession, MapRenderSessionConfig, NbtTag, PlannedTile, RENDERER_CACHE_VERSION,
    RegionLayout, RenderBackend, RenderCachePolicy, RenderCancelFlag, RenderCpuPipelineOptions,
    RenderDiagnostics, RenderExecutionProfile, RenderGpuBackend, RenderGpuFallbackPolicy,
    RenderGpuOptions, RenderGpuPipelineLevel, RenderJob, RenderLayout, RenderMemoryBudget,
    RenderMode, RenderOptions, RenderPalette, RenderPipelineStats, RenderTaskControl,
    RenderThreadingOptions, RenderTileOutputOptions, RenderTilePriority, ResolvedRenderBackend,
    SurfaceRenderOptions, TerrainLightingOptions, TileCoord, TileManifestProbeRequest,
    TilePixelFormat, TileReadySource, TileStreamEventV2,
    editor::{
        ActorRecord, ActorSource, Biome3d, BlockEntityRecord, GlobalRecordKind,
        HardcodedSpawnAreaKind, HeightMap2d, MapEditInvalidation, MapRecordId, MapWorldEditor,
        ParsedBiomeStorage, ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData,
        WorldScanOptions,
    },
};
pub(super) use bedrock_world::{
    ActorDigestKey, BedrockWorld, BlockPos, CancelFlag, ChunkDetail, ChunkKey, ChunkRecord,
    ChunkRecordTag, ChunkVersion, ParsedBlockEntity, PlayerData, PlayerId, RegionOverlayQuery,
    RegionOverlayQueryOptions, SelectionStats, SlimeChunkBounds, SlimeChunkWindow, SlimeWindowSize,
    VillageOverlay, VillageOverlayIndex, WriteGuard, delete_chunks_blocking, is_slime_chunk,
    query_block_tip_blocking, query_chunk_detail_blocking,
    query_region_overlays_blocking_with_control, query_selection_stats_blocking,
    query_slime_chunk_windows,
};
pub(super) use bytes::Bytes;
pub(super) use futures::channel::mpsc::{UnboundedSender, unbounded};
pub(super) use futures_util::StreamExt as _;
pub(super) use gpui::prelude::FluentBuilder as _;
pub(super) use gpui::*;
pub(super) use rustc_hash::FxHashMap as HashMap;
pub(super) use serde::{Deserialize, Serialize};
pub(super) use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub(super) use std::hash::Hash;
pub(super) use std::path::{Path, PathBuf};
pub(super) use std::sync::{Arc, Mutex, OnceLock};
pub(super) use std::time::{Duration, Instant};

//! 2D map document integration for the independent 3D editor.
//!
//! The map document owns the single world edit transaction/history and the shared
//! selection. The 3D window sends navigation and edit requests here; the map window
//! publishes its selection and receives a read-only camera snapshot for the overlay.

use super::super::model::{MapViewerWindowView, PasteTransform};
use super::super::prelude::*;
use super::super::selection::ChunkSelection;
use super::editor::{World3dEditorInit, World3dEditorView, open_world3d_editor_window};
use super::import::{World3dImportAction, World3dImportPreview};
use super::registry::world3d_editor_for;
use super::types::{
    CameraOverlayMode, TeleportHeightMode, World3dBlockEditKind, World3dBlockEditRequest,
    World3dCameraSnapshot, World3dEditOutcome, World3dSelection, World3dWorldKey,
    WorldNavigationTarget,
};
use ::bedrock_world::block::BlockState;
use ::bedrock_world::chunk::Dimension as BwDimension;
use ::bedrock_world::editor::{BlockEdit, BlockEditOptions, apply_block_edits};
use ::bedrock_world::level::read_level_dat_document;
use ::bedrock_world::world::{OpenOptions, World as BwWorld};
use ::bedrock_world::{BlockPos, ChunkPos as BwChunkPos, NbtTag, SubChunkDecodeMode, WriteGuard};
use std::path::Path;
use std::sync::Arc;

mod edit;
mod selection;

/// Map-window side state of the linked independent 3D editor.
#[derive(Default)]
pub(crate) struct World3dMapLink {
    pub(crate) world_key: Option<World3dWorldKey>,
    pub(crate) editor: Option<WeakEntity<World3dEditorView>>,
    pub(crate) window: Option<AnyWindowHandle>,
    pub(crate) camera: Option<World3dCameraSnapshot>,
    pub(crate) overlay_mode: CameraOverlayMode,
    pub(crate) overlay_hidden_by_user: bool,
    pub(crate) nav_generation: u64,
    pub(crate) opening: bool,
    pub(crate) follow_3d: bool,
    pub(crate) last_follow_center: Option<(i32, i32)>,
    pub(crate) import_mesh_origin_y: Option<i32>,
    import_generation: u64,
    copy_in_progress: bool,
}

impl World3dMapLink {
    pub(crate) fn try_start_copy(&mut self) -> bool {
        !std::mem::replace(&mut self.copy_in_progress, true)
    }

    pub(crate) fn finish_copy(&mut self) {
        self.copy_in_progress = false;
    }

    pub(crate) fn next_navigation_generation(&mut self) -> u64 {
        self.nav_generation = self.nav_generation.saturating_add(1);
        self.nav_generation
    }

    pub(crate) fn is_open(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.upgrade().is_some())
    }

    fn clear(&mut self) {
        self.world_key = None;
        self.editor = None;
        self.window = None;
        self.camera = None;
        self.last_follow_center = None;
        self.overlay_mode = CameraOverlayMode::Hidden;
        self.import_generation = self.import_generation.saturating_add(1);
    }
}

impl MapViewerWindowView {
    pub(crate) fn report_clipboard_busy(&mut self, cx: &mut Context<Self>) {
        let message = "已有复制任务，请等待完成后再复制";
        self.status = message.into();
        if let Some(editor) = self.world3d.editor.clone() {
            cx.defer(move |cx| {
                let _ = editor.update(cx, |editor, cx| {
                    editor.set_clipboard_copy_status(message.into(), false, cx);
                });
            });
        }
        cx.notify();
    }

    pub(crate) fn copy_world3d_selection(
        &mut self,
        selection: World3dSelection,
        cx: &mut Context<Self>,
    ) {
        if self.map_write_busy() {
            self.status = SharedString::from("地图正在写入，暂不能复制 3D 选区");
            cx.notify();
            return;
        }
        let size = selection.size();
        let Ok(size) = bedrock_world::McStructureSize::new(size[0], size[1], size[2]) else {
            self.status = SharedString::from("3D 选区尺寸无效");
            cx.notify();
            return;
        };
        if selection.volume() > 1_048_576 {
            self.status = SharedString::from("3D 选区超过 1048576 个方块，请缩小选区");
            cx.notify();
            return;
        }
        if !self.world3d.try_start_copy() {
            self.report_clipboard_busy(cx);
            return;
        }
        let source = ChunkPos {
            x: selection.min[0].div_euclid(16),
            z: selection.min[2].div_euclid(16),
            dimension: selection.dimension,
        };
        let path = self.world_path.clone();
        let editor = self.world3d.editor.clone();
        self.status = SharedString::from("正在复制 3D 选区...");
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let world =
                        bedrock_world::World::open(&path, bedrock_world::OpenOptions::default())
                            .map_err(|error| error.to_string())?;
                    let structure = bedrock_world::McStructureFile::from_world_region(
                        &world,
                        selection.dimension,
                        selection.min[0],
                        selection.min[1],
                        selection.min[2],
                        size,
                    )
                    .map_err(|error| error.to_string())?;
                    super::super::mcstructure::structure_as_copied_chunk(
                        Arc::new(structure),
                        source,
                        selection.min[1],
                    )
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                this.world3d.finish_copy();
                let (message, ok) = match result {
                    Ok(import) => {
                        this.professional.copied_chunk = Some(import.copied_chunk);
                        this.professional.imported_structure = Some(import.imported_structure);
                        this.professional.imported_region_package = false;
                        this.generator.preview_result_active = false;
                        this.image_generator.preview_result_active = false;
                        this.professional.copied_chunk_preview_images = import.preview_images;
                        this.clear_paste_preview_state(cx);
                        this.invalidate_preview_3d_mesh();
                        (
                            format!(
                                "已复制 3D 选区 {}×{}×{}；可在 3D 调整高度后粘贴",
                                size.x, size.y, size.z
                            ),
                            true,
                        )
                    }
                    Err(error) => (format!("复制 3D 选区失败：{error}"), false),
                };
                this.status = SharedString::from(message.clone());
                if let Some(editor) = editor {
                    cx.defer(move |cx| {
                        let _ = editor.update(cx, |view, cx| {
                            view.set_clipboard_copy_status(message, ok, cx)
                        });
                    });
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(crate) fn world3d_editor_is_open(&self) -> bool {
        self.world3d.is_open()
    }

    fn current_world3d_key(&self) -> World3dWorldKey {
        World3dWorldKey::from_path(&self.world_path)
    }

    fn focus_world3d_editor_window(&mut self, cx: &mut Context<Self>) {
        if let Some(window) = self.world3d.window {
            let _ = window.update(cx, |_root, window, _cx| {
                window.activate_window();
            });
        }
    }

    fn attach_world3d_editor_handle(
        &mut self,
        key: World3dWorldKey,
        editor: WeakEntity<World3dEditorView>,
        window: Option<AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        self.world3d.world_key = Some(key);
        self.world3d.editor = Some(editor);
        self.world3d.window = window;
        if self.world3d.overlay_mode == CameraOverlayMode::Hidden
            && !self.world3d.overlay_hidden_by_user
        {
            self.world3d.overlay_mode = CameraOverlayMode::PositionFrustum;
        }
        self.push_world3d_overlay(cx);
        self.sync_world3d_import_preview(cx);
        self.sync_world3d_chunk_clipboard(cx);
        self.ensure_world3d_import_preview(cx);
        cx.notify();
    }

    pub(crate) fn ensure_world3d_import_preview(&mut self, cx: &mut Context<Self>) {
        if self.world3d.is_open()
            && self.professional.copied_chunk.is_some()
            && self.professional.paste_preview.is_some()
            && (self.preview_3d.source != Preview3dSource::ImportPreview
                || (self.preview_3d.mesh.is_none() && !self.preview_3d.render_in_flight))
        {
            self.refresh_import_preview_3d(cx);
        }
    }

    pub(crate) fn sync_world3d_chunk_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.world3d.editor.clone() else {
            return;
        };
        let count = self
            .professional
            .copied_chunk
            .as_ref()
            .map_or(0, |copy| copy.chunk_count());
        cx.defer(move |cx| {
            let _ = editor.update(cx, |view, cx| view.set_chunk_clipboard_count(count, cx));
        });
    }

    pub(crate) fn copy_world3d_chunks(&mut self, chunks: Vec<ChunkPos>, cx: &mut Context<Self>) {
        let Some(first) = chunks.first() else {
            return;
        };
        if self.map_write_busy() {
            self.status = SharedString::from("地图正在写入，暂不能复制区块");
            cx.notify();
            return;
        }
        if first.dimension != self.dimension {
            self.set_dimension(first.dimension, cx);
        }
        self.copy_chunks(chunks, cx);
    }

    pub(crate) fn sync_world3d_import_preview(&mut self, cx: &mut Context<Self>) {
        self.world3d.import_generation = self.world3d.import_generation.saturating_add(1);
        let generation = self.world3d.import_generation;
        let Some(editor) = self.world3d.editor.clone() else {
            return;
        };
        let preview = self
            .professional
            .paste_preview
            .as_ref()
            .zip(self.preview_3d.mesh.as_ref())
            .filter(|(placement, _)| {
                self.preview_3d.source == Preview3dSource::ImportPreview
                    && placement.target_anchor.dimension == self.dimension
            })
            .and_then(|(placement, mesh)| {
                if let Some(import) = self.professional.imported_structure.as_ref() {
                    (import.source_anchor == placement.source_anchor).then(|| {
                        World3dImportPreview {
                            mesh: mesh.clone(),
                            source_anchor: import.source_anchor,
                            target_anchor: placement.target_anchor,
                            size: [import.structure.size.x, import.structure.size.z],
                            height: import.structure.size.y,
                            mesh_origin_y: self
                                .world3d
                                .import_mesh_origin_y
                                .unwrap_or(import.origin_y),
                            origin_y: import.origin_y,
                            transform: placement.transform,
                            is_structure: true,
                            is_writing: placement.is_writing(),
                            chunk_count: self
                                .professional
                                .copied_chunk
                                .as_ref()
                                .map_or(0, |copy| copy.chunk_count()),
                        }
                    })
                } else {
                    self.professional.copied_chunk.as_ref().and_then(|copied| {
                        (copied.anchor_chunk() == placement.source_anchor).then(|| {
                            World3dImportPreview {
                                mesh: mesh.clone(),
                                source_anchor: copied.anchor_chunk(),
                                target_anchor: placement.target_anchor,
                                size: [16, 16],
                                height: 0,
                                mesh_origin_y: 0,
                                origin_y: 0,
                                transform: placement.transform,
                                is_structure: false,
                                is_writing: placement.is_writing(),
                                chunk_count: copied.chunk_count(),
                            }
                        })
                    })
                }
            });
        cx.defer(move |cx| {
            let _ = editor.update(cx, |view, cx| {
                view.set_import_preview(generation, preview, cx);
            });
        });
    }

    pub(super) fn apply_world3d_import_action(
        &mut self,
        action: World3dImportAction,
        cx: &mut Context<Self>,
    ) {
        if let World3dImportAction::StartAt(target) = action {
            if self.map_write_busy() || self.professional.copied_chunk.is_none() {
                self.status = SharedString::from("没有可粘贴的区块副本，或地图正在写入");
                cx.notify();
                return;
            }
            if target.dimension != self.dimension {
                self.set_dimension(target.dimension, cx);
            }
            if self.set_paste_preview(target, PasteTransform::default(), 0.0, None, cx) {
                self.mark_converter_target_selected();
                self.status = SharedString::from("3D 粘贴预览已创建；点击地形定位后确认写入");
                cx.notify();
            }
            return;
        }
        let Some(preview) = self.professional.paste_preview.as_ref() else {
            return;
        };
        if self.professional.copied_chunk.is_none()
            || preview.target_anchor.dimension != self.dimension
            || preview.is_writing()
            || self.map_write_busy()
        {
            return;
        }
        match action {
            World3dImportAction::StartAt(_) => {}
            World3dImportAction::MoveTo(target, y) if target.dimension == self.dimension => {
                let transform = preview.transform;
                let degrees = preview.display_degrees;
                if let Some(import) = self.professional.imported_structure.as_mut() {
                    let (min_y, max_y) = target.y_range(ChunkVersion::New);
                    let max_origin = max_y
                        .saturating_sub(import.structure.size.y)
                        .saturating_add(1);
                    if max_origin < min_y {
                        return;
                    }
                    import.origin_y = y.clamp(min_y, max_origin);
                }
                let next_y = self
                    .professional
                    .imported_structure
                    .as_ref()
                    .map_or(self.y_layer, |import| import.origin_y);
                self.step_y(next_y.saturating_sub(self.y_layer), cx);
                if self.set_paste_preview(target, transform, degrees, None, cx) {
                    self.mark_converter_target_selected();
                    self.status = SharedString::from(format!(
                        "3D 粘贴预览已定位到区块 {}, {}；确认前不会写入世界",
                        target.x, target.z
                    ));
                    cx.notify();
                }
            }
            World3dImportAction::MoveTo(_, _) => {}
            World3dImportAction::Nudge(dx, dz) => {
                let target = ChunkPos {
                    x: preview.target_anchor.x.saturating_add(dx),
                    z: preview.target_anchor.z.saturating_add(dz),
                    dimension: self.dimension,
                };
                if self.set_paste_preview(
                    target,
                    preview.transform,
                    preview.display_degrees,
                    None,
                    cx,
                ) {
                    self.mark_converter_target_selected();
                    cx.notify();
                }
            }
            World3dImportAction::Raise(delta) => {
                let target = preview.target_anchor;
                let transform = preview.transform;
                let degrees = preview.display_degrees;
                let Some(import) = self.professional.imported_structure.as_ref() else {
                    return;
                };
                let (min_y, max_y) = preview.target_anchor.y_range(ChunkVersion::New);
                let max_origin = max_y
                    .saturating_sub(import.structure.size.y)
                    .saturating_add(1);
                if max_origin < min_y {
                    return;
                }
                let next = import
                    .origin_y
                    .saturating_add(delta)
                    .clamp(min_y, max_origin);
                if let Some(import) = self.professional.imported_structure.as_mut() {
                    import.origin_y = next;
                }
                self.step_y(next.saturating_sub(self.y_layer), cx);
                self.set_paste_preview(target, transform, degrees, None, cx);
            }
            World3dImportAction::Rotate(clockwise) => self.rotate_paste_preview(clockwise, cx),
            World3dImportAction::MirrorX => self.toggle_paste_preview_mirror_x(cx),
            World3dImportAction::MirrorZ => self.toggle_paste_preview_mirror_z(cx),
            World3dImportAction::Confirm => self.confirm_paste_preview(cx),
            World3dImportAction::Cancel => {
                self.cancel_paste_preview(cx);
            }
        }
    }

    pub(crate) fn set_2d_paste_height(
        &mut self,
        value: i32,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let preview = self
            .professional
            .paste_preview
            .as_ref()
            .ok_or_else(|| "请先建立粘贴预览".to_string())?;
        if preview.is_writing() || self.map_write_busy() {
            return Err("地图正在写入，暂不能调整粘贴高度".to_string());
        }
        let import = self
            .professional
            .imported_structure
            .as_ref()
            .ok_or_else(|| "当前区块副本没有可调整的结构高度".to_string())?;
        let target = preview.target_anchor;
        let (min_y, max_y) = target.y_range(ChunkVersion::New);
        let max_origin = max_y
            .saturating_sub(import.structure.size.y)
            .saturating_add(1);
        if !(min_y..=max_origin).contains(&value) {
            return Err(format!("粘贴高度须在 {min_y}..{max_origin} 内"));
        }
        self.apply_world3d_import_action(World3dImportAction::MoveTo(target, value), cx);
        Ok(())
    }

    pub(crate) fn adjust_2d_paste_height(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.apply_world3d_import_action(World3dImportAction::Raise(delta), cx);
    }

    pub(crate) fn update_world3d_camera_snapshot(
        &mut self,
        snapshot: Option<World3dCameraSnapshot>,
        cx: &mut Context<Self>,
    ) {
        match snapshot {
            Some(snapshot) => {
                let dimension_matches = snapshot.dimension == self.dimension;
                self.world3d.camera = Some(snapshot);
                if self.world3d.overlay_mode == CameraOverlayMode::Hidden
                    && !self.world3d.overlay_hidden_by_user
                {
                    self.world3d.overlay_mode = CameraOverlayMode::PositionFrustum;
                }
                if self.world3d.world_key.is_none() {
                    self.world3d.world_key = Some(self.current_world3d_key());
                }
                if !dimension_matches && self.world3d.overlay_mode.shows_frustum() {
                    self.status = SharedString::from(format!(
                        "3D 摄像机位于{}，已隐藏 2D 视锥叠加",
                        super::types::dimension_label(snapshot.dimension)
                    ));
                }
                if self.world3d.follow_3d && dimension_matches {
                    let center = (
                        snapshot.position[0].floor() as i32,
                        snapshot.position[2].floor() as i32,
                    );
                    let should_recenter = self.world3d.last_follow_center.is_none_or(|(x, z)| {
                        (x - center.0).abs() >= 8 || (z - center.1).abs() >= 8
                    });
                    if should_recenter {
                        self.world3d.last_follow_center = Some(center);
                        self.center_viewport_on_block(center.0, center.1, cx);
                    }
                }
                self.push_world3d_overlay(cx);
            }
            None => {
                self.world3d.camera = None;
                self.push_world3d_overlay(cx);
            }
        }
        cx.notify();
    }

    fn push_world3d_overlay(&mut self, cx: &mut Context<Self>) {
        let camera = self.world3d.camera;
        let mode = self.world3d.overlay_mode;
        let dimension = self.dimension;
        self.canvas_view.update(cx, |canvas, cx| {
            canvas.set_world3d_camera(camera, mode, dimension, cx);
        });
    }

    pub(crate) fn set_world3d_camera_overlay_mode(
        &mut self,
        mode: CameraOverlayMode,
        cx: &mut Context<Self>,
    ) {
        self.world3d.overlay_mode = mode;
        self.world3d.overlay_hidden_by_user = mode == CameraOverlayMode::Hidden;
        self.push_world3d_overlay(cx);
        self.status = SharedString::from(format!("2D 摄像机叠加：{}", mode.label()));
        cx.notify();
    }

    pub(crate) fn cycle_world3d_camera_overlay_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self.world3d.overlay_mode {
            CameraOverlayMode::Hidden => CameraOverlayMode::Position,
            CameraOverlayMode::Position => CameraOverlayMode::PositionFrustum,
            CameraOverlayMode::PositionFrustum => CameraOverlayMode::PositionFrustumDistance,
            CameraOverlayMode::PositionFrustumDistance => CameraOverlayMode::Hidden,
        };
        self.set_world3d_camera_overlay_mode(next, cx);
    }

    pub(crate) fn handle_world3d_editor_closed(
        &mut self,
        key: &World3dWorldKey,
        cx: &mut Context<Self>,
    ) {
        if self.world3d.world_key.as_ref() == Some(key) {
            self.world3d.clear();
            self.canvas_view.update(cx, |canvas, cx| {
                canvas.set_world3d_camera(None, CameraOverlayMode::Hidden, self.dimension, cx);
            });
            self.status = SharedString::from("3D 世界编辑器已关闭，2D 摄像机叠加已移除");
            cx.notify();
        }
    }

    pub(crate) fn close_world3d_editor_window(&mut self, cx: &mut Context<Self>) {
        let Some(window) = self.world3d.window else {
            return;
        };
        if let Some(key) = self.world3d.world_key.clone() {
            self.handle_world3d_editor_closed(&key, cx);
        }
        let _ = window.update(cx, |_root, window, _cx| {
            window.remove_window();
        });
    }

    /// Opens the linked 3D editor or forwards a navigation command to the live window.
    pub(crate) fn teleport_world3d_editor(
        &mut self,
        block_x: i32,
        block_z: i32,
        height_mode: TeleportHeightMode,
        exact_y: Option<f64>,
        cx: &mut Context<Self>,
    ) {
        let key = self.current_world3d_key();
        let generation = self.world3d.next_navigation_generation();
        let requested_y = match height_mode {
            TeleportHeightMode::CurrentMapLayer => Some(f64::from(self.y_layer) + 1.6),
            TeleportHeightMode::Exact => exact_y,
            TeleportHeightMode::SurfaceSafe => exact_y,
        };
        let target = WorldNavigationTarget {
            world_key: key.clone(),
            world_path: self.world_path.clone(),
            world_name: self.asset.display_name.to_string(),
            dimension: self.dimension,
            block_x,
            block_z,
            height_mode,
            requested_y,
            activate: true,
            request_generation: generation,
        };
        if self.world3d.world_key.as_ref() == Some(&key)
            && let Some(editor) = self.world3d.editor.clone()
            && editor.upgrade().is_some()
        {
            let _ = editor.update(cx, |editor, cx| {
                editor.navigate_for_map(target, cx);
            });
            self.focus_world3d_editor_window(cx);
            self.status = SharedString::from(format!(
                "已传送 3D 编辑器到 X {block_x} · Z {block_z}（{}）",
                height_mode.label()
            ));
            cx.notify();
            return;
        }
        if let Some(entry) = world3d_editor_for(&key)
            && entry.editor.upgrade().is_some()
        {
            let _ = entry.editor.update(cx, |editor, cx| {
                editor.navigate_for_map(target, cx);
            });
            self.attach_world3d_editor_handle(key, entry.editor, entry.window, cx);
            self.focus_world3d_editor_window(cx);
            self.status = SharedString::from(format!(
                "已传送 3D 编辑器到 X {block_x} · Z {block_z}（{}）",
                height_mode.label()
            ));
            cx.notify();
            return;
        }
        self.spawn_open_world3d_editor(target, cx);
    }

    fn spawn_open_world3d_editor(&mut self, target: WorldNavigationTarget, cx: &mut Context<Self>) {
        if self.world3d.opening {
            return;
        }
        self.world3d.opening = true;
        self.status = SharedString::from("正在读取世界出生点并打开 3D 世界编辑器...");
        cx.notify();
        let world_path = self.world_path.clone();
        let package_roots =
            super::super::interactions::preview_3d_resource_package_paths(&world_path, cx);
        let initial_selection = self.world3d_initial_selection();
        let target_for_open = target.clone();
        cx.spawn(async move |handle, cx| {
            let world_path_for_spawn = world_path.clone();
            let (spawn, _spawn_dimension, spawn_source) = cx
                .background_spawn(async move { read_world_spawn(&world_path_for_spawn) })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                this.world3d.opening = false;
                let target = target_for_open;
                let init = World3dEditorInit {
                    world_key: target.world_key.clone(),
                    world_path: target.world_path.clone(),
                    world_name: target.world_name.clone(),
                    dimension: target.dimension,
                    spawn,
                    spawn_source,
                    package_roots,
                    owner: Some(cx.entity().downgrade()),
                    initial_target: Some(target.clone()),
                    initial_selection,
                    view_radius: super::types::WORLD3D_DEFAULT_VIEW_RADIUS,
                };
                open_world3d_editor_window(init, cx);
                if let Some(entry) = world3d_editor_for(&target.world_key) {
                    this.world3d.world_key = Some(target.world_key.clone());
                    this.world3d.editor = Some(entry.editor);
                    this.world3d.window = entry.window;
                    this.sync_world3d_import_preview(cx);
                    this.sync_world3d_chunk_clipboard(cx);
                    this.ensure_world3d_import_preview(cx);
                    this.focus_world3d_editor_window(cx);
                }
                this.status = SharedString::from(format!(
                    "已打开 3D 世界编辑器 · 目标 X {} · Z {}",
                    target.block_x, target.block_z
                ));
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn world3d_initial_selection(&self) -> Option<World3dSelection> {
        let selection = self.professional.selection?;
        if let Some(volume) = self.professional.volume_selection
            && volume.dimension == self.dimension
        {
            return Some(volume);
        }
        let min_chunk_x = selection.start.x.min(selection.end.x);
        let max_chunk_x = selection.start.x.max(selection.end.x);
        let min_chunk_z = selection.start.z.min(selection.end.z);
        let max_chunk_z = selection.start.z.max(selection.end.z);
        let half_height = 16;
        Some(World3dSelection {
            dimension: self.dimension,
            min: [
                min_chunk_x.saturating_mul(16),
                self.y_layer.saturating_sub(half_height),
                min_chunk_z.saturating_mul(16),
            ],
            max: [
                max_chunk_x
                    .saturating_add(1)
                    .saturating_mul(16)
                    .saturating_sub(1),
                self.y_layer.saturating_add(half_height),
                max_chunk_z
                    .saturating_add(1)
                    .saturating_mul(16)
                    .saturating_sub(1),
            ],
        })
    }

    pub(crate) fn sync_2d_volume_selection(&mut self, cx: &mut Context<Self>) {
        let previous_y = self
            .professional
            .volume_selection
            .filter(|volume| volume.dimension == self.dimension)
            .map(|volume| (volume.min[1], volume.max[1]))
            .unwrap_or((self.y_layer, self.y_layer));
        self.professional.volume_selection = self
            .professional
            .selection
            .map(|selection| volume_for_chunk_selection(selection, previous_y));
        self.sync_volume_selection_to_3d(cx);
    }

    fn sync_volume_selection_to_3d(&self, cx: &mut Context<Self>) {
        let Some(editor) = self.world3d.editor.clone() else {
            return;
        };
        let selection = self.professional.volume_selection;
        let dimension = self.dimension;
        cx.defer(move |cx| {
            let _ = editor.update(cx, |view, cx| {
                view.set_selection_from_document(dimension, selection, cx)
            });
        });
    }

    pub(crate) fn adjust_2d_selection_height(
        &mut self,
        upper: bool,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(volume) = self.professional.volume_selection else {
            return;
        };
        let current = if upper { volume.max[1] } else { volume.min[1] };
        self.set_2d_selection_height_bound(upper, current.saturating_add(delta), cx);
    }

    pub(crate) fn set_2d_selection_height_bound(
        &mut self,
        upper: bool,
        value: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(mut volume) = self.professional.volume_selection else {
            return;
        };
        let (min_y, max_y) = ChunkPos {
            x: 0,
            z: 0,
            dimension: volume.dimension,
        }
        .y_range(ChunkVersion::New);
        if min_y > max_y {
            return;
        }
        if upper {
            volume.max[1] = value.clamp(volume.min[1], max_y);
        } else {
            volume.min[1] = value.clamp(min_y, volume.max[1]);
        }
        self.professional.volume_selection = Some(volume);
        self.sync_volume_selection_to_3d(cx);
        cx.notify();
    }

    pub(crate) fn use_current_layer_for_selection_height(
        &mut self,
        upper: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(mut volume) = self.professional.volume_selection else {
            return;
        };
        if upper {
            volume.max[1] = self.y_layer.max(volume.min[1]);
        } else {
            volume.min[1] = self.y_layer.min(volume.max[1]);
        }
        self.professional.volume_selection = Some(volume);
        self.sync_volume_selection_to_3d(cx);
        cx.notify();
    }

    pub(crate) fn show_2d_selection_height(&mut self, upper: bool, cx: &mut Context<Self>) {
        let Some(volume) = self.professional.volume_selection else {
            return;
        };
        let target = if upper { volume.max[1] } else { volume.min[1] };
        self.step_y(target.saturating_sub(self.y_layer), cx);
        self.set_mode(super::super::model::ViewerMode::Layer, cx);
    }

    pub(crate) fn copy_2d_height_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            return;
        };
        let Some(volume) = self.professional.volume_selection else {
            return;
        };
        if super::super::selection::exact_selection_chunks(selection)
            .as_deref()
            .is_some_and(|chunks| {
                !super::super::selection::selection_chunks_are_rectangular(selection, Some(chunks))
            })
        {
            self.status = SharedString::from("高度复制仅支持矩形选区；请重新框选连续区域");
            cx.notify();
            return;
        }
        self.copy_world3d_selection(volume, cx);
    }

    pub(crate) fn clear_2d_height_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            return;
        };
        let Some(volume) = self.professional.volume_selection else {
            return;
        };
        if super::super::selection::exact_selection_chunks(selection)
            .as_deref()
            .is_some_and(|chunks| {
                !super::super::selection::selection_chunks_are_rectangular(selection, Some(chunks))
            })
        {
            self.status = SharedString::from("高度清除仅支持矩形选区；请重新框选连续区域");
            cx.notify();
            return;
        }
        if volume.volume() > super::types::WORLD3D_MAX_EDIT_BLOCKS as u64 {
            self.status = SharedString::from(format!(
                "高度清除超过 {} 个方块上限",
                super::types::WORLD3D_MAX_EDIT_BLOCKS
            ));
            cx.notify();
            return;
        }
        self.apply_world3d_edit_request(
            World3dBlockEditRequest {
                request_id: 0,
                dimension: volume.dimension,
                min: volume.min,
                max: volume.max,
                kind: World3dBlockEditKind::Clear,
            },
            cx,
        );
    }

    pub(crate) fn fill_2d_height_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            return;
        };
        let Some(volume) = self.professional.volume_selection else {
            return;
        };
        if super::super::selection::exact_selection_chunks(selection)
            .as_deref()
            .is_some_and(|chunks| {
                !super::super::selection::selection_chunks_are_rectangular(selection, Some(chunks))
            })
        {
            self.status = SharedString::from("高度填充仅支持矩形选区；请重新框选连续区域");
            cx.notify();
            return;
        }
        if volume.volume() > super::types::WORLD3D_MAX_EDIT_BLOCKS as u64 {
            self.status = SharedString::from(format!(
                "高度填充超过 {} 个方块上限",
                super::types::WORLD3D_MAX_EDIT_BLOCKS
            ));
            cx.notify();
            return;
        }
        let block_name = self.input_fields.height_fill_block.read(cx).value();
        let block_name = block_name.as_ref().trim();
        if !block_name
            .split_once(':')
            .is_some_and(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
            || block_name.chars().any(|ch| {
                !(ch.is_ascii_lowercase()
                    || ch.is_ascii_digit()
                    || matches!(ch, ':' | '_' | '/' | '.' | '-'))
            })
        {
            self.status = SharedString::from("请输入有效方块 ID，例如 minecraft:stone");
            cx.notify();
            return;
        }
        self.apply_world3d_edit_request(
            World3dBlockEditRequest {
                request_id: 0,
                dimension: volume.dimension,
                min: volume.min,
                max: volume.max,
                kind: World3dBlockEditKind::Fill {
                    block_name: block_name.to_string(),
                },
            },
            cx,
        );
    }
}

fn volume_for_chunk_selection(selection: ChunkSelection, y: (i32, i32)) -> World3dSelection {
    let bounds = selection.bounds();
    World3dSelection {
        dimension: bounds.dimension,
        min: [
            bounds.min_chunk_x.saturating_mul(16),
            y.0,
            bounds.min_chunk_z.saturating_mul(16),
        ],
        max: [
            bounds
                .max_chunk_x
                .saturating_add(1)
                .saturating_mul(16)
                .saturating_sub(1),
            y.1,
            bounds
                .max_chunk_z
                .saturating_add(1)
                .saturating_mul(16)
                .saturating_sub(1),
        ],
    }
}

#[cfg(test)]
mod height_selection_tests {
    use super::ChunkSelection;
    use super::volume_for_chunk_selection;
    use super::{BwChunkPos, BwDimension};
    // Disambiguate the built-in test attribute from the glob-imported macro of the same name.
    use core::prelude::v1::test;

    #[test]
    fn chunk_selection_preserves_height_and_negative_chunk_bounds() {
        let selection = ChunkSelection {
            start: BwChunkPos {
                x: -2,
                z: 3,
                dimension: BwDimension::Overworld,
            },
            end: BwChunkPos {
                x: -1,
                z: 4,
                dimension: BwDimension::Overworld,
            },
        };
        let volume = volume_for_chunk_selection(selection, (-20, 12));
        assert_eq!(volume.min, [-32, -20, 48]);
        assert_eq!(volume.max, [-1, 12, 79]);
    }
}

impl MapViewerWindowView {
    /// Invalidates 3D chunk meshes after any 2D document write.
    pub(crate) fn notify_world3d_mutation(
        &mut self,
        chunks: &BTreeSet<ChunkPos>,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.world3d.editor.clone() else {
            return;
        };
        let dimension = self.dimension;
        let chunks = chunks
            .iter()
            .copied()
            .filter(|chunk| chunk.dimension == dimension)
            .collect::<Vec<_>>();
        if chunks.is_empty() {
            return;
        }
        let _ = editor.update(cx, |editor, cx| {
            editor.invalidate_chunks_from_document(chunks, cx);
        });
    }

    /// Opens the existing professional chunk data editor for a 3D target chunk.
    pub(crate) fn open_world3d_chunk_detail(&mut self, chunk: ChunkPos, cx: &mut Context<Self>) {
        self.ui_state.right_panel_open = true;
        self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
        self.load_edit_detail(super::super::model::EditTarget::BlockEntities(chunk), cx);
        cx.notify();
    }

    pub(crate) fn toggle_world3d_follow(&mut self, cx: &mut Context<Self>) {
        self.world3d.follow_3d = !self.world3d.follow_3d;
        self.world3d.last_follow_center = None;
        self.status = SharedString::from(if self.world3d.follow_3d {
            "2D 跟随 3D 摄像机已开启"
        } else {
            "2D 跟随 3D 摄像机已关闭"
        });
        if self.world3d.follow_3d {
            if let Some(camera) = self
                .world3d
                .camera
                .filter(|camera| camera.dimension == self.dimension)
            {
                let x = camera.position[0].floor() as i32;
                let z = camera.position[2].floor() as i32;
                self.world3d.last_follow_center = Some((x, z));
                self.center_viewport_on_block(x, z, cx);
            }
        }
        cx.notify();
    }
}

fn read_world_spawn(world_path: &Path) -> (Option<[f64; 3]>, Option<BwDimension>, String) {
    let level_dat = world_path.join("level.dat");
    let document = match read_level_dat_document(&level_dat) {
        Ok(document) => document,
        Err(error) => {
            return (None, None, format!("level.dat 读取失败：{error}"));
        }
    };
    let NbtTag::Compound(root) = &document.root else {
        return (None, None, "level.dat 根节点不是 compound".to_string());
    };
    let spawn_x = nbt_i32(root.get("SpawnX"));
    let spawn_y = nbt_i32(root.get("SpawnY"));
    let spawn_z = nbt_i32(root.get("SpawnZ"));
    let dimension = nbt_i32(root.get("SpawnDimension")).map(BwDimension::from_id);
    let spawn_dimension = dimension.unwrap_or(BwDimension::Overworld);
    let (min_y, max_y) = match spawn_dimension {
        BwDimension::Overworld => (-64, 319),
        BwDimension::Nether => (0, 127),
        BwDimension::End => (0, 255),
        BwDimension::Unknown(_) => (0, 255),
    };
    let in_build_height = spawn_y.is_some_and(|y| (min_y..=max_y).contains(&y));
    let in_world_bounds = spawn_x
        .zip(spawn_z)
        .is_some_and(|(x, z)| x.abs() < 30_000_000 && z.abs() < 30_000_000);
    let spawn = match (spawn_x, spawn_y, spawn_z) {
        (Some(x), Some(y), Some(z)) if in_build_height && in_world_bounds => {
            Some([f64::from(x) + 0.5, f64::from(y) + 1.6, f64::from(z) + 0.5])
        }
        _ => None,
    };
    (
        spawn,
        dimension,
        if spawn.is_some() {
            "level.dat SpawnX/Y/Z".to_string()
        } else if spawn_y.is_some() {
            format!(
                "level.dat SpawnY={} 超出 {} 建造高度（使用回退坐标）",
                spawn_y.unwrap_or_default(),
                match spawn_dimension {
                    BwDimension::Overworld => "主世界",
                    BwDimension::Nether => "下界",
                    BwDimension::End => "末地",
                    BwDimension::Unknown(_) => "未知维度",
                }
            )
        } else {
            "level.dat 缺少有效 SpawnX/Y/Z（使用回退坐标）".to_string()
        },
    )
}

fn nbt_i32(tag: Option<&NbtTag>) -> Option<i32> {
    match tag {
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

impl MapViewerWindowView {
    /// Closes the linked editor window and removes its 2D overlay state.
    pub(crate) fn release_world3d_link(&mut self, cx: &mut Context<Self>) {
        self.close_world3d_editor_window(cx);
        self.world3d.clear();
        let dimension = self.dimension;
        self.canvas_view.update(cx, |canvas, cx| {
            canvas.set_world3d_camera(None, CameraOverlayMode::Hidden, dimension, cx);
        });
    }

    pub(crate) fn world3d_overlay_mode_label(&self) -> String {
        self.world3d.overlay_mode.label().to_string()
    }

    /// Opens the editor centered on the current 2D viewport center.
    pub(crate) fn open_world3d_editor_at_map_center(&mut self, cx: &mut Context<Self>) {
        let (block_x, block_z) = self.viewport.center_block(self.active_layout);
        self.teleport_world3d_editor(
            block_x,
            block_z,
            TeleportHeightMode::CurrentMapLayer,
            Some(f64::from(self.y_layer) + 1.6),
            cx,
        );
    }

    /// Forwards a camera-targeted teleport request when the editor is already open.
    pub(crate) fn show_world3d_editor_window(&mut self, cx: &mut Context<Self>) {
        if self.world3d.is_open() {
            self.focus_world3d_editor_window(cx);
            return;
        }
        self.open_world3d_editor_at_map_center(cx);
    }
}

fn apply_world3d_edit_blocking(
    world_path: &Path,
    request: &World3dBlockEditRequest,
) -> Result<(MapEditInvalidation, String, Vec<BwChunkPos>), String> {
    let mut options = OpenOptions::default();
    options.read_only = false;
    let world = BwWorld::open(world_path, options)
        .map_err(|error| format!("打开世界写入通道失败：{error}"))?;
    let operation = format!(
        "3D block edit dim={} min={:?} max={:?}",
        request.dimension.id(),
        request.min,
        request.max
    );
    let guard = WriteGuard::confirmed(world_path.to_path_buf(), operation);
    let mut version_cache = std::collections::HashMap::<BwChunkPos, i32>::new();
    let mut edits = Vec::<BlockEdit>::new();
    let mut affected = std::collections::BTreeSet::<BwChunkPos>::new();
    match &request.kind {
        World3dBlockEditKind::Fill { block_name } => {
            push_volume_edits(
                &world,
                request,
                block_name,
                &mut version_cache,
                &mut edits,
                &mut affected,
            )?;
        }
        World3dBlockEditKind::Clear => {
            push_volume_edits(
                &world,
                request,
                "minecraft:air",
                &mut version_cache,
                &mut edits,
                &mut affected,
            )?;
        }
    }
    if edits.is_empty() {
        return Err("3D 编辑没有任何目标方块".to_string());
    }
    let history_label = match &request.kind {
        World3dBlockEditKind::Fill { block_name } => {
            format!("3D 填充 {} · {} 个方块", block_name, edits.len())
        }
        World3dBlockEditKind::Clear => format!("3D 清除选区 · {} 个方块", edits.len()),
    };
    let history_capture = capture_before_with_world_and_progress(
        MapHistoryCaptureSpec {
            kind: MapHistoryEntryKind::BlockEdit,
            label: history_label.clone(),
            world_path: world_path.to_path_buf(),
            chunks: affected.clone(),
            raw_keys: std::collections::BTreeSet::new(),
            include_level_dat: false,
        },
        &world,
        |_| {},
    );
    let result = apply_block_edits(
        &world,
        &edits,
        &guard,
        BlockEditOptions {
            commit_batch_chunks: 16,
            compact_empty_subchunks: true,
        },
    );
    let message = format!("{history_label} 已写入并记录到 2D 文档历史");
    match (history_capture, result) {
        (Ok(capture), Ok(result)) => {
            complete_after_with_world_and_progress(capture, &world, message.clone(), |_| {})
                .map_err(|error| format!("历史写入失败：{error}"))?;
            let chunks = result.affected_chunks.iter().copied().collect::<Vec<_>>();
            Ok((
                MapEditInvalidation::chunks(chunks.iter().copied()).with_metadata(),
                message,
                chunks,
            ))
        }
        (Ok(capture), Err(error)) => {
            let _ = complete_failed(capture, error.to_string());
            Err(format!("3D 写入失败：{error}"))
        }
        (Err(history_error), Ok(result)) => {
            tracing::warn!(%history_error, "3D edit history capture failed");
            let chunks = result.affected_chunks.iter().copied().collect::<Vec<_>>();
            Ok((
                MapEditInvalidation::chunks(chunks.iter().copied()).with_metadata(),
                format!("{message}（历史捕获失败：{history_error}）"),
                chunks,
            ))
        }
        (Err(history_error), Err(error)) => Err(format!(
            "3D 写入失败：{error}；历史捕获失败：{history_error}"
        )),
    }
}

fn push_volume_edits(
    world: &BwWorld,
    request: &World3dBlockEditRequest,
    block_name: &str,
    version_cache: &mut std::collections::HashMap<BwChunkPos, i32>,
    edits: &mut Vec<BlockEdit>,
    affected: &mut std::collections::BTreeSet<BwChunkPos>,
) -> Result<(), String> {
    let min_chunk_x = request.min[0].div_euclid(16);
    let max_chunk_x = request.max[0].div_euclid(16);
    let min_chunk_z = request.min[2].div_euclid(16);
    let max_chunk_z = request.max[2].div_euclid(16);
    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            let chunk = BwChunkPos {
                x: chunk_x,
                z: chunk_z,
                dimension: request.dimension,
            };
            let version = *version_cache
                .entry(chunk)
                .or_insert_with(|| chunk_block_state_version(world, chunk).unwrap_or(1));
            let x_start = request.min[0].max(chunk_x * 16);
            let x_end = request.max[0].min(chunk_x * 16 + 15);
            let z_start = request.min[2].max(chunk_z * 16);
            let z_end = request.max[2].min(chunk_z * 16 + 15);
            let mut chunk_has_edits = false;
            for y in request.min[1]..=request.max[1] {
                for z in z_start..=z_end {
                    for x in x_start..=x_end {
                        edits.push(BlockEdit::new(
                            request.dimension,
                            BlockPos { x, y, z },
                            BlockState {
                                name: block_name.to_string(),
                                states: std::collections::BTreeMap::new(),
                                version: Some(version),
                            },
                        ));
                        chunk_has_edits = true;
                    }
                }
            }
            if chunk_has_edits {
                affected.insert(chunk);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod copy_request_tests {
    use super::World3dMapLink;

    #[test]
    fn repeated_copy_requests_wait_for_the_previous_completion() {
        let mut link = World3dMapLink::default();
        assert!(link.try_start_copy());
        assert!(!link.try_start_copy());
        link.clear();
        assert!(!link.try_start_copy());
        link.finish_copy();
        assert!(link.try_start_copy());
    }
}

fn chunk_block_state_version(world: &BwWorld, chunk: BwChunkPos) -> Result<i32, String> {
    let version = if chunk.dimension == BwDimension::Overworld {
        ::bedrock_world::ChunkVersion::New
    } else {
        ::bedrock_world::ChunkVersion::Old
    };
    let (min_subchunk, max_subchunk) = chunk.subchunk_index_range(version);
    for subchunk_y in min_subchunk..=max_subchunk {
        let Ok(Some(subchunk)) = world.subchunk(chunk, subchunk_y, SubChunkDecodeMode::FullIndices)
        else {
            continue;
        };
        if let ::bedrock_world::SubChunkFormat::Paletted { storages, .. } = subchunk.format {
            for storage in storages {
                for state in storage.states {
                    if let Some(version) = state.version {
                        return Ok(version);
                    }
                }
            }
        }
    }
    Ok(1)
}

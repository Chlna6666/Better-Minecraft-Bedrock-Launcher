use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use crate::core::minecraft::map::image::MapResample;

use super::generator_panel::{generator_choice, generator_section_title};
use super::model::MapViewerWindowView;
use super::players::PlayerRecordHealth;
use super::prelude::*;
use super::tile_state::TilePriority;
use crate::core::minecraft::map::generator::{
    MapPreviewOptions, get_map_preview, get_map_preview_frame, release_map_preview,
    start_map_image_preview,
};
use crate::ui::components::slider::Slider;

mod actions;
mod bundle;
mod comparison;
mod controls;
mod players;
mod render;
mod target;

pub(super) struct MapImagePanelState {
    source: Option<PathBuf>,
    options: MapPreviewOptions,
    task_id: Option<String>,
    preview_options: Option<MapPreviewOptions>,
    export_task_id: Option<String>,
    install_task_id: Option<String>,
    install_refresh: Option<MapImageInstallRefresh>,
    install_destination: MapImageInstallDestination,
    selected_player: PlayerId,
    selected_block: Option<(ChunkPos, BlockPos)>,
    pub(super) player_search: Entity<InputState>,
    player_scroll: UniformListScrollHandle,
    frame_support: &'static str,
    preview: Option<Arc<RenderImage>>,
    preview_done: u32,
    grid_estimated: bool,
    preview_note: Option<String>,
}

impl MapImagePanelState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<MapViewerWindowView>) -> Self {
        let player_search = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("搜索玩家名称 / UUID / 存储 ID", window, cx);
            input
        });
        Self {
            source: None,
            options: MapPreviewOptions {
                auto_fit: true,
                ..MapPreviewOptions::default()
            },
            task_id: None,
            preview_options: None,
            export_task_id: None,
            install_task_id: None,
            install_refresh: None,
            install_destination: MapImageInstallDestination::Player,
            selected_player: PlayerId::Local,
            selected_block: None,
            player_search,
            player_scroll: UniformListScrollHandle::new(),
            frame_support: "minecraft:stone",
            preview: None,
            preview_done: 0,
            grid_estimated: false,
            preview_note: None,
        }
    }
}

pub(super) enum MapImageInstallRefresh {
    Player(PlayerId),
    Chunks(Arc<std::sync::Mutex<BTreeSet<ChunkPos>>>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MapImageInstallDestination {
    Player,
    SelectedBlock,
    NewUpFrame,
}

#[derive(Clone, Copy)]
enum GridAxis {
    Columns,
    Rows,
}

impl MapViewerWindowView {
    fn set_map_install_destination(
        &mut self,
        destination: MapImageInstallDestination,
        cx: &mut Context<Self>,
    ) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.install_destination = destination;
        if destination != MapImageInstallDestination::Player {
            self.set_import_workspace_mode(super::state::ImportWorkspaceMode::Placement, cx);
        }
        cx.notify();
    }

    fn set_map_frame_support(&mut self, name: &'static str, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.frame_support = name;
        cx.notify();
    }

    fn set_map_image_player(&mut self, player: PlayerId, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        self.map_image.selected_player = player;
        cx.notify();
    }

    pub(super) fn map_frame_grid_preview(&self) -> Option<super::model::MapFrameGridPreview> {
        if !self.ui_state.right_panel_open
            || self.ui_state.active_right_panel != MapViewerRightPanel::MapImage
            || self.map_image.install_destination != MapImageInstallDestination::NewUpFrame
            || self.map_image.source.is_none()
        {
            return None;
        }
        let (center_x, center_z) = self.viewport.center_block(self.active_layout);
        Some(super::model::MapFrameGridPreview {
            min_block_x: center_x.saturating_sub((self.map_image.options.columns / 2) as i32),
            min_block_z: center_z.saturating_sub((self.map_image.options.rows / 2) as i32),
            columns: self.map_image.options.columns,
            rows: self.map_image.options.rows,
        })
    }

    pub(super) fn accept_map_install_task_snapshot(
        &mut self,
        snapshot: &TaskSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self.map_image.install_task_id.as_deref() != Some(snapshot.id.as_ref())
            || !snapshot.is_terminal()
        {
            return;
        }
        if let Some(target) = self.map_image.install_refresh.take() {
            match target {
                MapImageInstallRefresh::Player(player) => self.load_player_detail(player, cx),
                MapImageInstallRefresh::Chunks(chunks) => match chunks.lock() {
                    Ok(chunks) if !chunks.is_empty() => {
                        let invalidation =
                            MapEditInvalidation::chunks(chunks.clone()).with_metadata();
                        self.apply_map_edit_invalidation_with_tile_priority(
                            &invalidation,
                            TilePriority::EditRefresh,
                            cx,
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(%error, "map import refresh scope is unavailable");
                    }
                },
            }
            self.refresh_history(cx);
        }
    }

    pub(super) fn release_map_image_preview(&mut self) {
        if let Some(task_id) = self.map_image.task_id.take() {
            task_manager::cancel_task(&task_id);
            release_map_preview(&task_id);
        }
        self.map_image.preview = None;
        self.map_image.preview_options = None;
        self.map_image.preview_done = 0;
        self.map_image.grid_estimated = false;
        self.map_image.preview_note = None;
    }

    pub(super) fn accept_map_image_task_snapshot(&mut self, snapshot: &TaskSnapshot) {
        if self.map_image.task_id.as_deref() != Some(snapshot.id.as_ref()) {
            return;
        }
        if snapshot.status.as_ref() != "completed" {
            if let Some(frame) = get_map_preview_frame(snapshot.id.as_ref()) {
                self.map_image.options.columns = frame.columns;
                self.map_image.options.rows = frame.rows;
                if frame.done == 0 {
                    self.map_image.grid_estimated = true;
                    self.map_image.preview_note = Some(format!(
                        "已读取图片 {}×{}；预计分片 {}×{}（{} 张），裁切 x={} y={} {}×{}。正在生成地图像素。",
                        frame.source_width,
                        frame.source_height,
                        frame.columns,
                        frame.rows,
                        frame.columns * frame.rows,
                        frame.crop.x,
                        frame.crop.y,
                        frame.crop.width,
                        frame.crop.height,
                    ));
                } else if frame.done > self.map_image.preview_done {
                    if let Ok(image) = RenderImage::from_raw_pixels(
                        frame.width,
                        frame.height,
                        ImagePixelFormat::Rgba8,
                        frame.rgba.clone(),
                    ) {
                        self.map_image.preview = Some(Arc::new(image));
                        self.map_image.preview_done = frame.done;
                        self.map_image.grid_estimated = false;
                        let total = snapshot.total.unwrap_or_else(|| {
                            u64::from(self.map_image.options.columns * self.map_image.options.rows)
                        });
                        self.map_image.preview_note = Some(format!(
                            "正在合并第 {} / {} 张地图；深色线只表示分片边界。",
                            frame.done, total,
                        ));
                    }
                }
            }
            return;
        }
        if self.map_image.preview_done
            == self.map_image.options.columns * self.map_image.options.rows
            && self
                .map_image
                .preview_note
                .as_ref()
                .is_some_and(|note| note.starts_with("源图"))
        {
            return;
        }
        let Some(result) = get_map_preview(snapshot.id.as_ref()) else {
            self.map_image.preview_note = Some("预览结果已释放或过期，请重新转换".to_owned());
            return;
        };
        match RenderImage::from_raw_pixels(
            result.preview_width,
            result.preview_height,
            ImagePixelFormat::Rgba8,
            result.preview_rgba.clone(),
        ) {
            Ok(image) => {
                self.map_image.options.columns = result.columns;
                self.map_image.options.rows = result.rows;
                self.map_image.preview = Some(Arc::new(image));
                self.map_image.preview_done = result.tiles.len() as u32;
                self.map_image.grid_estimated = false;
                self.map_image.preview_note = Some(format!(
                    "源图 {}×{}；裁剪 x={} y={} {}×{}；输出 {}×{} 张地图，实际像素 {}×{}。深色线仅表示分片边界。",
                    result.source_width,
                    result.source_height,
                    result.crop.x,
                    result.crop.y,
                    result.crop.width,
                    result.crop.height,
                    result.columns,
                    result.rows,
                    result.columns * 128,
                    result.rows * 128,
                ));
            }
            Err(error) => {
                self.map_image.preview_note = Some(format!("预览图像创建失败：{error}"));
            }
        }
    }

    fn map_image_busy(&self) -> bool {
        self.map_image.task_id.as_ref().is_some_and(|id| {
            self.task_snapshots
                .get(id.as_str())
                .is_some_and(|snapshot| !snapshot.is_terminal())
        })
    }

    fn map_image_operation_busy(&self) -> bool {
        self.map_write_busy()
            || [
                &self.map_image.export_task_id,
                &self.map_image.install_task_id,
            ]
            .into_iter()
            .flatten()
            .any(|id| {
                self.task_snapshots
                    .get(id.as_str())
                    .is_some_and(|snapshot| !snapshot.is_terminal())
            })
    }

    pub(super) fn map_bundle_write_busy(&self) -> bool {
        self.map_image.install_task_id.as_deref().is_some_and(|id| {
            self.task_snapshots
                .get(id)
                .is_none_or(|snapshot| !snapshot.is_terminal())
        })
    }

    pub(super) fn map_image_task_snapshot(&self) -> Option<&Arc<TaskSnapshot>> {
        [
            self.map_image.task_id.as_deref(),
            self.map_image.export_task_id.as_deref(),
            self.map_image.install_task_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|id| self.task_snapshots.get(id))
        .max_by_key(|task| {
            (
                !task.is_terminal(),
                task.started_at_unix,
                task.last_update_unix,
            )
        })
    }

    fn choose_map_source(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        if let Some(path) =
            pick_file_path_with_filter("图片", &["png", "jpg", "jpeg", "tga", "webp"])
        {
            self.map_image.source = Some(PathBuf::from(path));
            self.map_image.preview_options = None;
            self.map_image.options.auto_fit = true;
            self.start_map_preview(cx);
        }
    }

    fn set_grid_size(&mut self, axis: GridAxis, value: u32, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        let other = match axis {
            GridAxis::Columns => self.map_image.options.rows,
            GridAxis::Rows => self.map_image.options.columns,
        };
        let max = (1024 / other.max(1)).min(64);
        let target = match axis {
            GridAxis::Columns => &mut self.map_image.options.columns,
            GridAxis::Rows => &mut self.map_image.options.rows,
        };
        *target = value.clamp(1, max);
        self.map_image.options.auto_fit = false;
        self.start_map_preview(cx);
    }

    fn set_grid_size_from_slider(&mut self, axis: GridAxis, value: u32, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        let other = match axis {
            GridAxis::Columns => self.map_image.options.rows,
            GridAxis::Rows => self.map_image.options.columns,
        };
        let max = (1024 / other.max(1)).min(64);
        let target = match axis {
            GridAxis::Columns => &mut self.map_image.options.columns,
            GridAxis::Rows => &mut self.map_image.options.rows,
        };
        let value = value.clamp(1, max);
        if *target != value || self.map_image.options.auto_fit {
            *target = value;
            self.map_image.options.auto_fit = false;
            self.release_map_image_preview();
            cx.notify();
        }
    }

    fn enable_grid_auto_fit(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        if !self.map_image.options.auto_fit {
            self.map_image.options.auto_fit = true;
            self.start_map_preview(cx);
        }
    }

    pub(super) fn ensure_map_image_preview(&mut self, cx: &mut Context<Self>) {
        if self.map_image.source.is_some() && self.map_image.task_id.is_none() {
            self.start_map_preview(cx);
        }
    }

    fn start_map_preview(&mut self, cx: &mut Context<Self>) {
        if self.map_image_operation_busy() {
            return;
        }
        let Some(source) = self.map_image.source.clone() else {
            self.status = SharedString::from("请先选择图片");
            cx.notify();
            return;
        };
        let mut options = self.map_image.options;
        if options.auto_fit {
            options.columns = 1;
            options.rows = 1;
        }
        if self.map_image.preview_options == Some(options)
            && self
                .map_image
                .task_id
                .as_deref()
                .and_then(|id| self.task_snapshots.get(id))
                .is_some_and(|task| !matches!(task.status.as_ref(), "error" | "cancelled"))
        {
            return;
        }
        self.release_map_image_preview();
        self.map_image.export_task_id = None;
        match start_map_image_preview(source, options) {
            Ok(task_id) => {
                self.map_image.preview_options = Some(options);
                self.map_image.task_id = Some(task_id.clone());
                if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
                    self.accept_map_image_task_snapshot(&snapshot);
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.status = SharedString::from("正在生成 Bedrock 地图像素预览");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}

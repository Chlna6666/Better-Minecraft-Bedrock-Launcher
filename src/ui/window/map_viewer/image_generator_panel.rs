use std::path::PathBuf;

use crate::core::minecraft::map::image::MapResample;

use super::generator_panel::{generator_choice, generator_section_title};
use super::model::MapViewerWindowView;
use super::prelude::*;
use crate::core::minecraft::map::generator::{
    Dithering, ImageBlockExportRequest, ImageBlockHeight, ImageBlockOptions, ImageFit,
    ReliefImageFill, StructurePreviewKind, StructurePreviewRequest, approved_block_names,
    release_structure_preview, start_image_block_export, start_structure_preview,
};
use crate::ui::components::slider::Slider;

mod parameters;
mod render;

mod comparison;
mod controls;

const IMAGE_PREVIEW_REFRESH_DELAY: Duration = Duration::from_millis(180);

pub(super) struct ImageGeneratorPanelState {
    source: Option<PathBuf>,
    output: Option<PathBuf>,
    options: ImageBlockOptions,
    preview_options: Option<ImageBlockOptions>,
    task_id: Option<String>,
    pub(super) preview_task_id: Option<String>,
    pub(super) preview_processed: bool,
    pub(super) preview_size: Option<[i32; 3]>,
    pub(super) preview_result_active: bool,
    pub(super) preview_anchor: Option<ChunkPos>,
    fill_picker_open: bool,
    fill_candidates: Vec<String>,
    preview_refresh_task: Option<Task<anyhow::Result<()>>>,
    preview_generation: u64,
    preview_refresh_pending: bool,
}

impl Default for ImageGeneratorPanelState {
    fn default() -> Self {
        Self {
            source: None,
            output: None,
            options: ImageBlockOptions::default(),
            preview_options: None,
            task_id: None,
            preview_task_id: None,
            preview_processed: false,
            preview_size: None,
            preview_result_active: false,
            preview_anchor: None,
            fill_picker_open: false,
            fill_candidates: approved_block_names(),
            preview_refresh_task: None,
            preview_generation: 0,
            preview_refresh_pending: false,
        }
    }
}

#[derive(Clone, Copy)]
enum ImageAxis {
    Width,
    Height,
}

impl MapViewerWindowView {
    pub(super) fn image_export_task_id(&self) -> Option<&str> {
        self.image_generator.task_id.as_deref()
    }

    fn image_generator_busy(&self) -> bool {
        self.map_write_busy()
            || self.image_generator.task_id.as_ref().is_some_and(|id| {
                self.task_snapshots
                    .get(id.as_str())
                    .is_some_and(|snapshot| !snapshot.is_terminal())
            })
    }

    fn choose_generator_image(&mut self, cx: &mut Context<Self>) {
        if self.image_generator_busy() {
            return;
        }
        if let Some(path) =
            pick_file_path_with_filter("图片", &["png", "jpg", "jpeg", "tga", "webp"])
        {
            self.image_generator.source = Some(PathBuf::from(path));
            self.image_generator.preview_options = None;
            self.image_generator.output = None;
            self.image_generator.task_id = None;
            self.start_image_live_preview(cx);
            cx.notify();
        }
    }

    fn set_image_dimension(&mut self, axis: ImageAxis, size: u32, cx: &mut Context<Self>) {
        if self.image_generator_busy() {
            return;
        }
        let target = match axis {
            ImageAxis::Width => &mut self.image_generator.options.width,
            ImageAxis::Height => &mut self.image_generator.options.height,
        };
        *target = size.clamp(1, 512);
        self.clamp_image_relief_height();
        self.start_image_live_preview(cx);
        cx.notify();
    }

    fn update_image_dimension_from_slider(
        &mut self,
        axis: ImageAxis,
        size: u32,
        cx: &mut Context<Self>,
    ) {
        if self.image_generator_busy() {
            return;
        }
        let target = match axis {
            ImageAxis::Width => &mut self.image_generator.options.width,
            ImageAxis::Height => &mut self.image_generator.options.height,
        };
        *target = size.clamp(1, 512);
        self.clamp_image_relief_height();
        self.schedule_image_live_preview(cx);
    }

    fn clamp_image_relief_height(&mut self) {
        let max_allowed = self.image_generator.options.max_relief_height();
        if let ImageBlockHeight::LuminanceRelief { max_height, fill } =
            self.image_generator.options.depth
            && max_height > max_allowed
        {
            self.image_generator.options.depth = ImageBlockHeight::LuminanceRelief {
                max_height: max_allowed,
                fill,
            };
        }
    }

    pub(super) fn cancel_image_preview_refresh(&mut self) {
        self.image_generator.preview_generation =
            self.image_generator.preview_generation.saturating_add(1);
        drop(self.image_generator.preview_refresh_task.take());
        self.image_generator.preview_refresh_pending = false;
    }

    fn schedule_image_live_preview(&mut self, cx: &mut Context<Self>) {
        if self.image_generator.preview_refresh_pending {
            cx.notify();
            return;
        }
        self.cancel_image_preview_refresh();
        self.clear_image_live_preview(cx);
        self.image_generator.preview_refresh_pending = true;
        let generation = self.image_generator.preview_generation;
        self.image_generator.preview_refresh_task = Some(cx.spawn(async move |handle, cx| {
            Timer::after(IMAGE_PREVIEW_REFRESH_DELAY).await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.image_generator.preview_generation != generation {
                    return;
                }
                this.image_generator.preview_refresh_pending = false;
                this.start_image_live_preview_now(cx);
            })?;
            Ok::<(), anyhow::Error>(())
        }));
        cx.notify();
    }

    pub(super) fn start_image_live_preview(&mut self, cx: &mut Context<Self>) {
        self.cancel_image_preview_refresh();
        if self.image_generator.preview_options.as_ref() == Some(&self.image_generator.options)
            && self
                .image_generator
                .preview_task_id
                .as_deref()
                .is_some_and(|id| {
                    self.task_snapshots
                        .get(id)
                        .is_some_and(|task| !matches!(task.status.as_ref(), "error" | "cancelled"))
                })
        {
            return;
        }
        self.clear_image_live_preview(cx);
        self.start_image_live_preview_now(cx);
    }

    fn clear_image_live_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.image_generator.preview_task_id.take() {
            task_manager::cancel_task(&id);
            release_structure_preview(&id);
        }
        if self.image_generator.source.is_some() {
            self.clear_live_structure_preview(cx);
        }
        self.image_generator.preview_processed = false;
        self.image_generator.preview_size = None;
        self.image_generator.preview_result_active = false;
        self.image_generator.preview_options = None;
    }

    fn start_image_live_preview_now(&mut self, cx: &mut Context<Self>) {
        let source = self.image_generator.source.clone();
        let Some(source) = source else {
            return;
        };
        self.image_generator.preview_processed = false;
        self.image_generator.preview_size = None;
        let options = self.image_generator.options.clone();
        match start_structure_preview(StructurePreviewRequest {
            source,
            kind: StructurePreviewKind::Image(options),
        }) {
            Ok(id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.image_generator.preview_task_id = Some(id);
                self.image_generator.preview_options = Some(self.image_generator.options.clone());
            }
            Err(error) => self.status = SharedString::from(error),
        }
        cx.notify();
    }

    fn set_image_depth(&mut self, depth: ImageBlockHeight, cx: &mut Context<Self>) {
        self.image_generator.options.depth = depth;
        self.clamp_image_relief_height();
        self.start_image_live_preview(cx);
        cx.notify();
    }

    fn set_image_dithering(&mut self, dithering: Dithering, cx: &mut Context<Self>) {
        self.image_generator.options.dithering = dithering;
        self.start_image_live_preview(cx);
        cx.notify();
    }

    fn update_image_relief_height(
        &mut self,
        max_height: u16,
        fill: ReliefImageFill,
        cx: &mut Context<Self>,
    ) {
        self.image_generator.options.depth = ImageBlockHeight::LuminanceRelief { max_height, fill };
        self.schedule_image_live_preview(cx);
    }

    fn start_image_export(&mut self, cx: &mut Context<Self>) {
        if self.image_generator_busy() {
            return;
        }
        let Some(source) = self.image_generator.source.clone() else {
            self.status = SharedString::from("请先选择图片");
            cx.notify();
            return;
        };
        let suffix = match self.image_generator.options.depth {
            ImageBlockHeight::Flat => "flat",
            ImageBlockHeight::LuminanceRelief { .. } => "relief",
        };
        let default_name = format!(
            "{}-{suffix}.mcstructure",
            source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("image")
        );
        let Some(path) =
            pick_save_path_with_filter("Bedrock Structure", &["mcstructure"], &default_name)
        else {
            return;
        };
        let output = PathBuf::from(path);
        match start_image_block_export(ImageBlockExportRequest {
            source,
            output: output.clone(),
            options: self.image_generator.options.clone(),
        }) {
            Ok(task_id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.image_generator.output = Some(output);
                self.image_generator.task_id = Some(task_id);
                self.status = SharedString::from("图片方块结构导出任务已开始");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}

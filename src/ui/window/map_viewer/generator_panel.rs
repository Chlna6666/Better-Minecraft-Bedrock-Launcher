use std::path::PathBuf;

use bedrock_voxel::{ObjFill, ObjVoxelOptions};

use super::model::MapViewerWindowView;
use super::prelude::*;
use crate::core::minecraft::map::generator::{
    ObjExportRequest, StructurePreviewKind, StructurePreviewRequest, release_structure_preview,
    start_obj_export, start_structure_preview,
};
use crate::ui::components::slider::Slider;

mod parameters;
mod render;

pub(super) struct GeneratorPanelState {
    source: Option<PathBuf>,
    output: Option<PathBuf>,
    options: ObjVoxelOptions,
    task_id: Option<String>,
    pub(super) preview_task_id: Option<String>,
    pub(super) preview_processed: bool,
    pub(super) preview_size: Option<[i32; 3]>,
    pub(super) preview_result_active: bool,
    pub(super) preview_anchor: Option<ChunkPos>,
    preview_options: Option<ObjVoxelOptions>,
    preview_refresh_task: Option<Task<anyhow::Result<()>>>,
    preview_generation: u64,
    preview_refresh_pending: bool,
}

impl Default for GeneratorPanelState {
    fn default() -> Self {
        Self {
            source: None,
            output: None,
            options: ObjVoxelOptions::default(),
            task_id: None,
            preview_task_id: None,
            preview_processed: false,
            preview_size: None,
            preview_result_active: false,
            preview_anchor: None,
            preview_options: None,
            preview_refresh_task: None,
            preview_generation: 0,
            preview_refresh_pending: false,
        }
    }
}

const OBJ_PREVIEW_REFRESH_DELAY: Duration = Duration::from_millis(180);

impl MapViewerWindowView {
    pub(super) fn generator_export_task_id(&self) -> Option<&str> {
        self.generator.task_id.as_deref()
    }

    fn generator_busy(&self) -> bool {
        self.map_write_busy()
            || self.generator.task_id.as_ref().is_some_and(|id| {
                self.task_snapshots
                    .get(id.as_str())
                    .is_some_and(|snapshot| !snapshot.is_terminal())
            })
    }

    fn choose_generator_obj(&mut self, cx: &mut Context<Self>) {
        if self.generator_busy() {
            return;
        }
        if let Some(path) = pick_file_path_with_filter("Wavefront OBJ", &["obj"]) {
            self.generator.source = Some(PathBuf::from(path));
            self.generator.preview_options = None;
            self.generator.output = None;
            self.generator.task_id = None;
            self.start_obj_live_preview(cx);
            cx.notify();
        }
    }

    pub(super) fn start_obj_live_preview(&mut self, cx: &mut Context<Self>) {
        self.cancel_obj_preview_refresh();
        if self.generator.preview_options == Some(self.generator.options)
            && self
                .generator
                .preview_task_id
                .as_deref()
                .and_then(|id| self.task_snapshots.get(id))
                .is_some_and(|snapshot| !matches!(snapshot.status.as_ref(), "error" | "cancelled"))
        {
            return;
        }
        self.clear_obj_live_preview(cx);
        self.start_obj_live_preview_now(cx);
    }

    pub(super) fn ensure_obj_live_preview(&mut self, cx: &mut Context<Self>) {
        self.cancel_obj_preview_refresh();
        if self.generator.preview_options == Some(self.generator.options)
            && self.generator.preview_task_id.is_some()
        {
            return;
        }
        self.start_obj_live_preview(cx);
    }

    pub(super) fn schedule_obj_live_preview(&mut self, cx: &mut Context<Self>) {
        if self.generator.preview_refresh_pending {
            return;
        }
        self.cancel_obj_preview_refresh();
        self.clear_obj_live_preview(cx);
        self.generator.preview_refresh_pending = true;
        let generation = self.generator.preview_generation;
        self.generator.preview_refresh_task = Some(cx.spawn(async move |handle, cx| {
            Timer::after(OBJ_PREVIEW_REFRESH_DELAY).await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.generator.preview_generation != generation {
                    return;
                }
                this.generator.preview_refresh_pending = false;
                this.start_obj_live_preview_now(cx);
            })?;
            Ok::<(), anyhow::Error>(())
        }));
        cx.notify();
    }

    pub(super) fn cancel_obj_preview_refresh(&mut self) {
        self.generator.preview_generation = self.generator.preview_generation.saturating_add(1);
        drop(self.generator.preview_refresh_task.take());
        self.generator.preview_refresh_pending = false;
    }

    fn clear_obj_live_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.generator.preview_task_id.take() {
            task_manager::cancel_task(&id);
            release_structure_preview(&id);
        }
        if self.generator.source.is_some() {
            self.clear_live_structure_preview(cx);
        }
        self.generator.preview_processed = false;
        self.generator.preview_size = None;
        self.generator.preview_options = None;
    }

    fn start_obj_live_preview_now(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.generator.source.clone() else {
            return;
        };
        let options = self.generator.options;
        self.generator.preview_options = Some(options);
        match start_structure_preview(StructurePreviewRequest {
            source,
            kind: StructurePreviewKind::Obj(options),
        }) {
            Ok(id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.generator.preview_task_id = Some(id);
            }
            Err(error) => self.status = SharedString::from(error),
        }
        cx.notify();
    }

    fn export_generator_obj(&mut self, cx: &mut Context<Self>) {
        if self.generator_busy() {
            return;
        }
        let Some(source) = self.generator.source.clone() else {
            self.status = SharedString::from("请先选择 OBJ 模型");
            cx.notify();
            return;
        };
        let default_name = format!(
            "{}.mcstructure",
            source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("model")
        );
        let Some(path) =
            pick_save_path_with_filter("Bedrock Structure", &["mcstructure"], &default_name)
        else {
            return;
        };
        let output = PathBuf::from(path);
        match start_obj_export(ObjExportRequest {
            source,
            output: output.clone(),
            voxel: self.generator.options,
        }) {
            Ok(task_id) => {
                if let Some(snapshot) = task_manager::get_snapshot(&task_id) {
                    self.task_snapshots
                        .insert(snapshot.id.clone(), Arc::new(snapshot));
                }
                self.generator.output = Some(output);
                self.generator.task_id = Some(task_id);
                self.status = SharedString::from("OBJ 转方块任务已开始");
            }
            Err(error) => {
                self.status = SharedString::from(error.clone());
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}

pub(super) fn generator_section_title(colors: &ThemeColors, title: &'static str) -> Div {
    div()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(title)
}

pub(super) fn generator_choice(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    selected: bool,
) -> Div {
    div()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(colors.border)
        .bg(if selected {
            colors.surface_hover
        } else {
            colors.surface
        })
        .text_size(px(12.0))
        .text_color(colors.text_primary)
        .cursor_pointer()
        .child(label.into())
}

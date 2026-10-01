//! Cancellable in-memory structure previews shared by image and OBJ generators.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use bedrock_voxel::{
    ObjVoxelOptions, default_block_candidates, load_obj_model, voxelize_obj_with_progress,
};
use bedrock_world::{structure::McStructureFile, surface::CancelFlag};

use super::image::{
    ImageBlockHeight, ImageBlockOptions, decode_image, image_block_plan_with_progress,
};
use crate::tasks::{runtime, task_manager};

const PREVIEW_LIMIT_BYTES: usize = 128 * 1024 * 1024;
const PREVIEW_TTL: Duration = Duration::from_secs(15 * 60);

/// Source-specific settings for an in-memory block preview.
#[derive(Clone, Debug)]
pub enum StructurePreviewKind {
    /// Flat north-up image conversion.
    Image(ImageBlockOptions),
    /// OBJ surface or solid voxel conversion.
    Obj(ObjVoxelOptions),
}

/// Preview input. No output path or Minecraft world is opened for writing.
pub struct StructurePreviewRequest {
    /// PNG/JPEG/TGA/WebP image or OBJ path.
    pub source: PathBuf,
    /// Conversion settings.
    pub kind: StructurePreviewKind,
}

#[derive(Default)]
struct PreviewStore {
    entries: HashMap<String, (Arc<Vec<u8>>, Instant)>,
    discarded: HashMap<String, Instant>,
}

static PREVIEWS: OnceLock<Mutex<PreviewStore>> = OnceLock::new();

fn previews() -> &'static Mutex<PreviewStore> {
    PREVIEWS.get_or_init(|| Mutex::new(PreviewStore::default()))
}

impl PreviewStore {
    fn expire(&mut self) {
        self.entries
            .retain(|_, (_, at)| at.elapsed() <= PREVIEW_TTL);
        self.discarded.retain(|_, at| at.elapsed() <= PREVIEW_TTL);
    }

    fn insert(&mut self, id: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.expire();
        if self.discarded.contains_key(id) {
            return Ok(());
        }
        if bytes.len() > PREVIEW_LIMIT_BYTES {
            return Err("结构预览超过 128 MiB 结果上限".to_owned());
        }
        let mut used = self
            .entries
            .values()
            .map(|(bytes, _)| bytes.len())
            .sum::<usize>();
        while used.saturating_add(bytes.len()) > PREVIEW_LIMIT_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, at))| at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            if let Some((removed, _)) = self.entries.remove(&oldest) {
                used -= removed.len();
            }
        }
        self.entries
            .insert(id.to_owned(), (Arc::new(bytes), Instant::now()));
        Ok(())
    }
}

/// Returns serialized `.mcstructure` bytes for the completed task.
///
/// Consume this from a task event, outside GPUI render. The bytes preserve exact
/// converted block states and can be decoded on a background worker.
#[must_use]
pub fn get_structure_preview(task_id: &str) -> Option<Arc<Vec<u8>>> {
    let mut store = previews().lock().ok()?;
    store.expire();
    store.entries.get(task_id).map(|(bytes, _)| bytes.clone())
}

/// Drops a superseded result and prevents its running task from publishing later.
pub fn release_structure_preview(task_id: &str) {
    if let Ok(mut store) = previews().lock() {
        store.entries.remove(task_id);
        store.discarded.insert(task_id.to_owned(), Instant::now());
    }
}

/// Starts a visible preview task that never writes a world or output file.
///
/// Preview bytes are bounded to 128 MiB across tasks and expire after 15 minutes.
/// Cancellation is cooperative through parse, conversion and serialization.
///
/// # Errors
/// Returns a runtime submission error. Parse, conversion, and size failures are
/// reported through the returned task snapshot.
pub fn start_structure_preview(request: StructurePreviewRequest) -> Result<String, String> {
    let name = match &request.kind {
        StructurePreviewKind::Image(ImageBlockOptions {
            depth: ImageBlockHeight::LuminanceRelief { .. },
            ..
        }) => "图片亮度阶梯实时预览",
        StructurePreviewKind::Image(_) => "图片方块实时预览",
        StructurePreviewKind::Obj(_) => "OBJ 方块实时预览",
    };
    let detail = request
        .source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let task_id =
        task_manager::create_task_with_details(None, name, detail, "读取源文件", None, false);
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel = cancel.clone();
        move || cancel.cancel()
    });
    let worker_id = task_id.clone();
    if let Err(error) = runtime::spawn_io(async move {
        let result = generate_preview(request, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(error) if error.contains("已取消") => "cancelled",
            Err(_) => "error",
        };
        task_manager::finish_task(
            &worker_id,
            status,
            Some(match result {
                Ok(message) | Err(message) => message,
            }),
        );
    }) {
        task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }
    Ok(task_id)
}

async fn generate_preview(
    request: StructurePreviewRequest,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let worker_id = task_id.to_owned();
    let worker_cancel = cancel.clone();
    let structure = match request.kind {
        StructurePreviewKind::Image(options) => {
            let source = request.source;
            let decoded = runtime::run_io_blocking(move || decode_image(&source)).await??;
            if cancel.is_cancelled() {
                return Err("图片预览已取消".to_owned());
            }
            let total = u64::from(options.width) * u64::from(options.height);
            let stage = match options.depth {
                ImageBlockHeight::Flat => "匹配方块",
                ImageBlockHeight::LuminanceRelief { .. } => "匹配方块并构建亮度阶梯",
            };
            task_manager::reset_progress(task_id, Some(total), Some(stage));
            runtime::run_cpu(move || {
                let mut previous = 0_u64;
                let mut previous_total = total;
                let plan = image_block_plan_with_progress(
                    &decoded,
                    &options,
                    &worker_cancel,
                    |done, progress_total| {
                        if progress_total != previous_total {
                            task_manager::reset_progress(
                                &worker_id,
                                Some(progress_total),
                                Some(stage),
                            );
                            previous = 0;
                            previous_total = progress_total;
                        }
                        if done > previous {
                            task_manager::update_progress(
                                &worker_id,
                                done - previous,
                                Some(progress_total),
                                Some(stage),
                            );
                            previous = done;
                        }
                    },
                )?;
                McStructureFile::from_placement_plan(&plan).map_err(|error| error.to_string())
            })
            .await??
        }
        StructurePreviewKind::Obj(options) => {
            let source = request.source;
            let model = runtime::run_io_blocking(move || load_obj_model(&source)).await??;
            if cancel.is_cancelled() {
                return Err("OBJ 预览已取消".to_owned());
            }
            task_manager::reset_progress(
                task_id,
                Some(model.triangles.len() as u64),
                Some("体素化三角形"),
            );
            runtime::run_cpu(move || {
                let candidates = default_block_candidates();
                let mut previous = 0;
                let plan = voxelize_obj_with_progress(
                    &model,
                    candidates,
                    options,
                    &worker_cancel,
                    |done, total| {
                        task_manager::update_progress(
                            &worker_id,
                            (done - previous) as u64,
                            Some(total as u64),
                            Some("体素化三角形"),
                        );
                        previous = done;
                    },
                )
                .map_err(|error| error.to_string())?;
                McStructureFile::from_placement_plan(&plan).map_err(|error| error.to_string())
            })
            .await??
        }
    };
    if cancel.is_cancelled() {
        return Err("结构预览已取消".to_owned());
    }
    task_manager::reset_progress(task_id, Some(1), Some("保存预览结果"));
    let bytes =
        runtime::run_cpu(move || structure.to_bytes().map_err(|error| error.to_string())).await??;
    if cancel.is_cancelled() {
        return Err("结构预览已取消".to_owned());
    }
    let size = bytes.len();
    previews()
        .lock()
        .map_err(|_| "结构预览存储不可用".to_owned())?
        .insert(task_id, bytes)?;
    task_manager::update_progress(task_id, 1, Some(1), Some("保存预览结果"));
    Ok(format!("方块预览已生成（{} KiB）", size.div_ceil(1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_task_cannot_publish_a_late_preview() {
        let mut store = PreviewStore::default();
        store.insert("old", vec![1, 2, 3]).expect("initial result");
        store.entries.remove("old");
        store.discarded.insert("old".to_owned(), Instant::now());
        store
            .insert("old", vec![4, 5, 6])
            .expect("discarded result");
        assert!(!store.entries.contains_key("old"));
    }
}

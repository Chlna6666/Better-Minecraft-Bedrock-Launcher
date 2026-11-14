//! Cancellable in-memory structure previews shared by image and OBJ generators.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use bedrock_voxel::{
    ObjVoxelOptions, VoxelizeTexturesRequest, default_block_candidates, voxelize_textured,
};
use bedrock_world::{structure::McStructureFile, surface::CancelFlag};

use super::image::{
    ImageBlockHeight, ImageBlockOptions, decode_image, plan_blocks,
};
use super::model_import::prepare_map_model;
use super::source_obj_preview::ObjSourcePreview;
use crate::tasks::task_manager::ProgressUpdate;
use crate::tasks::{runtime, task_manager};

const PREVIEW_LIMIT_BYTES: usize = 128 * 1024 * 1024;
// .mcstructure serializes two dense i32 block-index layers (8 bytes/cell).
// Check their extent before allocating the dense structure and its NBT copy.
const MAX_PREVIEW_STRUCTURE_CELLS: u64 = 16_000_000;
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
    original_obj: HashMap<String, (Arc<ObjSourcePreview>, Instant)>,
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
        self.original_obj
            .retain(|_, (_, at)| at.elapsed() <= PREVIEW_TTL);
    }

    fn insert_original_obj(&mut self, id: &str, preview: Arc<ObjSourcePreview>) {
        self.expire();
        if self.discarded.contains_key(id) {
            return;
        }
        if self.original_obj.len() >= 2
            && let Some(old) = self
                .original_obj
                .iter()
                .min_by_key(|(_, (_, at))| at)
                .map(|(key, _)| key.clone())
        {
            self.original_obj.remove(&old);
        }
        self.original_obj
            .insert(id.to_owned(), (preview, Instant::now()));
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
pub fn get(task_id: &str) -> Option<Arc<Vec<u8>>> {
    let mut store = previews().lock().ok()?;
    store.expire();
    store.entries.get(task_id).map(|(bytes, _)| bytes.clone())
}

/// Actual source OBJ geometry, never the voxelized structure.
#[must_use]
pub fn get_obj_source(task_id: &str) -> Option<Arc<ObjSourcePreview>> {
    let mut store = previews().lock().ok()?;
    store.expire();
    store
        .original_obj
        .get(task_id)
        .map(|(scene, _)| scene.clone())
}

/// Drops a superseded result and prevents its running task from publishing later.
pub fn release(task_id: &str) {
    if let Ok(mut store) = previews().lock() {
        store.entries.remove(task_id);
        store.original_obj.remove(task_id);
        store.discarded.insert(task_id.to_owned(), Instant::now());
    }
}

fn check_preview_structure_extent(
    plan: &bedrock_world::editor::BlockPlacementPlan,
) -> Result<(), String> {
    let mut min = [i64::MAX; 3];
    let mut max = [i64::MIN; 3];
    for block in plan.blocks() {
        let pos = [
            i64::from(block.offset.x),
            i64::from(block.offset.y),
            i64::from(block.offset.z),
        ];
        for axis in 0..3 {
            min[axis] = min[axis].min(pos[axis]);
            max[axis] = max[axis].max(pos[axis]);
        }
    }
    let mut volume = 1_u64;
    for axis in 0..3 {
        let extent = u64::try_from(max[axis] - min[axis] + 1)
            .map_err(|_| "方块预览尺寸无效".to_owned())?;
        volume = volume
            .checked_mul(extent)
            .ok_or_else(|| "方块预览范围溢出".to_owned())?;
    }
    if volume > MAX_PREVIEW_STRUCTURE_CELLS {
        return Err(format!(
            "模型空心转换成功，但其方块分布范围需要 {} 万格结构数据，超过预览的 128 MiB 内存限制；请降低尺寸或缩小模型包围盒",
            volume.div_ceil(10_000),
        ));
    }
    Ok(())
}

/// Starts a visible preview task that never writes a world or output file.
///
/// Preview bytes are bounded to 128 MiB across tasks and expire after 15 minutes.
/// Cancellation is cooperative through parse, conversion and serialization.
///
/// # Errors
/// Returns a runtime submission error. Parse, conversion, and size failures are
/// reported through the returned task snapshot.
pub fn start(request: StructurePreviewRequest) -> Result<String, String> {
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
    let mut options = task_manager::TaskOptions::new(name, "读取源文件");
    if let Some(detail) = detail {
        options = options.detail(detail);
    }
    let task_id = task_manager::create_task(options)?;
    task_manager::register_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_cancel_hook(task_id.clone(), {
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
    let (structure, original_obj) = match request.kind {
        StructurePreviewKind::Image(options) => {
            let source = request.source;
            let decoded = runtime::blocking::run_io(move || decode_image(&source)).await??;
            if cancel.is_cancelled() {
                return Err("图片预览已取消".to_owned());
            }
            let total = u64::from(options.width) * u64::from(options.height);
            let stage = match options.depth {
                ImageBlockHeight::Flat => "匹配方块",
                ImageBlockHeight::LuminanceRelief { .. } => "匹配方块并构建亮度阶梯",
            };
            task_manager::reset_progress(task_id, Some(total), Some(stage));
            let structure = runtime::run_cpu(move || {
                let mut previous = 0_u64;
                let mut previous_total = total;
                let plan = plan_blocks(
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
                                ProgressUpdate::advance(done - previous)
                                    .total(progress_total)
                                    .stage(stage),
                            );
                            previous = done;
                        }
                    },
                )?;
                McStructureFile::from_placement_plan(&plan).map_err(|error| error.to_string())
            })
            .await??;
            (structure, None)
        }
        StructurePreviewKind::Obj(options) => {
            let source = request.source;
            let prepared = runtime::blocking::run_io(move || prepare_map_model(&source)).await??;
            let super::model_import::PreparedMapModel {
                model,
                decoded,
                preview,
                ..
            } = prepared;
            if cancel.is_cancelled() {
                return Err("OBJ 预览已取消".to_owned());
            }
            task_manager::reset_progress(
                task_id,
                Some(model.triangles.len() as u64),
                Some("体素化三角形"),
            );
            let result = runtime::run_cpu(move || {
                // Reuse the already parsed source; do not read OBJ/MTL twice.
                // Failure to construct a secondary visual scene must not stop conversion.
                let original = preview;
                // Publish the actual source mesh before the expensive voxelization phase.
                // A discarded/cancelled task cannot resurrect the old scene.
                if let Some(scene) = original.as_ref()
                    && let Ok(mut store) = previews().lock()
                {
                    store.insert_original_obj(&worker_id, scene.clone());
                }
                let candidates = default_block_candidates();
                let mut previous = 0;
                let plan = voxelize_textured(
                    VoxelizeTexturesRequest {
                        model: &model,
                        textures: &decoded,
                        candidates,
                        options,
                        cancel: &worker_cancel,
                    },
                    |done, total| {
                        task_manager::update_progress(
                            &worker_id,
                            ProgressUpdate::advance((done - previous) as u64)
                                .total(total as u64)
                                .stage("体素化三角形"),
                        );
                        previous = done;
                    },
                )
                .map_err(|error| error.to_string())?;
                check_preview_structure_extent(&plan)?;
                let structure = McStructureFile::from_placement_plan(&plan)
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>((structure, original))
            })
            .await??;
            result
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
    let mut store = previews()
        .lock()
        .map_err(|_| "结构预览存储不可用".to_owned())?;
    store.insert(task_id, bytes)?;
    if let Some(original) = original_obj {
        store.insert_original_obj(task_id, original);
    }
    task_manager::update_progress(
        task_id,
        ProgressUpdate::advance(1).total(1).stage("保存预览结果"),
    );
    Ok(format!("方块预览已生成（{} KiB）", size.div_ceil(1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_structure_extent_limits_dense_serialization_not_surface_voxelization() {
        use bedrock_world::editor::{BlockOffset, BlockPlacementPlan, PlacementBlock};
        use bedrock_world::block::BlockState;
        use std::collections::BTreeMap;
        let block = |x, y, z| PlacementBlock {
            offset: BlockOffset { x, y, z },
            state: BlockState {
                name: "minecraft:stone".to_owned(),
                states: BTreeMap::new(),
                version: None,
            },
        };
        // 324 × 256 × 100 still fits the 128 MiB two-layer format.
        let accepted = BlockPlacementPlan::new(vec![
            block(0, 0, 0), block(323, 255, 99),
        ]).expect("placement");
        super::check_preview_structure_extent(&accepted).expect("bounded structure");
        // The sparse voxelizer can represent this, but materializing a
        // 384³ .mcstructure would allocate hundreds of MiB for empty space.
        let enormous = BlockPlacementPlan::new(vec![
            block(0, 0, 0), block(383, 383, 383),
        ]).expect("placement");
        let error = super::check_preview_structure_extent(&enormous).unwrap_err();
        assert!(error.contains("128 MiB"), "{error}");
    }

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

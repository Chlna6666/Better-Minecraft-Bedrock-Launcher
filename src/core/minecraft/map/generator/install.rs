//! Installs a completed map image preview into one LevelDB-backed world target.

use std::{path::PathBuf, sync::Arc};

use bedrock_world::{
    BlockPos, ChunkPos, MapBlockContainerImportPlan, MapFrameImportPlan, MapPlayerImportPlan,
    OpenOptions, PlayerId, World,
    block::BlockState,
    map_item::{MapBundle, filled_map_id},
    nbt::{NbtTag, parse_root_nbt},
    surface::CancelFlag,
};
use indexmap::IndexMap;
use std::collections::BTreeSet;

use super::{
    map_bundle::build_bundle,
    map_image::{MapPreviewResult, get_map_preview},
};
use crate::tasks::{runtime, task_manager};

enum InstallSource {
    Preview(Arc<MapPreviewResult>, String),
    File(PathBuf),
}

/// A selected player or an existing block entity selected in Map Viewer.
pub enum MapBundleInstallTarget {
    /// The `~local_player` or `player_<xuid>` LevelDB inventory record.
    Player(PlayerId),
    /// Detect an existing chest, shulker box or empty frame at this position.
    Block(ChunkPos, BlockPos),
    /// Create the bundle's upward-facing frame grid around this center position.
    NewUpFrame(ChunkPos, BlockPos, BlockState),
}

/// UI-owned history capture for the new `map_<id>` keys and destination record.
///
/// The first callback runs after IDs are chosen and before any write. It must return a callback
/// that persists the after snapshot. Capture failure aborts the import. No GPUI state may be
/// captured by either callback because both run on the application's I/O worker.
pub type MapInstallHistory = Box<
    dyn FnOnce(
            Vec<i64>,
            BTreeSet<ChunkPos>,
        ) -> Result<Box<dyn FnOnce() -> Result<(), String> + Send>, String>
        + Send,
>;

enum ImportedBundle {
    Portable(MapBundle),
    ContainerItems(Vec<NbtTag>),
}

enum Prepared {
    Player(MapPlayerImportPlan),
    Container(MapBlockContainerImportPlan),
    Frame(MapFrameImportPlan),
}

impl Prepared {
    fn map_ids(&self) -> Vec<i64> {
        match self {
            Self::Player(plan) => plan.map_ids().to_vec(),
            Self::Container(plan) => plan.map_ids().to_vec(),
            Self::Frame(plan) => plan.map_ids().to_vec(),
        }
    }

    fn affected_chunks(&self) -> BTreeSet<ChunkPos> {
        match self {
            Self::Player(_) => BTreeSet::new(),
            Self::Container(plan) => plan.affected_chunks(),
            Self::Frame(plan) => plan.affected_chunks(),
        }
    }

    fn commit(self, world: &World) -> Result<(), String> {
        match self {
            Self::Player(plan) => plan.commit(world),
            Self::Container(plan) => plan.commit(world),
            Self::Frame(plan) => plan.commit(world),
        }
        .map_err(|error| error.to_string())
    }
}

/// Starts a visible task to build the preview's map records and install them atomically.
///
/// The world is opened writable only on the worker. Cancellation is accepted until the single
/// LevelDB batch begins; after that the task reports the actual result and Undo uses the supplied
/// history capture. The source image and preview tiles are never modified.
///
/// # Errors
///
/// Returns a missing/expired preview or worker-submission error. Preparation, conflicts, capacity,
/// storage and history failures appear on the returned task's terminal snapshot.
pub fn start_map_bundle_install(
    preview_task_id: &str,
    world_path: PathBuf,
    title: String,
    target: MapBundleInstallTarget,
    history: MapInstallHistory,
) -> Result<String, String> {
    let preview = get_map_preview(preview_task_id)
        .ok_or_else(|| "地图预览已释放或过期，请重新转换".to_owned())?;
    start_install_task(
        InstallSource::Preview(preview, title.clone()),
        world_path,
        title,
        target,
        history,
    )
}

/// Imports a BMCBL map bundle or ordinary Bedrock container NBT.
///
/// Ordinary containers contribute only their filled-map items. Their `map_uuid` references must
/// resolve to pixel records already present in the target world; cross-world transfers require a
/// BMCBL bundle, which includes those records. Imported IDs are remapped when the target world is
/// prepared. The source file and existing target records are not overwritten.
///
/// # Errors
///
/// Returns a worker-submission error. File read, decode and world errors are task failures.
pub fn start_map_bundle_file_install(
    input: PathBuf,
    world_path: PathBuf,
    target: MapBundleInstallTarget,
    history: MapInstallHistory,
) -> Result<String, String> {
    let title = input.file_name().map_or_else(
        || "地图包".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    start_install_task(
        InstallSource::File(input),
        world_path,
        title,
        target,
        history,
    )
}

fn start_install_task(
    source: InstallSource,
    world_path: PathBuf,
    title: String,
    target: MapBundleInstallTarget,
    history: MapInstallHistory,
) -> Result<String, String> {
    let (phase, total) = match &source {
        InstallSource::Preview(preview, _) => ("生成地图记录", preview.tiles.len() as u64),
        InstallSource::File(_) => ("读取地图包", 1),
    };
    let task_id = task_manager::create_task_with_details(
        None,
        "写入地图包",
        Some(title),
        phase,
        Some(total),
        false,
    );
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel = cancel.clone();
        move || cancel.cancel()
    });
    let worker_id = task_id.clone();
    if let Err(error) = runtime::spawn_io(async move {
        let result = install(source, world_path, target, history, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(message) if message.contains("已取消") => "cancelled",
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

async fn install(
    source: InstallSource,
    world_path: PathBuf,
    target: MapBundleInstallTarget,
    history: MapInstallHistory,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let bundle = match source {
        InstallSource::Preview(preview, title) => {
            let worker_cancel = cancel.clone();
            let worker_id = task_id.to_owned();
            ImportedBundle::Portable(
                runtime::run_cpu(move || {
                    let total = preview.tiles.len() as u64;
                    build_bundle(&preview, &title, &worker_cancel, |_done| {
                        task_manager::update_progress(
                            &worker_id,
                            1,
                            Some(total),
                            Some("生成地图记录"),
                        );
                    })
                })
                .await??,
            )
        }
        InstallSource::File(input) => {
            let imported = runtime::run_io_blocking(move || read_map_import(&input)).await??;
            task_manager::update_progress(task_id, 1, Some(1), Some("读取地图包"));
            imported
        }
    };
    if cancel.is_cancelled() {
        return Err("地图包写入已取消".to_owned());
    }
    task_manager::reset_progress(task_id, Some(1), Some("检查目标与地图 ID"));
    let worker_cancel = cancel.clone();
    let task_id = task_id.to_owned();
    let result = runtime::run_io_blocking(move || {
        let mut options = OpenOptions::default();
        options.read_only = false;
        let world = World::open(&world_path, options).map_err(|error| error.to_string())?;
        let bundle = match bundle {
            ImportedBundle::Portable(bundle) => bundle,
            ImportedBundle::ContainerItems(items) => resolve_container_maps(&world, items)?,
        };
        let plan = match target {
            MapBundleInstallTarget::Player(player) => Prepared::Player(
                world
                    .prepare_map_bundle_player(&bundle, &player)
                    .map_err(|error| error.to_string())?,
            ),
            MapBundleInstallTarget::Block(chunk, position) => {
                let block = world
                    .block_state(chunk.dimension, position)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "选中位置没有可用方块".to_owned())?;
                if block.name == "minecraft:frame" {
                    Prepared::Frame(
                        world
                            .prepare_map_bundle_frame(&bundle, chunk, position)
                            .map_err(|error| error.to_string())?,
                    )
                } else {
                    Prepared::Container(
                        world
                            .prepare_map_bundle_container(&bundle, chunk, position)
                            .map_err(|error| error.to_string())?,
                    )
                }
            }
            MapBundleInstallTarget::NewUpFrame(chunk, position, support) => Prepared::Frame(
                world
                    .prepare_map_bundle_new_up_frame(&bundle, chunk, position, support)
                    .map_err(|error| error.to_string())?,
            ),
        };
        let map_ids = plan.map_ids();
        let affected_chunks = plan.affected_chunks();
        task_manager::update_progress(&task_id, 1, Some(1), Some("检查目标与地图 ID"));
        if worker_cancel.is_cancelled() {
            return Err("地图包写入已取消".to_owned());
        }
        task_manager::reset_progress(&task_id, Some(1), Some("创建撤销快照"));
        let complete_history = history(map_ids.clone(), affected_chunks)?;
        task_manager::update_progress(&task_id, 1, Some(1), Some("创建撤销快照"));
        if worker_cancel.is_cancelled() {
            return Err("地图包写入已取消".to_owned());
        }
        task_manager::reset_progress(&task_id, Some(1), Some("原子提交地图与物品"));
        plan.commit(&world)?;
        task_manager::update_progress(&task_id, 1, Some(1), Some("原子提交地图与物品"));
        task_manager::reset_progress(&task_id, Some(1), Some("保存撤销历史"));
        complete_history().map_err(|error| format!("地图已写入，但撤销历史保存失败：{error}"))?;
        task_manager::update_progress(&task_id, 1, Some(1), Some("保存撤销历史"));
        Ok::<_, String>(format!(
            "已写入 {} 张地图，ID：{:?}",
            map_ids.len(),
            map_ids
        ))
    })
    .await?;
    result
}

fn read_map_import(input: &std::path::Path) -> Result<ImportedBundle, String> {
    let metadata = std::fs::metadata(input).map_err(|error| error.to_string())?;
    if metadata.len() > 256 * 1024 * 1024 {
        return Err("地图 NBT 超过 256 MiB 上限".to_owned());
    }
    let bytes = std::fs::read(input).map_err(|error| error.to_string())?;
    let portable_error = match MapBundle::from_nbt_bytes(&bytes) {
        Ok(bundle) => return Ok(ImportedBundle::Portable(bundle)),
        Err(error) => error.to_string(),
    };
    let root = parse_root_nbt(&bytes)
        .map_err(|error| format!("文件不是可读取的 Bedrock NBT 地图包或容器：{error}"))?;
    if matches!(
        &root,
        NbtTag::Compound(fields)
            if matches!(fields.get("format"), Some(NbtTag::String(format)) if format.starts_with("BMCBL"))
    ) {
        return Err(format!("BMCBL 地图包无效：{portable_error}"));
    }
    let mut items = Vec::new();
    let mut ids = BTreeSet::new();
    collect_filled_maps(&root, &mut items, &mut ids)?;
    if items.is_empty() {
        return Err("普通容器 NBT 中没有 minecraft:filled_map 物品".to_owned());
    }
    Ok(ImportedBundle::ContainerItems(items))
}

fn collect_filled_maps(
    tag: &NbtTag,
    items: &mut Vec<NbtTag>,
    ids: &mut BTreeSet<String>,
) -> Result<(), String> {
    match tag {
        NbtTag::Compound(fields) => {
            if let Some(id) = filled_map_id(tag) {
                if !ids.insert(id.as_str().to_owned()) {
                    return Err(format!("容器 NBT 重复引用地图 map_{}", id.as_str()));
                }
                if items.len() == 1024 {
                    return Err("单次最多导入 1024 张地图".to_owned());
                }
                items.push(tag.clone());
                return Ok(());
            }
            for child in fields.values() {
                collect_filled_maps(child, items, ids)?;
            }
        }
        NbtTag::List(children) => {
            for child in children {
                collect_filled_maps(child, items, ids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_container_maps(world: &World, items: Vec<NbtTag>) -> Result<MapBundle, String> {
    let mut records = Vec::with_capacity(items.len());
    for item in &items {
        let id = filled_map_id(item).ok_or_else(|| "容器 NBT 包含无法读取的地图引用".to_owned())?;
        let record = world
            .map_item(&id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| {
                format!(
                    "普通容器 NBT 引用 map_{}，但目标世界没有这张地图的像素记录；请从源世界导出 BMCBL 可移植地图包",
                    id.as_str()
                )
            })?;
        records.push(record);
    }
    let container = NbtTag::Compound(IndexMap::from([("Items".to_owned(), NbtTag::List(items))]));
    MapBundle::new(records, container).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::map_item::new_filled_map_item;

    #[test]
    fn collects_maps_from_chest_and_nested_shulker_items() {
        let nested = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_owned(),
                NbtTag::String("minecraft:undyed_shulker_box".to_owned()),
            ),
            (
                "tag".to_owned(),
                NbtTag::Compound(IndexMap::from([(
                    "Items".to_owned(),
                    NbtTag::List(vec![new_filled_map_item(12, 0).expect("map item")]),
                )])),
            ),
        ]));
        let root = NbtTag::Compound(IndexMap::from([(
            "Items".to_owned(),
            NbtTag::List(vec![new_filled_map_item(11, 0).expect("map item"), nested]),
        )]));
        let mut items = Vec::new();
        let mut ids = BTreeSet::new();
        collect_filled_maps(&root, &mut items, &mut ids).expect("collect maps");
        assert_eq!(items.len(), 2);
        assert!(ids.contains("11"));
        assert!(ids.contains("12"));
    }

    #[test]
    fn rejects_repeated_map_references_in_container_nbt() {
        let root = NbtTag::Compound(IndexMap::from([(
            "Items".to_owned(),
            NbtTag::List(vec![
                new_filled_map_item(11, 0).expect("map item"),
                new_filled_map_item(11, 1).expect("map item"),
            ]),
        )]));
        let error = collect_filled_maps(&root, &mut Vec::new(), &mut BTreeSet::new())
            .expect_err("duplicate reference");
        assert!(error.contains("重复引用"));
    }
}

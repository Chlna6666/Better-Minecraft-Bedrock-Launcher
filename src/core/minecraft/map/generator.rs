//! Application-owned OBJ conversion and `.mcstructure` export.

use std::path::PathBuf;

use bedrock_voxel::{
    ObjVoxelOptions, default_block_candidates, voxelize_obj_with_textures_and_progress,
};
use bedrock_world::{structure::McStructureFile, surface::CancelFlag};

use crate::tasks::{runtime, task_manager};

mod image;
mod install;
mod map_bundle;
mod map_image;
mod model_import;
mod model_export;
mod source_obj_preview;
mod structure_preview;

pub use image::{
    Dithering, ImageBlockExportRequest, ImageBlockHeight, ImageBlockOptions, ImageFit,
    ReliefImageFill, approved_block_names, start_image_block_export,
};
pub use install::{
    MapBundleInstallTarget, MapInstallHistory, start_map_bundle_file_install,
    start_map_bundle_install, start_map_frame_grid_preflight,
};
pub use map_bundle::start_map_bundle_export;
pub use source_obj_preview::ObjSourcePreview;

/// Whether a path is a supported importable model source.
pub fn is_supported_model(path: &std::path::Path) -> bool {
    model_import::recognized_model(path)
}

/// Convert a locally exported OBJ/MTL model into a requested 3D file format.
/// Caller must execute on a background worker; no UI-thread or GPU work occurs.
pub fn convert_local_model_file(
    source: &std::path::Path,
    output: &std::path::Path,
    cancel: &CancelFlag,
) -> Result<(), String> {
    model_export::export_model_path(source, output, cancel)
}

pub use map_image::{
    MapPreviewOptions, get_map_preview, get_map_preview_frame, release_map_preview,
    start_map_image_preview,
};
pub use structure_preview::{
    StructurePreviewKind, StructurePreviewRequest, get_structure_preview,
    get_obj_source_preview, release_structure_preview, start_structure_preview,
};

/// Inputs for one OBJ conversion and Bedrock structure export.
pub struct ObjExportRequest {
    /// Local model path; MTL and textures must resolve inside its directory.
    pub source: PathBuf,
    /// Output `.mcstructure` path selected by the user.
    pub output: PathBuf,
    /// Model size, fill, transparency and background settings.
    pub voxel: ObjVoxelOptions,
}

/// Starts a visible, cancellable OBJ export task and returns its task ID.
///
/// Model files are only read. The selected output file is written after
/// conversion succeeds; no Minecraft world is opened or modified. Cancellation
/// before the write starts discards the computed placement. Once writing has
/// started, the worker finishes and reports the actual result.
///
/// # Errors
/// Returns a runtime-submission error. Parsing, conversion and output failures
/// are reported on the returned TaskManager task.
pub fn start_obj_export(request: ObjExportRequest) -> Result<String, String> {
    let detail = request
        .source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let task_id = task_manager::create_task_with_details(
        None,
        "OBJ 转方块并导出结构",
        detail,
        "解析 OBJ",
        None,
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
        let result = export_obj(request, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(error) if error.contains("已取消") || error.contains("cancelled") => "cancelled",
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

async fn export_obj(
    request: ObjExportRequest,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let source = request.source;
    let prepared = runtime::run_io_blocking(move || {
        model_import::prepare_model_for_export(&source)
    }).await??;
    let model_import::PreparedMapModel { model, decoded, .. } = prepared;
    if cancel.is_cancelled() {
        return Err("OBJ 转换已取消".to_owned());
    }
    let triangle_count = model.triangles.len();
    let total = triangle_count as u64;
    task_manager::reset_progress(task_id, Some(total), Some("体素化三角形"));
    let worker_id = task_id.to_owned();
    let worker_cancel = cancel.clone();
    let (structure, block_count) = runtime::run_cpu(move || {
        let candidates = default_block_candidates();
        if candidates.is_empty() {
            return Err("没有可用的普通实心方块颜色".to_owned());
        }
        let mut previous = 0_usize;
        let plan = voxelize_obj_with_textures_and_progress(
            &model,
            &decoded,
            candidates,
            request.voxel,
            &worker_cancel,
            |completed, total| {
                task_manager::update_progress(
                    &worker_id,
                    (completed - previous) as u64,
                    Some(total as u64),
                    Some("体素化三角形"),
                );
                previous = completed;
            },
        )
        .map_err(|error| error.to_string())?;
        let block_count = plan.blocks().len();
        let structure =
            McStructureFile::from_placement_plan(&plan).map_err(|error| error.to_string())?;
        Ok::<_, String>((structure, block_count))
    })
    .await??;
    if cancel.is_cancelled() {
        return Err("OBJ 转换已取消".to_owned());
    }
    task_manager::reset_progress(task_id, Some(1), Some("写入 .mcstructure"));
    let output = request.output;
    let output_for_worker = output.clone();
    runtime::run_io_blocking(move || structure.write_to_path(&output_for_worker))
        .await?
        .map_err(|error| error.to_string())?;
    task_manager::update_progress(task_id, 1, Some(1), Some("写入 .mcstructure"));
    Ok(format!(
        "已导出 {} 个三角形、{} 个方块到 {}",
        triangle_count,
        block_count,
        output.display()
    ))
}


/// Request to export an imported model as OBJ(+MTL), glTF or GLB.
/// This preserves the user's original .mcstructure export option separately.
pub struct ModelFileExportRequest {
    pub source: PathBuf,
    pub output: PathBuf,
}

/// Submit cancellable model serialization through TaskManager.
pub fn start_model_file_export(request: ModelFileExportRequest) -> Result<String,String> {
    if !model_import::recognized_model(&request.source) {
        return Err("请选择 OBJ、glTF 或 GLB 模型文件".into());
    }
    let supported = request.output.extension().and_then(|x|x.to_str())
        .is_some_and(|x|matches!(x.to_ascii_lowercase().as_str(),"obj"|"gltf"|"glb"));
    if !supported {return Err("模型只能导出 OBJ、glTF、GLB 格式".into());}
    if request.source == request.output {
        return Err("不能覆盖当前正在读取的源模型文件".into());
    }
    let name=request.output.file_name().map(|s|s.to_string_lossy().into_owned());
    let task_id=task_manager::create_task_with_details(
        None,"模型格式导出",name,"读取模型",None,false,
    );
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel=CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel=cancel.clone();
        move ||cancel.cancel()
    });
    let worker_id=task_id.clone();
    if let Err(error)=runtime::spawn_io(async move {
        let result=async {
            let source=request.source;
            let prepared=runtime::run_io_blocking(move||model_import::prepare_model_for_export(&source)).await??;
            if cancel.is_cancelled(){return Err::<String,String>("模型导出已取消".into());}
            task_manager::reset_progress(&worker_id,Some(1),Some("序列化模型与纹理"));
            let output=request.output;
            let cpu_cancel=cancel.clone();
            runtime::run_cpu(move||{
                model_export::write_model(&prepared.model,&prepared.decoded,&output,&cpu_cancel)?;
                Ok::<_,String>(format!("模型导出完成：{}",output.display()))
            }).await?
        }.await;
        let status=match &result {
            Ok(_)=>"completed",
            Err(err) if err.contains("已取消")=>"cancelled",
            Err(_)=>"error",
        };
        task_manager::finish_task(&worker_id,status,Some(match result {
            Ok(message)=>message,
            Err(error)=>error,
        }));
    }) {
        task_manager::finish_task(&task_id,"error",Some(error.clone()));
        return Err(error);
    }
    Ok(task_id)
}

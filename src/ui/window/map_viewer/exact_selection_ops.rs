use super::model::*;
use super::prelude::*;
use super::preview_3d::{
    Preview3dMesh, load_preview_3d_mesh_blocking_incremental,
    load_preview_3d_mesh_blocking_incremental_with_block_models,
};
use super::preview_3d_obj::export_preview_3d_obj_with_materials_with_progress;
use crate::ui::state::launcher::LauncherState;
use crate::ui::state::local_versions::LocalVersionsState;
use ::bedrock_world::{ExactChunkSelection, query_selection_stats_exact_blocking};
use bedrock_block_model::BlockModelRepository;
use bedrock_render::ExactChunkRenderPlan;
use std::collections::BTreeSet;

impl MapViewerWindowView {
    /// Runs professional statistics against the exact selected chunk set instead
    /// of expanding the selection to its bounding rectangle.
    pub(super) fn query_selection_stats_exact(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("请先选择区块");
            cx.notify();
            return;
        };
        let chunks = selection.chunks();
        let Ok(exact_selection) = ExactChunkSelection::new(chunks.clone()) else {
            self.status = SharedString::from("当前选区没有有效区块");
            cx.notify();
            return;
        };

        let options = self.professional_overlay_query_options();
        let generation = self.metadata_generation;
        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        self.professional.selection_stats = None;
        self.status = SharedString::from(format!(
            "正在统计精确选区 · {} chunks...",
            exact_selection.len()
        ));
        cx.notify();

        let requested_chunks = exact_selection.to_vec();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let world = BedrockWorld::open_blocking(
                        &world_path,
                        ::bedrock_world::BedrockWorldOpenOptions::default(),
                    )
                    .map_err(|error| error.to_string())?;
                    query_selection_stats_exact_blocking(&world, &exact_selection, options)
                        .map_err(|error| error.to_string())
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.metadata_generation != generation
                    || this.professional.selection.map(ChunkSelection::chunks)
                        != Some(requested_chunks.clone())
                    || this.professional_overlay_query_options() != options
                {
                    return;
                }
                match result {
                    Ok(stats) => {
                        let exact_chunk_count = stats.chunk_count;
                        this.professional.selection_stats = Some(stats);
                        this.status = SharedString::from(format!(
                            "精确选区统计完成 · {exact_chunk_count} chunks"
                        ));
                    }
                    Err(error) => {
                        this.status = SharedString::from(format!("精确选区统计失败: {error}"));
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    /// Loads the 3D preview from the exact selected chunks. The selection is
    /// decomposed by the public render plan; holes are never queried.
    pub(super) fn refresh_preview_3d_exact(&mut self, cx: &mut Context<Self>) {
        self.preview_3d.source = Preview3dSource::Selection;
        let Some(selection) = self.professional.selection else {
            self.clear_preview_3d_resources(false);
            self.status = SharedString::from("请先选择 chunk 范围");
            cx.notify();
            return;
        };
        let Ok(exact_selection) = ExactChunkSelection::new(selection.chunks()) else {
            self.clear_preview_3d_resources(false);
            self.status = SharedString::from("当前选区没有可加载的 chunk");
            cx.notify();
            return;
        };
        let signature = Preview3dSelectionSignature {
            bounds: exact_selection.bounds(),
        };
        let chunk_count = exact_selection.len();

        self.preview_3d.generation = self.preview_3d.generation.saturating_add(1);
        let generation = self.preview_3d.generation;
        if let Some(cancel) = self.preview_3d.cancel.take() {
            cancel.cancel();
        }
        let preview_cancel = CancelFlag::new();
        let preview_cancel_for_load = preview_cancel.clone();
        let preview_cancel_for_owner = preview_cancel.clone();
        self.preview_3d.status = Preview3dStatus::Loading(Preview3dBuildStatus::new(
            "准备精确选区",
            format!("{chunk_count} chunks"),
        ));
        self.preview_3d.signature = Some(signature);
        self.preview_3d.mesh = None;
        #[cfg(target_os = "windows")]
        self.preview_3d.clear_surface();
        self.preview_3d.reset_view_and_model();
        self.preview_3d.render_in_flight = true;
        self.preview_3d.cancel = Some(preview_cancel);
        self.status = SharedString::from(format!("正在加载精确 3D 预览 · {chunk_count} chunks..."));
        cx.notify();

        let world_path = self.world_path.clone();
        let query_budget = self.map_query_budget.clone();
        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let (event_sender, mut event_receiver) = unbounded::<Preview3dLoadEvent>();
            let complete_sender = event_sender.clone();
            let load_task = cx.background_spawn(async move {
                let result = load_preview_3d_mesh_exact_blocking_incremental(
                    &world_path,
                    exact_selection,
                    Some(preview_cancel_for_load),
                    {
                        let event_sender = event_sender.clone();
                        move |mesh, status| {
                            if event_sender
                                .unbounded_send(Preview3dLoadEvent::Chunk { mesh, status })
                                .is_err()
                            {
                                tracing::debug!("exact preview 3d incremental receiver dropped");
                            }
                        }
                    },
                )
                .map(Arc::new);
                if complete_sender
                    .unbounded_send(Preview3dLoadEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("exact preview 3d completion receiver dropped");
                }
            });

            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(event, Preview3dLoadEvent::Complete(_));
                let Some(view) = handle.upgrade() else {
                    preview_cancel_for_owner.cancel();
                    load_task.detach();
                    return Ok(());
                };
                view.update(cx, move |this, cx| {
                    if this.preview_3d.generation != generation {
                        return;
                    }
                    match event {
                        Preview3dLoadEvent::Chunk { mesh, status } => {
                            this.preview_3d.mesh = Some(mesh);
                            this.preview_3d.status = Preview3dStatus::Loading(status.clone());
                            this.status = SharedString::from(format!(
                                "正在拼接精确 3D 预览: {} {}",
                                status.phase, status.detail
                            ));
                        }
                        Preview3dLoadEvent::Complete(result) => {
                            this.finish_preview_3d_load(result);
                            this.preview_3d.cancel = None;
                        }
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            load_task.await;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    /// Exports OBJ from the exact selected chunks. Bounding-box holes and
    /// disconnected gaps are not loaded into the mesh.
    pub(super) fn export_selection_as_obj_exact(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.professional.selection else {
            self.status = SharedString::from("没有可导出的选区");
            cx.notify();
            return;
        };
        let Ok(exact_selection) = ExactChunkSelection::new(selection.chunks()) else {
            self.status = SharedString::from("没有可导出的 chunk");
            cx.notify();
            return;
        };
        let bounds = exact_selection.bounds();
        let default_file_name = format!(
            "chunk-selection-{}-{}-{}-{}.obj",
            bounds.min_chunk_x, bounds.min_chunk_z, bounds.max_chunk_x, bounds.max_chunk_z
        );
        let Some(path) = pick_save_path_with_filter("Wavefront OBJ", &["obj"], &default_file_name)
        else {
            self.status = SharedString::from("已取消导出 OBJ");
            cx.notify();
            return;
        };
        let path = PathBuf::from(path);
        let world_path = self.world_path.clone();
        let package_paths = exact_preview_3d_resource_package_paths(&world_path, cx);
        let query_budget = self.map_query_budget.clone();
        let chunk_count = exact_selection.len();
        self.context_menu = None;
        self.status = SharedString::from(format!("正在导出精确选区 OBJ · {chunk_count} chunks..."));
        cx.notify();

        cx.spawn(async move |handle, cx| {
            let _query_permit = query_budget.acquire().await;
            let result = cx
                .background_spawn(async move {
                    let mut resolved_package_paths =
                        bedrock_block_model::world_resource_pack_paths(&world_path);
                    for package_path in package_paths {
                        bedrock_block_model::push_unique_resource_pack_path(
                            &mut resolved_package_paths,
                            package_path,
                        );
                    }
                    for package_path in
                        crate::core::minecraft::paths::discover_local_package_roots_with_vanilla()
                    {
                        bedrock_block_model::push_unique_resource_pack_path(
                            &mut resolved_package_paths,
                            package_path,
                        );
                    }
                    let block_models = BlockModelRepository::load_packs(
                        resolved_package_paths.iter().map(PathBuf::as_path),
                    )
                    .map(Arc::new)
                    .map_err(|error| format!("加载方块模型资源失败：{error}"))?;
                    let mesh = load_preview_3d_mesh_exact_blocking_incremental_with_block_models(
                        &world_path,
                        exact_selection,
                        Some(block_models),
                        None,
                        |_mesh, _status| {},
                    )?;
                    let export_target = bedrock_block_model::ObjExportTarget::from_obj_path(&path)
                        .map_err(|error| error.to_string())?;
                    let export = export_preview_3d_obj_with_materials_with_progress(
                        &mesh,
                        &export_target.material_library_name,
                        "textures",
                        &resolved_package_paths,
                        |_completed, _total| {},
                    );
                    bedrock_block_model::write_obj_export_files(
                        &export,
                        &export_target.obj_path,
                        &export_target.material_library_path,
                        &export_target.export_root,
                    )
                    .map_err(|error| error.to_string())?;
                    Ok::<_, String>((export_target.export_root, mesh.chunk_count))
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                match result {
                    Ok((export_root, exact_chunk_count)) => {
                        this.status = SharedString::from(format!(
                            "精确选区 OBJ 导出完成 · {exact_chunk_count} chunks · {}",
                            export_root.display()
                        ));
                    }
                    Err(error) => {
                        this.status = SharedString::from(format!("精确选区 OBJ 导出失败: {error}"));
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

pub(super) fn load_preview_3d_mesh_exact_blocking_incremental(
    world_path: &Path,
    selection: ExactChunkSelection,
    cancel: Option<CancelFlag>,
    update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    load_preview_3d_mesh_exact_impl(world_path, selection, None, cancel, update)
}

pub(super) fn load_preview_3d_mesh_exact_blocking_incremental_with_block_models(
    world_path: &Path,
    selection: ExactChunkSelection,
    block_models: Option<Arc<BlockModelRepository>>,
    cancel: Option<CancelFlag>,
    update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    load_preview_3d_mesh_exact_impl(world_path, selection, block_models, cancel, update)
}

fn load_preview_3d_mesh_exact_impl(
    world_path: &Path,
    selection: ExactChunkSelection,
    block_models: Option<Arc<BlockModelRepository>>,
    cancel: Option<CancelFlag>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    let plan = ExactChunkRenderPlan::new(selection);
    let chunks = plan.positions().to_vec();
    let rectangles = plan.rectangle_cover().to_vec();
    let total_chunks = plan.chunk_count();
    let total_rectangles = rectangles.len();
    let mut parts = Vec::with_capacity(total_rectangles);
    let mut completed_chunks = 0usize;

    for (index, bounds) in rectangles.into_iter().enumerate() {
        if cancel.as_ref().is_some_and(CancelFlag::is_cancelled) {
            return Err("3D 预览已取消".to_string());
        }
        let part = if let Some(block_models) = block_models.clone() {
            load_preview_3d_mesh_blocking_incremental_with_block_models(
                world_path,
                bounds,
                Some(block_models),
                cancel.clone(),
                |_mesh, _status| {},
            )?
        } else {
            load_preview_3d_mesh_blocking_incremental(
                world_path,
                bounds,
                cancel.clone(),
                |_mesh, _status| {},
            )?
        };
        completed_chunks = completed_chunks.saturating_add(bounds.chunk_count());
        parts.push(part);
        let merged = merge_exact_preview_meshes(&parts, &chunks);
        update(
            Arc::new(merged),
            Preview3dBuildStatus::new(
                "精确选区",
                format!(
                    "{}/{} chunks · 子区域 {}/{}",
                    completed_chunks.min(total_chunks),
                    total_chunks,
                    index + 1,
                    total_rectangles
                ),
            ),
        );
    }

    Ok(merge_exact_preview_meshes(&parts, &chunks))
}

fn merge_exact_preview_meshes(parts: &[Preview3dMesh], chunks: &[ChunkPos]) -> Preview3dMesh {
    let min_chunk_x = chunks.iter().map(|chunk| chunk.x).min().unwrap_or(0);
    let max_chunk_x = chunks.iter().map(|chunk| chunk.x).max().unwrap_or(0);
    let min_chunk_z = chunks.iter().map(|chunk| chunk.z).min().unwrap_or(0);
    let max_chunk_z = chunks.iter().map(|chunk| chunk.z).max().unwrap_or(0);
    let min_y = parts
        .iter()
        .filter(|mesh| mesh.surface_face_count() > 0)
        .map(|mesh| mesh.min_y)
        .min()
        .unwrap_or(0);
    let max_y = parts
        .iter()
        .filter(|mesh| mesh.surface_face_count() > 0)
        .map(|mesh| mesh.max_y)
        .max()
        .unwrap_or(0);

    Preview3dMesh {
        chunk_meshes: parts
            .iter()
            .flat_map(|mesh| mesh.chunk_meshes.iter().cloned())
            .collect(),
        min_y,
        max_y,
        min_x: min_chunk_x.saturating_mul(16),
        max_x: max_chunk_x
            .saturating_add(1)
            .saturating_mul(16)
            .saturating_sub(1),
        min_z: min_chunk_z.saturating_mul(16),
        max_z: max_chunk_z
            .saturating_add(1)
            .saturating_mul(16)
            .saturating_sub(1),
        missing_chunks: parts.iter().map(|mesh| mesh.missing_chunks).sum(),
        chunk_count: chunks.len(),
        processed_chunk_count: parts
            .iter()
            .map(|mesh| mesh.processed_chunk_count)
            .sum::<usize>()
            .min(chunks.len()),
        subchunk_count: parts.iter().map(|mesh| mesh.subchunk_count).sum(),
        solid_block_count: parts.iter().map(|mesh| mesh.solid_block_count).sum(),
        glass_block_count: parts.iter().map(|mesh| mesh.glass_block_count).sum(),
        water_block_count: parts.iter().map(|mesh| mesh.water_block_count).sum(),
        lava_block_count: parts.iter().map(|mesh| mesh.lava_block_count).sum(),
        face_count: parts.iter().map(|mesh| mesh.face_count).sum(),
        glass_face_count: parts.iter().map(|mesh| mesh.glass_face_count).sum(),
        water_face_count: parts.iter().map(|mesh| mesh.water_face_count).sum(),
        lava_face_count: parts.iter().map(|mesh| mesh.lava_face_count).sum(),
        culled_face_count: parts.iter().map(|mesh| mesh.culled_face_count).sum(),
        omitted_face_count: parts.iter().map(|mesh| mesh.omitted_face_count).sum(),
        truncated_chunk_count: parts.iter().map(|mesh| mesh.truncated_chunk_count).sum(),
        vertex_budget: parts.iter().map(|mesh| mesh.vertex_budget).sum(),
    }
}

fn exact_preview_3d_resource_package_paths(world_path: &Path, cx: &App) -> Vec<PathBuf> {
    let mut package_paths = Vec::new();
    let launcher_path = cx.read_global(|state: &LauncherState, _cx| {
        let package_path = state.package_path.to_string();
        (!package_path.trim().is_empty()).then(|| PathBuf::from(package_path))
    });
    if let Some(package_path) = launcher_path {
        bedrock_block_model::push_unique_resource_pack_path(&mut package_paths, package_path);
    }

    cx.read_global(|state: &LocalVersionsState, _cx| {
        for version in state.versions.iter() {
            bedrock_block_model::push_unique_resource_pack_path(
                &mut package_paths,
                PathBuf::from(version.path.as_ref()),
            );
        }
    });

    for package_path in
        crate::core::minecraft::paths::infer_package_roots_from_world_path(world_path)
    {
        bedrock_block_model::push_unique_resource_pack_path(&mut package_paths, package_path);
    }
    package_paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[::core::prelude::v1::test]
    fn public_render_plan_never_fills_l_shape_hole() {
        let selection =
            ExactChunkSelection::new([chunk(0, 0), chunk(1, 0), chunk(0, 1), chunk(0, 2)])
                .expect("selection");
        let plan = ExactChunkRenderPlan::new(selection);
        let covered = plan
            .rectangle_cover()
            .iter()
            .flat_map(|bounds| {
                (bounds.min_chunk_z..=bounds.max_chunk_z).flat_map(move |z| {
                    (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |x| (x, z))
                })
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(covered.len(), plan.chunk_count());
        assert!(!covered.contains(&(1, 1)));
        assert!(!covered.contains(&(1, 2)));
    }

    #[::core::prelude::v1::test]
    fn public_render_plan_preserves_disconnected_chunks() {
        let selection =
            ExactChunkSelection::new([chunk(0, 0), chunk(4, 0), chunk(4, 1)]).expect("selection");
        let plan = ExactChunkRenderPlan::new(selection);
        assert_eq!(
            plan.rectangle_cover()
                .iter()
                .map(|bounds| bounds.chunk_count())
                .sum::<usize>(),
            3
        );
    }
}

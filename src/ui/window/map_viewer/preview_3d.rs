use super::bedrock_world_domains as bedrock_world;
mod mesh_namespace;
pub(super) use mesh_namespace::namespace_preview_3d_mesh;

pub(super) use super::preview_3d_source::{
    IndexRange, Preview3dBuildStatus, Preview3dCamera, Preview3dDragMode, Preview3dDragState,
    Preview3dMaterialRanges, Preview3dModelRotation, Preview3dMovementInput,
    Preview3dSelectionSignature, Preview3dSource, Preview3dStatus, preview_3d_bounds_depth,
    preview_3d_bounds_width, preview_3d_viewport_parameters,
};

use bedrock_block_model::BlockModelRepository;
use bedrock_render::ChunkPos;
use bedrock_world::{CancelFlag, SlimeChunkBounds};
use gpui_3d::{
    AlphaMode, Camera, Material, Mesh, MeshId, MeshPart, Node, ProjectionRegion, Scene, SceneView,
    ShadingModel, Transform, Vec2, Vec3, Vertex,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::hash::Hash;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const REGION_CHUNKS_XZ: i32 = 8;
const REGION_BLOCKS_XZ: i32 = REGION_CHUNKS_XZ * 16;
const REGION_SUBCHUNKS_Y: i32 = 4;
const REGION_BLOCKS_Y: i32 = REGION_SUBCHUNKS_Y * 16;
const LOD1_ZOOM_THRESHOLD: f32 = 0.48;
const LOD2_ZOOM_THRESHOLD: f32 = 0.20;

#[derive(Clone, Debug)]
pub(super) struct Preview3dMesh {
    pub(super) chunk_meshes: Vec<Preview3dChunkMesh>,
    pub(super) scenes: [Arc<Scene>; 3],
    pub(super) scene_view: Arc<SceneView>,
    pub(super) min_y: i16,
    pub(super) max_y: i16,
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
    pub(super) missing_chunks: usize,
    pub(super) chunk_count: usize,
    pub(super) processed_chunk_count: usize,
    pub(super) subchunk_count: usize,
    pub(super) solid_block_count: usize,
    pub(super) glass_block_count: usize,
    pub(super) water_block_count: usize,
    pub(super) lava_block_count: usize,
    pub(super) face_count: usize,
    pub(super) glass_face_count: usize,
    pub(super) water_face_count: usize,
    pub(super) lava_face_count: usize,
    pub(super) culled_face_count: usize,
    pub(super) omitted_face_count: usize,
    pub(super) truncated_chunk_count: usize,
    pub(super) vertex_budget: usize,
}

impl Preview3dMesh {
    pub(super) fn append_world_preview(
        &self,
        scene: &mut Scene,
        transform: Transform,
    ) -> Result<(), String> {
        let root = scene
            .insert(None, Node::new().with_transform(transform))
            .map_err(|error| error.to_string())?;
        for chunk in &self.chunk_meshes {
            scene
                .insert(
                    Some(root),
                    Node::new()
                        .with_mesh(chunk.mesh.clone())
                        .with_materials([import_preview_material()])
                        .with_transform(Transform {
                            translation: Vec3::new(
                                chunk.world_origin[0] as f32,
                                chunk.world_origin[1] as f32,
                                chunk.world_origin[2] as f32,
                            ),
                            ..Transform::IDENTITY
                        }),
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(super) fn scene_view_for(
        &self,
        camera: Preview3dCamera,
        center: [f32; 3],
        fit_scale: f32,
        model_rotation: Preview3dModelRotation,
    ) -> Result<Arc<SceneView>, String> {
        let scene_index = if camera.zoom <= LOD2_ZOOM_THRESHOLD {
            2
        } else if camera.zoom <= LOD1_ZOOM_THRESHOLD {
            1
        } else {
            0
        };
        let (camera, scene_transform) =
            preview_3d_viewport_parameters(center, fit_scale, camera, model_rotation);
        let scene_view = self
            .scene_view
            .with_scene(self.scenes[scene_index].clone())
            .with_camera(camera)
            .with_scene_transform(scene_transform)
            .map_err(|error| error.to_string())?;
        Ok(Arc::new(scene_view))
    }

    pub(super) fn vertex_count(&self) -> usize {
        self.chunk_meshes
            .iter()
            .map(|mesh| mesh.mesh.vertex_count())
            .sum()
    }

    pub(super) fn chunk_mesh_count(&self) -> usize {
        self.chunk_meshes.len()
    }

    pub(super) fn estimated_cpu_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.chunk_meshes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Preview3dChunkMesh>()),
            )
            .saturating_add(
                self.chunk_meshes
                    .iter()
                    .map(Preview3dChunkMesh::estimated_cpu_bytes)
                    .sum::<usize>(),
            )
    }

    pub(super) fn rendered_chunk_count(&self) -> usize {
        self.processed_chunk_count
            .saturating_sub(self.missing_chunks)
            .min(self.chunk_count)
    }

    pub(super) fn surface_face_count(&self) -> usize {
        self.face_count
            .saturating_add(self.glass_face_count)
            .saturating_add(self.water_face_count)
            .saturating_add(self.lava_face_count)
    }

    pub(super) fn horizontal_span(&self) -> f32 {
        let span_x = self
            .max_x
            .saturating_sub(self.min_x)
            .saturating_add(1)
            .max(1) as f32;
        let span_z = self
            .max_z
            .saturating_sub(self.min_z)
            .saturating_add(1)
            .max(1) as f32;
        span_x.max(span_z)
    }
}

fn import_preview_material() -> Arc<Material> {
    static MATERIAL: OnceLock<Arc<Material>> = OnceLock::new();
    MATERIAL
        .get_or_init(|| {
            let mut material = Material::new();
            material.alpha_mode = AlphaMode::Blend;
            material.base_color[3] = 0.65;
            material.shading_model = ShadingModel::Unlit;
            material.double_sided = true;
            Arc::new(material)
        })
        .clone()
}

#[derive(Clone, Debug)]
pub(super) struct Preview3dChunkMesh {
    pub(super) mesh: Arc<Mesh>,
    lod1_mesh: Option<Arc<Mesh>>,
    lod2_mesh: Option<Arc<Mesh>>,
    pub(super) world_origin: [i32; 3],
    material_table: Arc<[Arc<str>]>,
    pub(super) face_metadata: Arc<[Preview3dFaceMetadata]>,
    region_key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
}

impl Preview3dChunkMesh {
    fn estimated_cpu_bytes(&self) -> usize {
        let mesh_bytes = |mesh: &Mesh| {
            std::mem::size_of::<Mesh>().saturating_add(mesh.geometry_storage_bytes())
        };
        std::mem::size_of::<Self>()
            .saturating_add(mesh_bytes(&self.mesh))
            .saturating_add(self.lod1_mesh.as_deref().map_or(0, mesh_bytes))
            .saturating_add(self.lod2_mesh.as_deref().map_or(0, mesh_bytes))
            .saturating_add(
                self.face_metadata
                    .len()
                    .saturating_mul(std::mem::size_of::<Preview3dFaceMetadata>()),
            )
            .saturating_add(
                self.material_table
                    .iter()
                    .map(|material| material.len())
                    .sum::<usize>(),
            )
    }

    pub(super) fn face_material(&self, face_index: usize) -> Option<&str> {
        let metadata = self.face_metadata.get(face_index)?;
        self.material_table
            .get(usize::from(metadata.material_id))
            .map(AsRef::as_ref)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Preview3dFaceMetadata {
    pub(super) material_id: u16,
    pub(super) uv: Option<[[f32; 2]; 4]>,
}

#[derive(Clone, Debug)]
pub(super) struct Preview3dState {
    pub(super) source: Preview3dSource,
    pub(super) status: Preview3dStatus,
    pub(super) camera: Preview3dCamera,
    pub(super) model_rotation: Preview3dModelRotation,
    pub(super) mesh: Option<Arc<Preview3dMesh>>,
    pub(super) signature: Option<Preview3dSelectionSignature>,
    pub(super) generation: u64,
    pub(super) drag: Option<Preview3dDragState>,
    pub(super) movement_input: Preview3dMovementInput,
    pub(super) last_motion_frame_at: Option<Instant>,
    pub(super) render_in_flight: bool,
    pub(super) cancel: Option<CancelFlag>,
}

impl Default for Preview3dState {
    fn default() -> Self {
        Self {
            source: Preview3dSource::Selection,
            status: Preview3dStatus::Idle,
            camera: Preview3dCamera::default(),
            model_rotation: Preview3dModelRotation::default(),
            mesh: None,
            signature: None,
            generation: 0,
            drag: None,
            movement_input: Preview3dMovementInput::default(),
            last_motion_frame_at: None,
            render_in_flight: false,
            cancel: None,
        }
    }
}

impl Preview3dState {
    pub(super) fn clear_resources(&mut self, clear_pipeline: bool) {
        self.status = Preview3dStatus::Idle;
        self.reset_view_and_model();
        self.signature = None;
        self.mesh = None;
        let _ = clear_pipeline;
        self.render_in_flight = false;
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
    }

    pub(super) fn estimated_surface_bytes(&self) -> usize {
        0
    }

    pub(super) fn clear_surface(&mut self) {}

    pub(super) fn clear_navigation_input(&mut self) {
        self.movement_input = Preview3dMovementInput::default();
        self.last_motion_frame_at = None;
    }

    pub(super) fn reset_view_and_model(&mut self) {
        self.camera = Preview3dCamera::default();
        self.model_rotation = Preview3dModelRotation::default();
        self.drag = None;
        self.clear_navigation_input();
    }

    pub(super) fn tick_motion(&mut self, now: Instant, focused: bool) -> bool {
        if !focused {
            self.clear_navigation_input();
            return false;
        }
        if !self.movement_input.any_active() {
            self.last_motion_frame_at = None;
            return false;
        }
        let delta_seconds = self.last_motion_frame_at.map_or(1.0 / 60.0, |previous| {
            (now - previous).as_secs_f32().clamp(1.0 / 240.0, 0.05)
        });
        self.last_motion_frame_at = Some(now);
        self.camera
            .move_from_input(self.movement_input, delta_seconds);
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Preview3dRegionKey {
    x: i32,
    y: i32,
    z: i32,
}

impl Preview3dRegionKey {
    fn origin(self) -> [i32; 3] {
        [
            self.x.saturating_mul(REGION_BLOCKS_XZ),
            self.y.saturating_mul(REGION_BLOCKS_Y),
            self.z.saturating_mul(REGION_BLOCKS_XZ),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Preview3dRegionPass {
    Opaque,
    Cutout,
    Transparent,
}

#[derive(Clone, Debug)]
struct RegionFace {
    corners: [[f32; 3]; 4],
    color: [f32; 4],
    material: Arc<str>,
    uv: Option<[[f32; 2]; 4]>,
    pass: Preview3dRegionPass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegionFaceFingerprint {
    hash: u64,
    face_count: usize,
    build_lods: bool,
}

#[derive(Clone, Debug)]
struct CachedRegionMesh {
    fingerprint: RegionFaceFingerprint,
    chunk: Preview3dChunkMesh,
}

#[derive(Default)]
struct RegionMeshReuseCache {
    chunks: FxHashMap<(Preview3dRegionKey, Preview3dRegionPass), CachedRegionMesh>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceMeshSignature {
    processed_chunks: usize,
    surface_faces: usize,
    vertices: usize,
    min_y: i16,
    max_y: i16,
}

fn source_mesh_signature(mesh: &super::preview_3d_source::Preview3dMesh) -> SourceMeshSignature {
    SourceMeshSignature {
        processed_chunks: mesh.processed_chunk_count,
        surface_faces: mesh.surface_face_count(),
        vertices: mesh.vertex_count(),
        min_y: mesh.min_y,
        max_y: mesh.max_y,
    }
}

pub(super) fn load_preview_3d_mesh_blocking_incremental(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    cancel: Option<CancelFlag>,
    update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    load_incremental_with_converter(
        move |callback| {
            super::preview_3d_source::load_preview_3d_mesh_blocking_incremental(
                world_path, bounds, cancel, callback,
            )
        },
        update,
    )
}

pub(super) fn load_preview_3d_mesh_blocking_incremental_with_block_models(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    block_models: Option<Arc<BlockModelRepository>>,
    cancel: Option<CancelFlag>,
    update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    load_incremental_with_converter(
        move |callback| {
            super::preview_3d_source::load_preview_3d_mesh_blocking_incremental_with_block_models(
                world_path,
                bounds,
                block_models,
                cancel,
                callback,
            )
        },
        update,
    )
}

fn load_incremental_with_converter(
    loader: impl FnOnce(
        Box<dyn FnMut(Arc<super::preview_3d_source::Preview3dMesh>, Preview3dBuildStatus) + Send>,
    ) -> Result<super::preview_3d_source::Preview3dMesh, String>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    let cache = Arc::new(Mutex::new(RegionMeshReuseCache::default()));
    let last = Arc::new(Mutex::new(None::<(SourceMeshSignature, Preview3dMesh)>));
    let callback_cache = cache.clone();
    let callback_last = last.clone();
    let source_mesh = loader(Box::new(move |source_mesh, status| {
        let signature = source_mesh_signature(&source_mesh);
        let converted = match convert_source_mesh(
            source_mesh.as_ref(),
            &mut callback_cache
                .lock()
                .expect("preview 3D region reuse cache poisoned"),
        ) {
            Ok(converted) => converted,
            Err(error) => {
                tracing::error!(%error, "failed to prepare map 3D scene view");
                return;
            }
        };
        *callback_last
            .lock()
            .expect("preview 3D last mesh cache poisoned") = Some((signature, converted.clone()));
        update(Arc::new(converted), status);
    }))?;
    let signature = source_mesh_signature(&source_mesh);
    if let Some((last_signature, last_mesh)) = last
        .lock()
        .expect("preview 3D last mesh cache poisoned")
        .as_ref()
    {
        if *last_signature == signature {
            tracing::debug!(
                processed_chunks = signature.processed_chunks,
                surface_faces = signature.surface_faces,
                "map_viewer preview_3d_final_incremental_mesh_reused"
            );
            return Ok(last_mesh.clone());
        }
    }
    convert_source_mesh(
        &source_mesh,
        &mut cache
            .lock()
            .expect("preview 3D region reuse cache poisoned"),
    )
}

pub(super) fn load_preview_3d_mesh_from_mcstructure_blocking(
    structure: &bedrock_world::McStructureFile,
    anchor_chunk: ChunkPos,
    origin_y: i32,
) -> Result<Preview3dMesh, String> {
    super::preview_3d_source::load_preview_3d_mesh_from_mcstructure_blocking(
        structure,
        anchor_chunk,
        origin_y,
    )
    .and_then(|mesh| convert_source_mesh(&mesh, &mut RegionMeshReuseCache::default()))
}

pub(super) fn load_preview_3d_mesh_from_copied_chunk_blocking(
    copied_chunk: &super::model::CopiedChunkData,
) -> Result<Preview3dMesh, String> {
    super::preview_3d_source::load_preview_3d_mesh_from_copied_chunk_blocking(copied_chunk)
        .and_then(|mesh| convert_source_mesh(&mesh, &mut RegionMeshReuseCache::default()))
}

fn convert_source_mesh(
    source_mesh: &super::preview_3d_source::Preview3dMesh,
    cache: &mut RegionMeshReuseCache,
) -> Result<Preview3dMesh, String> {
    let started_at = Instant::now();
    let mut groups =
        FxHashMap::<(Preview3dRegionKey, Preview3dRegionPass), Vec<RegionFace>>::default();
    let mut extracted_faces = 0usize;
    let mut rejected_faces = 0usize;
    // The source mesher works in individual chunks. Legitimate blocks and
    // resource-pack models can overhang their column by a few blocks, but a
    // vertex far outside the selected world is a malformed mesh, not a fluid
    // surface or an intentional region-spanning quad.
    let margin = 32.0f32;
    let selection_bounds = [
        [source_mesh.min_x as f32 - margin, source_mesh.max_x as f32 + 1.0 + margin],
        [source_mesh.min_y as f32 - margin, source_mesh.max_y as f32 + 1.0 + margin],
        [source_mesh.min_z as f32 - margin, source_mesh.max_z as f32 + 1.0 + margin],
    ];
    for chunk in &source_mesh.chunk_meshes {
        extract_legacy_faces(
            chunk, selection_bounds, &mut groups,
            &mut extracted_faces, &mut rejected_faces,
        );
    }
    if rejected_faces != 0 {
        tracing::warn!(
            rejected_faces,
            processed_chunks = source_mesh.processed_chunk_count,
            "map_viewer preview_3d_rejected_outlier_faces"
        );
    }

    let mut chunks = Vec::with_capacity(groups.len());
    let mut reused_regions = 0usize;
    let mut lod1_faces = 0usize;
    let mut lod2_faces = 0usize;
    let mut current_keys = FxHashSet::default();
    let build_lods = source_mesh.processed_chunk_count >= source_mesh.chunk_count;
    for ((region_key, pass), faces) in groups {
        let cache_key = (region_key, pass);
        current_keys.insert(cache_key);
        let fingerprint = region_faces_fingerprint(&faces, build_lods);
        let chunk = if let Some(cached) = cache
            .chunks
            .get(&cache_key)
            .filter(|cached| cached.fingerprint == fingerprint)
        {
            reused_regions = reused_regions.saturating_add(1);
            cached.chunk.clone()
        } else {
            build_region_chunk(region_key, pass, &faces, build_lods)
        };
        lod1_faces = lod1_faces.saturating_add(
            chunk
                .lod1_mesh
                .as_ref()
                .map_or(0, |mesh| mesh.index_count() / 6),
        );
        lod2_faces = lod2_faces.saturating_add(
            chunk
                .lod2_mesh
                .as_ref()
                .map_or(0, |mesh| mesh.index_count() / 6),
        );
        cache.chunks.insert(
            cache_key,
            CachedRegionMesh {
                fingerprint,
                chunk: chunk.clone(),
            },
        );
        chunks.push(chunk);
    }
    cache.chunks.retain(|key, _| current_keys.contains(key));
    chunks.sort_by_key(|chunk| (chunk.pass, chunk.region_key));

    let face_count = chunks
        .iter()
        .filter(|chunk| {
            matches!(
                chunk.pass,
                Preview3dRegionPass::Opaque | Preview3dRegionPass::Cutout
            )
        })
        .map(|chunk| chunk.mesh.index_count() / 6)
        .sum();
    let transparent_faces = chunks
        .iter()
        .filter(|chunk| chunk.pass == Preview3dRegionPass::Transparent)
        .map(|chunk| chunk.mesh.index_count() / 6)
        .sum::<usize>();
    tracing::debug!(
        processed_chunks = source_mesh.processed_chunk_count,
        legacy_meshes = source_mesh.chunk_meshes.len(),
        extracted_faces,
        region_meshes = chunks.len(),
        reused_regions,
        lod1_faces,
        lod2_faces,
        region_chunks_xz = REGION_CHUNKS_XZ,
        region_subchunks_y = REGION_SUBCHUNKS_Y,
        elapsed_ms = started_at.elapsed().as_millis(),
        "map_viewer preview_3d_spatial_regions_built"
    );

    let (scenes, scene_view) = preview_3d_scene_data(&chunks)?;

    Ok(Preview3dMesh {
        chunk_meshes: chunks,
        scenes,
        scene_view,
        min_y: source_mesh.min_y,
        max_y: source_mesh.max_y,
        min_x: source_mesh.min_x,
        max_x: source_mesh.max_x,
        min_z: source_mesh.min_z,
        max_z: source_mesh.max_z,
        missing_chunks: source_mesh.missing_chunks,
        chunk_count: source_mesh.chunk_count,
        processed_chunk_count: source_mesh.processed_chunk_count,
        subchunk_count: source_mesh.subchunk_count,
        solid_block_count: source_mesh.solid_block_count,
        glass_block_count: source_mesh.glass_block_count,
        water_block_count: source_mesh.water_block_count,
        lava_block_count: source_mesh.lava_block_count,
        face_count,
        glass_face_count: transparent_faces,
        water_face_count: 0,
        lava_face_count: 0,
        culled_face_count: source_mesh.culled_face_count,
        omitted_face_count: source_mesh.omitted_face_count,
        truncated_chunk_count: source_mesh.truncated_chunk_count,
        vertex_budget: source_mesh.vertex_budget,
    })
}

pub(super) fn preview_3d_scene_data(
    chunks: &[Preview3dChunkMesh],
) -> Result<([Arc<Scene>; 3], Arc<SceneView>), String> {
    let full = build_map_scene(chunks, 0)?;
    // The progressive loader publishes before LOD geometry is available.
    // Do not construct two redundant SceneGraphs containing exactly the
    // same Meshes/Nodes after every batch of newly decoded chunks.
    let scenes = if chunks.iter().all(|chunk| {
        chunk.lod1_mesh.is_none() && chunk.lod2_mesh.is_none()
    }) {
        [full.clone(), full.clone(), full]
    } else {
        [full, build_map_scene(chunks, 1)?, build_map_scene(chunks, 2)?]
    };
    let scene_view = SceneView::new(
        scenes[0].clone(),
        Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            55.0_f32.to_radians(),
            0.1,
            256.0,
        ),
    )
    // Preview meshes consist only of authored colored quads. Expand them
    // directly on the GPU rather than uploading a shared index table.
    .with_voxel_shader()
    .with_projection_region(ProjectionRegion::VisibleContent)
    .with_projection_inset(6.0, 0.08)
    .map_err(|error| error.to_string())?
    .with_blend_edge_feather(1.0)
    .map_err(|error| error.to_string())?;
    Ok((scenes, Arc::new(scene_view)))
}

fn build_map_scene(chunks: &[Preview3dChunkMesh], lod: usize) -> Result<Arc<Scene>, String> {
    let mut scene = Scene::new();
    for chunk in chunks {
        let mesh = match lod {
            1 => chunk.lod1_mesh.as_ref().unwrap_or(&chunk.mesh),
            2 => chunk
                .lod2_mesh
                .as_ref()
                .or(chunk.lod1_mesh.as_ref())
                .unwrap_or(&chunk.mesh),
            _ => &chunk.mesh,
        };
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([region_material(chunk.pass)])
                    .with_transform(Transform {
                        translation: Vec3::new(
                            chunk.world_origin[0] as f32,
                            chunk.world_origin[1] as f32,
                            chunk.world_origin[2] as f32,
                        ),
                        ..Transform::IDENTITY
                    }),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(Arc::new(scene))
}

fn region_material(pass: Preview3dRegionPass) -> Arc<Material> {
    static OPAQUE: OnceLock<Arc<Material>> = OnceLock::new();
    static CUTOUT: OnceLock<Arc<Material>> = OnceLock::new();
    static TRANSPARENT: OnceLock<Arc<Material>> = OnceLock::new();
    let slot = match pass {
        Preview3dRegionPass::Opaque => &OPAQUE,
        Preview3dRegionPass::Cutout => &CUTOUT,
        Preview3dRegionPass::Transparent => &TRANSPARENT,
    };
    slot.get_or_init(|| {
        let mut material = Material::new();
        material.alpha_mode = if pass == Preview3dRegionPass::Transparent {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        };
        material.shading_model = ShadingModel::Unlit;
        material.double_sided = true;
        Arc::new(material)
    })
    .clone()
}

fn region_faces_fingerprint(faces: &[RegionFace], build_lods: bool) -> RegionFaceFingerprint {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut push = |value: u32| {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    push(u32::from(build_lods));
    for face in faces {
        push(face.pass as u32);
        for corner in face.corners {
            for value in corner.map(canonical_f32_bits) {
                push(value);
            }
        }
        for value in face.color.map(canonical_f32_bits) {
            push(value);
        }
        for byte in face.material.as_bytes() {
            push(u32::from(*byte));
        }
        match face.uv {
            Some(uv) => {
                push(1);
                for coordinate in uv {
                    push(canonical_f32_bits(coordinate[0]));
                    push(canonical_f32_bits(coordinate[1]));
                }
            }
            None => push(0),
        }
    }
    RegionFaceFingerprint {
        hash,
        face_count: faces.len(),
        build_lods,
    }
}

fn extract_legacy_faces(
    chunk: &super::preview_3d_source::Preview3dChunkMesh,
    selection_bounds: [[f32; 2]; 3],
    groups: &mut FxHashMap<(Preview3dRegionKey, Preview3dRegionPass), Vec<RegionFace>>,
    extracted_faces: &mut usize,
    rejected_faces: &mut usize,
) {
    let mesh = chunk.mesh.as_ref();
    for (face_index, indices) in mesh.indices().chunks_exact(6).enumerate() {
        let first_index = face_index.saturating_mul(6);
        // The source mesher exports one quad as (0,1,2,0,2,3).
        // Do not reinterpret unrelated triangles into an invented quad:
        // a false fourth corner creates the long stretched triangles seen
        // in malformed water/transparent preview geometry.
        if indices[3] != indices[0] || indices[4] != indices[2] {
            continue;
        }
        let pass = pass_for_legacy_range(
            first_index,
            chunk.ranges,
            chunk.face_metadata.get(face_index),
        );
        let corner_indices = [indices[0], indices[1], indices[2], indices[5]];
        let mut corners = [[0.0; 3]; 4];
        let mut valid = true;
        for (output, index) in corners.iter_mut().zip(corner_indices) {
            let Some(vertex) = mesh.vertices().get(index as usize) else {
                valid = false;
                break;
            };
            *output = [
                vertex.position.x + chunk.world_origin[0] as f32,
                vertex.position.y + chunk.world_origin[1] as f32,
                vertex.position.z + chunk.world_origin[2] as f32,
            ];
        }
        if !valid || !preview_quad_within_selection(&corners, selection_bounds) {
            *rejected_faces = rejected_faces.saturating_add(1);
            continue;
        }
        let Some(vertex) = mesh.vertices().get(indices[0] as usize) else {
            continue;
        };
        let metadata = chunk.face_metadata.get(face_index);
        let face = RegionFace {
            corners,
            color: vertex.color,
            material: metadata
                .map(|metadata| metadata.material.clone())
                .unwrap_or_else(|| Arc::from("minecraft_unknown")),
            uv: metadata.and_then(|metadata| metadata.uv),
            pass,
        };
        for_each_region_face(face, |key, split| {
            groups.entry((key, pass)).or_default().push(split);
            *extracted_faces = extracted_faces.saturating_add(1);
        });
    }
}

/// Discard malformed source quads before they enter the region reuse cache.
///
/// This covers solid/detail as well as transparent and liquid geometry.
/// Bound checks alone would permit a single corrupt quad to stretch across a
/// whole selected world, so retain the existing per-chunk mesher's conservative
/// maximal face extent as a second independent condition.
fn preview_quad_within_selection(
    corners: &[[f32; 3]; 4],
    bounds: [[f32; 2]; 3],
) -> bool {
    let mut low = [f32::INFINITY; 3];
    let mut high = [f32::NEG_INFINITY; 3];
    for corner in corners {
        for axis in 0..3 {
            let value = corner[axis];
            if !value.is_finite() || value < bounds[axis][0] || value > bounds[axis][1] {
                return false;
            }
            low[axis] = low[axis].min(value);
            high[axis] = high[axis].max(value);
        }
    }
    // Region splitting is performed after this validation. A source quad
    // spanning multiple 128-block regions is inconsistent with the chunk
    // mesher, and is the source of screen-wide stretched triangles.
    (0..3).all(|axis| {
        let limit = if axis == 1 { REGION_BLOCKS_Y } else { REGION_BLOCKS_XZ };
        high[axis] - low[axis] <= (limit * 2) as f32
    })
}

fn pass_for_legacy_range(
    first_index: usize,
    ranges: Preview3dMaterialRanges,
    metadata: Option<&super::preview_3d_source::Preview3dFaceMetadata>,
) -> Preview3dRegionPass {
    let first_index = first_index as u32;
    if range_contains(ranges.glass, first_index) || range_contains(ranges.water, first_index) {
        return Preview3dRegionPass::Transparent;
    }
    if metadata.is_some_and(|metadata| material_is_cutout(metadata.material.as_ref())) {
        Preview3dRegionPass::Cutout
    } else {
        Preview3dRegionPass::Opaque
    }
}

fn range_contains(range: IndexRange, index: u32) -> bool {
    range.count > 0 && index >= range.start && index < range.start.saturating_add(range.count)
}

fn material_is_cutout(material: &str) -> bool {
    [
        "leaves", "grass", "flower", "sapling", "fern", "vine", "web", "rail", "torch", "ladder",
        "bars", "pane", "redstone", "mushroom", "coral", "kelp", "seagrass",
    ]
    .iter()
    .any(|needle| material.contains(needle))
}

/// Directly emit region faces without constructing a temporary Vec for each
/// quad. Large selections contain hundreds of thousands of single-region
/// faces; avoiding one heap allocation per face materially reduces CPU churn.
///
/// Reconstruct only proven axis-aligned rectangles. Taking the min/max bounds
/// of an irregular native quad used to replace its actual geometry with an
/// invented rectangle, producing stretched transparent/water triangles.
fn for_each_region_face(
    face: RegionFace,
    mut emit: impl FnMut(Preview3dRegionKey, RegionFace),
) {
    let key = region_key_for_face(&face);
    let origin = key.origin();
    // Most faces are strictly inside their 8x8x4 region. Publish them
    // unchanged without recomputing a normal, synthesizing four corners,
    // cloning the material or allocating a region-split container.
    if face.uv.is_some() || face.corners.iter().all(|corner| {
        [0, 1, 2].into_iter().all(|axis| {
            let extent = if axis == 1 { REGION_BLOCKS_Y } else { REGION_BLOCKS_XZ };
            corner[axis] >= origin[axis] as f32 - 1.0e-4
                && corner[axis] <= (origin[axis] + extent) as f32 + 1.0e-4
        })
    }) {
        emit(key, face);
        return;
    }
    let normal = face_normal(&face);
    let Some(axis) = dominant_axis(normal) else {
        emit(region_key_for_face(&face), face);
        return;
    };
    if normal.iter().enumerate()
        .any(|(index, value)| index != axis && value.abs() > 1.0e-4)
    {
        emit(region_key_for_face(&face), face);
        return;
    }
    let (u_axis, v_axis) = match axis {
        0 => (2, 1),
        1 => (0, 2),
        2 => (0, 1),
        _ => {
            emit(region_key_for_face(&face), face);
            return;
        }
    };
    let plane = face.corners[0][axis];
    if !face.corners.iter().flatten().all(|value| value.is_finite())
        || face.corners.iter()
            .any(|corner| (corner[axis] - plane).abs() > 1.0e-4)
    {
        emit(region_key_for_face(&face), face);
        return;
    }
    let u0 = face.corners.iter()
        .map(|corner| corner[u_axis]).fold(f32::INFINITY, f32::min);
    let u1 = face.corners.iter()
        .map(|corner| corner[u_axis]).fold(f32::NEG_INFINITY, f32::max);
    let v0 = face.corners.iter()
        .map(|corner| corner[v_axis]).fold(f32::INFINITY, f32::min);
    let v1 = face.corners.iter()
        .map(|corner| corner[v_axis]).fold(f32::NEG_INFINITY, f32::max);
    // AABB reconstruction is valid only for four distinct corner combinations.
    // Detail, fluid and custom capture quads may be sloped or irregular;
    // they must be retained without synthetic axes/vertices.
    let mut seen = [false; 4];
    let mut rectangle = u1 > u0 + 1.0e-5 && v1 > v0 + 1.0e-5;
    for corner in &face.corners {
        let u = if (corner[u_axis] - u0).abs() < 1.0e-4 { Some(0) }
            else if (corner[u_axis] - u1).abs() < 1.0e-4 { Some(1) }
            else { None };
        let v = if (corner[v_axis] - v0).abs() < 1.0e-4 { Some(0) }
            else if (corner[v_axis] - v1).abs() < 1.0e-4 { Some(1) }
            else { None };
        match (u, v) {
            (Some(u), Some(v)) if !seen[v * 2 + u] => {
                seen[v * 2 + u] = true;
            }
            _ => rectangle = false,
        }
    }
    if !rectangle || seen.iter().any(|was_seen| !was_seen) {
        emit(region_key_for_face(&face), face);
        return;
    }
    let u_span = if u_axis == 1 { REGION_BLOCKS_Y } else { REGION_BLOCKS_XZ } as f32;
    let v_span = if v_axis == 1 { REGION_BLOCKS_Y } else { REGION_BLOCKS_XZ } as f32;
    let mut u = u0;
    let mut emitted = false;
    while u < u1 - 1.0e-5 {
        let next_u = ((u / u_span).floor() + 1.0) * u_span;
        let end_u = u1.min(next_u.max(u + 1.0e-5));
        let mut v = v0;
        while v < v1 - 1.0e-5 {
            let next_v = ((v / v_span).floor() + 1.0) * v_span;
            let end_v = v1.min(next_v.max(v + 1.0e-5));
            let mut split = face.clone();
            split.corners = axis_aligned_face_corners(
                axis, normal[axis] >= 0.0, plane, u, end_u, v, end_v,
            );
            emit(region_key_for_face(&split), split);
            emitted = true;
            v = end_v;
        }
        u = end_u;
    }
    if !emitted {
        emit(region_key_for_face(&face), face);
    }
}

fn axis_aligned_face_corners(
    axis: usize,
    positive: bool,
    plane: f32,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
) -> [[f32; 3]; 4] {
    match (axis, positive) {
        (0, true) => [
            [plane, v0, u0],
            [plane, v0, u1],
            [plane, v1, u1],
            [plane, v1, u0],
        ],
        (0, false) => [
            [plane, v0, u1],
            [plane, v0, u0],
            [plane, v1, u0],
            [plane, v1, u1],
        ],
        (1, true) => [
            [u0, plane, v0],
            [u1, plane, v0],
            [u1, plane, v1],
            [u0, plane, v1],
        ],
        (1, false) => [
            [u0, plane, v1],
            [u1, plane, v1],
            [u1, plane, v0],
            [u0, plane, v0],
        ],
        (2, true) => [
            [u1, v0, plane],
            [u0, v0, plane],
            [u0, v1, plane],
            [u1, v1, plane],
        ],
        (2, false) => [
            [u0, v0, plane],
            [u1, v0, plane],
            [u1, v1, plane],
            [u0, v1, plane],
        ],
        _ => [[0.0; 3]; 4],
    }
}

fn region_key_for_face(face: &RegionFace) -> Preview3dRegionKey {
    let center =
        [0, 1, 2].map(|axis| face.corners.iter().map(|corner| corner[axis]).sum::<f32>() / 4.0);
    Preview3dRegionKey {
        x: (center[0].floor() as i32).div_euclid(REGION_BLOCKS_XZ),
        y: (center[1].floor() as i32).div_euclid(REGION_BLOCKS_Y),
        z: (center[2].floor() as i32).div_euclid(REGION_BLOCKS_XZ),
    }
}

fn build_region_chunk(
    key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
    faces: &[RegionFace],
    build_lods: bool,
) -> Preview3dChunkMesh {
    let origin = key.origin();
    let (mesh, material_table, face_metadata) = build_region_mesh(key, pass, 0, faces, origin);
    let (lod1_mesh, lod2_mesh) = if build_lods {
        let lod1_faces = faces
            .iter()
            .filter(|face| lod1_keeps_face(face))
            .cloned()
            .collect::<Vec<_>>();
        let lod2_faces = faces
            .iter()
            .filter(|face| lod2_keeps_face(face, key))
            .cloned()
            .collect::<Vec<_>>();
        let lod1_mesh = (!lod1_faces.is_empty() && lod1_faces.len() < faces.len())
            .then(|| build_region_mesh(key, pass, 1, &lod1_faces, origin).0);
        let lod2_mesh = (!lod2_faces.is_empty()
            && lod2_faces.len() < lod1_faces.len().max(faces.len()))
        .then(|| build_region_mesh(key, pass, 2, &lod2_faces, origin).0);
        (lod1_mesh, lod2_mesh)
    } else {
        (None, None)
    };
    Preview3dChunkMesh {
        mesh,
        lod1_mesh,
        lod2_mesh,
        world_origin: origin,
        material_table,
        face_metadata,
        region_key: key,
        pass,
    }
}

fn build_region_mesh(
    key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
    lod: u8,
    faces: &[RegionFace],
    origin: [i32; 3],
) -> (Arc<Mesh>, Arc<[Arc<str>]>, Arc<[Preview3dFaceMetadata]>) {
    // Region faces are independent quads. Global vertex welding previously
    // produced a large, shared, indexed topology; keep every face's four
    // vertices together and let the voxel shader expand its two triangles.
    let mut quads = Vec::<[Vertex; 4]>::with_capacity(faces.len());
    let mut materials = Vec::<Arc<str>>::new();
    let mut material_ids = FxHashMap::<Arc<str>, u16>::default();
    let mut metadata = Vec::with_capacity(faces.len());
    for face in faces {
        let material_id = if let Some(id) = material_ids.get(&face.material).copied() {
            id
        } else {
            let id = u16::try_from(materials.len()).unwrap_or(u16::MAX);
            materials.push(face.material.clone());
            material_ids.insert(face.material.clone(), id);
            id
        };
        let normal = face_normal(face);
        let quad = std::array::from_fn(|slot| {
            let corner = face.corners[slot];
            let uv = face.uv.map_or(Vec2::ZERO, |coordinates| {
                Vec2::new(coordinates[slot][0], coordinates[slot][1])
            });
            Vertex {
                position: Vec3::new(
                    corner[0] - origin[0] as f32,
                    corner[1] - origin[1] as f32,
                    corner[2] - origin[2] as f32,
                ),
                normal: Vec3::new(normal[0], normal[1], normal[2]),
                uv,
                color: face.color.map(|channel| channel.clamp(0.0, 1.0)),
            }
        });
        quads.push(quad);
        metadata.push(Preview3dFaceMetadata { material_id, uv: face.uv });
    }
    let id = region_mesh_id(key, pass, lod);
    let generation = region_mesh_generation(&quads, &materials, &metadata);
    let parts = if quads.is_empty() {
        Vec::new()
    } else {
        vec![MeshPart::new(
            0,
            u32::try_from(quads.len().saturating_mul(6))
                .expect("preview region quad index count must fit u32"),
            0,
        )]
    };
    let mesh = Mesh::from_voxel_quads_without_raycast(quads, parts)
        .expect("preview region contains finite independent quads")
        .with_identity(id, generation);
    (
        Arc::new(mesh),
        Arc::from(materials.into_boxed_slice()),
        Arc::from(metadata.into_boxed_slice()),
    )
}

fn region_mesh_id(key: Preview3dRegionKey, pass: Preview3dRegionPass, lod: u8) -> MeshId {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [
        key.x as u32,
        key.y as u32,
        key.z as u32,
        pass as u32,
        u32::from(lod),
        0x424d_3344,
    ] {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    MeshId(hash)
}

fn region_mesh_generation(
    quads: &[[Vertex; 4]],
    materials: &[Arc<str>],
    metadata: &[Preview3dFaceMetadata],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut push = |value: u32| {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    // Domain-tag the indexed-to-procedural conversion: cached region geometry
    // from an older publication must not be considered equivalent.
    push(0x5155_4144);
    push(quads.len() as u32);
    for quad in quads {
        for vertex in quad {
            for value in
                [vertex.position.x, vertex.position.y, vertex.position.z].map(canonical_f32_bits)
            {
                push(value);
            }
            for value in [vertex.normal.x, vertex.normal.y, vertex.normal.z].map(canonical_f32_bits) {
                push(value);
            }
            for value in [vertex.uv.x, vertex.uv.y].map(canonical_f32_bits) {
                push(value);
            }
            for value in vertex.color.map(canonical_f32_bits) {
                push(value);
            }
        }
    }
    for material in materials {
        for byte in material.as_bytes() {
            push(u32::from(*byte));
        }
    }
    for meta in metadata {
        push(u32::from(meta.material_id));
        push(u32::from(meta.uv.is_some()));
    }
    hash
}

fn lod1_keeps_face(face: &RegionFace) -> bool {
    match face.pass {
        Preview3dRegionPass::Opaque => face_area(face) >= 0.35,
        Preview3dRegionPass::Cutout => face_area(face) >= 0.75,
        Preview3dRegionPass::Transparent => face_area(face) >= 0.75 || face_normal(face)[1] > 0.5,
    }
}

fn lod2_keeps_face(face: &RegionFace, key: Preview3dRegionKey) -> bool {
    let normal = face_normal(face);
    match face.pass {
        Preview3dRegionPass::Cutout => false,
        Preview3dRegionPass::Transparent => normal[1] > 0.5 && face_area(face) >= 1.0,
        Preview3dRegionPass::Opaque => {
            if normal[1] > 0.5 && face_area(face) >= 1.0 {
                return true;
            }
            let origin = key.origin();
            let min = origin.map(|value| value as f32);
            let max = [
                (origin[0] + REGION_BLOCKS_XZ) as f32,
                (origin[1] + REGION_BLOCKS_Y) as f32,
                (origin[2] + REGION_BLOCKS_XZ) as f32,
            ];
            face.corners.iter().all(|corner| {
                (normal[0] < -0.5 && (corner[0] - min[0]).abs() < 1.0e-3)
                    || (normal[0] > 0.5 && (corner[0] - max[0]).abs() < 1.0e-3)
                    || (normal[2] < -0.5 && (corner[2] - min[2]).abs() < 1.0e-3)
                    || (normal[2] > 0.5 && (corner[2] - max[2]).abs() < 1.0e-3)
            })
        }
    }
}

fn face_area(face: &RegionFace) -> f32 {
    let ab = vec3_sub(face.corners[1], face.corners[0]);
    let ac = vec3_sub(face.corners[2], face.corners[0]);
    vec3_length(vec3_cross(ab, ac))
}

fn face_normal(face: &RegionFace) -> [f32; 3] {
    let ab = vec3_sub(face.corners[1], face.corners[0]);
    let ac = vec3_sub(face.corners[2], face.corners[0]);
    let normal = vec3_cross(ab, ac);
    let length = vec3_length(normal).max(1.0e-8);
    [normal[0] / length, normal[1] / length, normal[2] / length]
}

fn dominant_axis(normal: [f32; 3]) -> Option<usize> {
    (0..3).max_by(|left, right| normal[*left].abs().total_cmp(&normal[*right].abs()))
}

fn vec3_sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn vec3_cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn vec3_length(value: [f32; 3]) -> f32 {
    (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt()
}

fn canonical_f32_bits(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

#[cfg(test)]
mod preview_region_geometry_tests {
    use super::*;

    #[test]
    fn preview_region_uses_independent_quads_even_for_distant_faces() {
        let base = face([
            [0.0, 64.0, 0.0], [1.0, 64.0, 0.0],
            [1.0, 64.0, 1.0], [0.0, 64.0, 1.0],
        ]);
        let mut distant = base.clone();
        for point in &mut distant.corners {
            point[0] += 108.0;
        }
        let key = Preview3dRegionKey { x: 0, y: 1, z: 0 };
        let (mesh, _, _) = build_region_mesh(
            key, Preview3dRegionPass::Transparent, 0,
            &[base, distant], key.origin(),
        );
        assert_eq!(mesh.vertex_count(), 8);
        assert_eq!(mesh.index_count(), 12);
        assert_eq!(mesh.geometry_storage_bytes(), 8 * 27);
        assert_eq!(mesh.indices(), &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        assert_eq!(mesh.vertices()[0].position.x, 0.0);
        assert_eq!(mesh.vertices()[4].position.x, 108.0);
    }

    #[test]
    fn preview_region_quad_topology_survives_u16_index_boundary() {
        let water = face([
            [0.0, 64.0, 0.0], [1.0, 64.0, 0.0],
            [1.0, 64.0, 1.0], [0.0, 64.0, 1.0],
        ]);
        let faces = vec![water; 16_400];
        let key = Preview3dRegionKey { x: 0, y: 1, z: 0 };
        let (mesh, _, _) =
            build_region_mesh(key, Preview3dRegionPass::Transparent, 0, &faces, key.origin());
        assert_eq!(mesh.vertex_count(), 65_600);
        assert_eq!(mesh.index_count(), 98_400);
        assert_eq!(mesh.parts()[0].index_count(), 98_400);
        assert_eq!(
            &mesh.indices()[98_394..],
            &[65_596, 65_597, 65_598, 65_596, 65_598, 65_599]
        );
    }

    #[test]
    fn preview_region_rejects_stray_solid_and_fluid_quad_vertices() {
        let bounds = [[-32.0, 160.0], [-64.0, 224.0], [-32.0, 160.0]];
        let valid_water = [[0.0, 64.875, 0.0], [16.0, 64.875, 0.0],
            [16.0, 64.875, 16.0], [0.0, 64.875, 16.0]];
        assert!(preview_quad_within_selection(&valid_water, bounds));
        let mut corrupt = valid_water;
        corrupt[2][0] = 1024.0;
        assert!(!preview_quad_within_selection(&corrupt, bounds));
        let mut non_finite = valid_water;
        non_finite[1][2] = f32::NAN;
        assert!(!preview_quad_within_selection(&non_finite, bounds));
        let wide_bounds = [[-2000.0, 2000.0], [-64.0, 224.0], [-32.0, 160.0]];
        assert!(!preview_quad_within_selection(&corrupt, wide_bounds));
    }


    fn face(corners: [[f32; 3]; 4]) -> RegionFace {
        RegionFace {
            corners,
            color: [0.2, 0.5, 0.9, 0.75],
            material: Arc::from("water"),
            uv: None,
            pass: Preview3dRegionPass::Transparent,
        }
    }

    #[test]
    fn single_region_water_quad_needs_no_split_or_new_geometry() {
        let original = face([
            [0.0, 64.0, 0.0], [2.0, 64.0, 0.0],
            [2.0, 64.0, 2.0], [0.0, 64.0, 2.0],
        ]);
        let mut results = Vec::new();
        for_each_region_face(original.clone(), |key, quad| results.push((key, quad)));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.corners, original.corners);
    }

    #[test]
    fn axis_aligned_quad_crossing_tile_boundary_is_clipped_to_two_tiles() {
        let original = face([
            [127.0, 64.0, 8.0], [129.0, 64.0, 8.0],
            [129.0, 64.0, 9.0], [127.0, 64.0, 9.0],
        ]);
        let mut results = Vec::new();
        for_each_region_face(original, |key, quad| results.push((key, quad)));
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.x, 0);
        assert_eq!(results[1].0.x, 1);
        for (key, quad) in results {
            let origin = key.origin();
            assert!(quad.corners.iter().all(|corner| {
                corner[0] >= origin[0] as f32 - 1.0e-4
                    && corner[0] <= (origin[0] + REGION_BLOCKS_XZ) as f32 + 1.0e-4
            }));
        }
    }

    #[test]
    fn non_rectangular_water_face_keeps_actual_corners() {
        let original = face([
            [127.0, 64.0, 8.0], [129.0, 64.0, 8.0],
            [129.0, 64.0, 9.0], [127.0, 64.0, 8.5],
        ]);
        let mut results = Vec::new();
        for_each_region_face(original.clone(), |key, quad| results.push((key, quad)));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.corners, original.corners,
            "irregular native water geometry must not expand to its AABB");
    }

    #[test]
    fn progressive_preview_reuses_scene_until_lod_meshes_exist() {
        let (scenes, _) = preview_3d_scene_data(&[]).expect("empty scene setup");
        assert!(Arc::ptr_eq(&scenes[0], &scenes[1]));
        assert!(Arc::ptr_eq(&scenes[1], &scenes[2]));
    }
}

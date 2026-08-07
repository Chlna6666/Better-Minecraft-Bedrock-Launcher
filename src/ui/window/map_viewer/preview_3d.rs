pub(super) use super::preview_3d_source::{
    Preview3dBuildStatus, Preview3dCamera, Preview3dDragMode, Preview3dDragState,
    Preview3dModelRotation, Preview3dMovementInput, Preview3dSelectionSignature, Preview3dSource,
    Preview3dStatus, preview_3d_bounds_depth, preview_3d_bounds_width, preview_3d_draw_parameters,
    preview_3d_local_draw_parameters, preview_3d_world_draw_parameters,
};

use bedrock_block_model::BlockModelRepository;
use bedrock_render::ChunkPos;
use bedrock_world::{CancelFlag, SlimeChunkBounds};
use gpui::{
    GpuMesh3d, GpuMesh3dDrawRanges, GpuMesh3dId, GpuMesh3dRange, GpuMesh3dShader, GpuMesh3dVertex,
    WgslShaderSource,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const REGION_CHUNKS_XZ: i32 = 8;
const REGION_BLOCKS_XZ: i32 = REGION_CHUNKS_XZ * 16;
const REGION_SUBCHUNKS_Y: i32 = 4;
const REGION_BLOCKS_Y: i32 = REGION_SUBCHUNKS_Y * 16;
const REGION_SHADER_SOURCE: &str = include_str!("preview_3d_surface.wgsl");
const PREVIEW_3D_VERTICAL_SCALE: f32 = 1.0;
const LOD1_ZOOM_THRESHOLD: f32 = 0.48;
const LOD2_ZOOM_THRESHOLD: f32 = 0.20;

#[derive(Clone, Debug)]
pub(super) struct Preview3dMesh {
    pub(super) chunk_meshes: Vec<Preview3dChunkMesh>,
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
    pub(super) fn vertex_count(&self) -> usize {
        self.chunk_meshes
            .iter()
            .map(|mesh| mesh.gpu_mesh.vertices.len())
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

#[derive(Clone, Debug)]
pub(super) struct Preview3dChunkMesh {
    pub(super) gpu_mesh: Arc<GpuMesh3d>,
    lod1_mesh: Option<Arc<GpuMesh3d>>,
    lod2_mesh: Option<Arc<GpuMesh3d>>,
    pub(super) world_origin: [i32; 3],
    local_bounds: Preview3dMeshBounds,
    material_table: Arc<[Arc<str>]>,
    pub(super) face_metadata: Arc<[Preview3dFaceMetadata]>,
    region_key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
}

impl Preview3dChunkMesh {
    fn estimated_cpu_bytes(&self) -> usize {
        let mesh_bytes = |mesh: &GpuMesh3d| {
            std::mem::size_of::<GpuMesh3d>()
                .saturating_add(
                    mesh.vertices
                        .len()
                        .saturating_mul(std::mem::size_of::<GpuMesh3dVertex>()),
                )
                .saturating_add(
                    mesh.indices
                        .len()
                        .saturating_mul(std::mem::size_of::<u32>()),
                )
        };
        std::mem::size_of::<Self>()
            .saturating_add(mesh_bytes(&self.gpu_mesh))
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

    pub(super) fn selected_gpu_mesh(&self, camera: Preview3dCamera) -> Arc<GpuMesh3d> {
        if camera.zoom <= LOD2_ZOOM_THRESHOLD {
            if let Some(mesh) = &self.lod2_mesh {
                return mesh.clone();
            }
        }
        if camera.zoom <= LOD1_ZOOM_THRESHOLD {
            if let Some(mesh) = &self.lod1_mesh {
                return mesh.clone();
            }
        }
        self.gpu_mesh.clone()
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Preview3dFaceMetadata {
    pub(super) material_id: u16,
    pub(super) uv: Option<[[f32; 2]; 4]>,
}

#[derive(Clone, Copy, Debug)]
struct Preview3dMeshBounds {
    min: [f32; 3],
    max: [f32; 3],
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
        let converted = convert_source_mesh(
            source_mesh.as_ref().clone(),
            &mut callback_cache
                .lock()
                .expect("preview 3D region reuse cache poisoned"),
        );
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
    Ok(convert_source_mesh(
        source_mesh,
        &mut cache
            .lock()
            .expect("preview 3D region reuse cache poisoned"),
    ))
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
    .map(|mesh| convert_source_mesh(mesh, &mut RegionMeshReuseCache::default()))
}

pub(super) fn load_preview_3d_mesh_from_copied_chunk_blocking(
    copied_chunk: &super::model::CopiedChunkData,
) -> Result<Preview3dMesh, String> {
    super::preview_3d_source::load_preview_3d_mesh_from_copied_chunk_blocking(copied_chunk)
        .map(|mesh| convert_source_mesh(mesh, &mut RegionMeshReuseCache::default()))
}

fn convert_source_mesh(
    source_mesh: super::preview_3d_source::Preview3dMesh,
    cache: &mut RegionMeshReuseCache,
) -> Preview3dMesh {
    let started_at = Instant::now();
    let global_center = source_mesh
        .chunk_meshes
        .first()
        .map(|chunk| {
            [
                chunk.gpu_mesh.center[0] + chunk.world_origin[0] as f32,
                chunk.gpu_mesh.center[1] + chunk.world_origin[1] as f32,
                chunk.gpu_mesh.center[2] + chunk.world_origin[2] as f32,
            ]
        })
        .unwrap_or([0.0; 3]);
    let fit_scale = source_mesh
        .chunk_meshes
        .first()
        .map_or(1.0, |chunk| chunk.gpu_mesh.fit_scale);
    let mut groups =
        FxHashMap::<(Preview3dRegionKey, Preview3dRegionPass), Vec<RegionFace>>::default();
    let mut extracted_faces = 0usize;
    for chunk in &source_mesh.chunk_meshes {
        extract_legacy_faces(chunk, &mut groups, &mut extracted_faces);
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
            build_region_chunk(
                region_key,
                pass,
                &faces,
                global_center,
                fit_scale,
                build_lods,
            )
        };
        lod1_faces = lod1_faces.saturating_add(
            chunk
                .lod1_mesh
                .as_ref()
                .map_or(0, |mesh| mesh.indices.len() / 6),
        );
        lod2_faces = lod2_faces.saturating_add(
            chunk
                .lod2_mesh
                .as_ref()
                .map_or(0, |mesh| mesh.indices.len() / 6),
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
        .map(|chunk| chunk.gpu_mesh.indices.len() / 6)
        .sum();
    let transparent_faces = chunks
        .iter()
        .filter(|chunk| chunk.pass == Preview3dRegionPass::Transparent)
        .map(|chunk| chunk.gpu_mesh.indices.len() / 6)
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

    Preview3dMesh {
        chunk_meshes: chunks,
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
    }
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
    groups: &mut FxHashMap<(Preview3dRegionKey, Preview3dRegionPass), Vec<RegionFace>>,
    extracted_faces: &mut usize,
) {
    let mesh = chunk.gpu_mesh.as_ref();
    for (face_index, indices) in mesh.indices.chunks_exact(6).enumerate() {
        let first_index = face_index.saturating_mul(6);
        let pass = pass_for_legacy_range(
            first_index,
            mesh.ranges,
            chunk.face_metadata.get(face_index),
        );
        let corner_indices = [indices[0], indices[1], indices[2], indices[5]];
        let mut corners = [[0.0; 3]; 4];
        let mut valid = true;
        for (output, index) in corners.iter_mut().zip(corner_indices) {
            let Some(vertex) = mesh.vertices.get(index as usize) else {
                valid = false;
                break;
            };
            *output = [
                vertex.position[0] + chunk.world_origin[0] as f32,
                vertex.position[1] + chunk.world_origin[1] as f32,
                vertex.position[2] + chunk.world_origin[2] as f32,
            ];
        }
        if !valid {
            continue;
        }
        let Some(vertex) = mesh.vertices.get(indices[0] as usize) else {
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
        for (key, split) in split_face_into_regions(face) {
            groups.entry((key, pass)).or_default().push(split);
            *extracted_faces = extracted_faces.saturating_add(1);
        }
    }
}

fn pass_for_legacy_range(
    first_index: usize,
    ranges: GpuMesh3dDrawRanges,
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

fn range_contains(range: GpuMesh3dRange, index: u32) -> bool {
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

fn split_face_into_regions(face: RegionFace) -> Vec<(Preview3dRegionKey, RegionFace)> {
    if face.uv.is_some() {
        return vec![(region_key_for_face(&face), face)];
    }
    let normal = face_normal(&face);
    let Some(axis) = dominant_axis(normal) else {
        return vec![(region_key_for_face(&face), face)];
    };
    if normal
        .iter()
        .enumerate()
        .any(|(index, value)| index != axis && value.abs() > 1.0e-4)
    {
        return vec![(region_key_for_face(&face), face)];
    }
    let (u_axis, v_axis) = match axis {
        0 => (2, 1),
        1 => (0, 2),
        2 => (0, 1),
        _ => return vec![(region_key_for_face(&face), face)],
    };
    let plane = face.corners[0][axis];
    if face
        .corners
        .iter()
        .any(|corner| (corner[axis] - plane).abs() > 1.0e-4)
    {
        return vec![(region_key_for_face(&face), face)];
    }
    let u0 = face
        .corners
        .iter()
        .map(|corner| corner[u_axis])
        .fold(f32::INFINITY, f32::min);
    let u1 = face
        .corners
        .iter()
        .map(|corner| corner[u_axis])
        .fold(f32::NEG_INFINITY, f32::max);
    let v0 = face
        .corners
        .iter()
        .map(|corner| corner[v_axis])
        .fold(f32::INFINITY, f32::min);
    let v1 = face
        .corners
        .iter()
        .map(|corner| corner[v_axis])
        .fold(f32::NEG_INFINITY, f32::max);
    let u_span = if u_axis == 1 {
        REGION_BLOCKS_Y
    } else {
        REGION_BLOCKS_XZ
    } as f32;
    let v_span = if v_axis == 1 {
        REGION_BLOCKS_Y
    } else {
        REGION_BLOCKS_XZ
    } as f32;
    let mut output = Vec::new();
    let mut u = u0;
    while u < u1 - 1.0e-5 {
        let next_u = ((u / u_span).floor() + 1.0) * u_span;
        let end_u = u1.min(next_u.max(u + 1.0e-5));
        let mut v = v0;
        while v < v1 - 1.0e-5 {
            let next_v = ((v / v_span).floor() + 1.0) * v_span;
            let end_v = v1.min(next_v.max(v + 1.0e-5));
            let mut split = face.clone();
            split.corners =
                axis_aligned_face_corners(axis, normal[axis] >= 0.0, plane, u, end_u, v, end_v);
            let key = region_key_for_face(&split);
            output.push((key, split));
            v = end_v;
        }
        u = end_u;
    }
    if output.is_empty() {
        output.push((region_key_for_face(&face), face));
    }
    output
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
    global_center: [f32; 3],
    fit_scale: f32,
    build_lods: bool,
) -> Preview3dChunkMesh {
    let origin = key.origin();
    let (gpu_mesh, material_table, face_metadata, bounds) =
        build_region_gpu_mesh(key, pass, 0, faces, origin, global_center, fit_scale);
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
        let lod1_mesh = (!lod1_faces.is_empty() && lod1_faces.len() < faces.len()).then(|| {
            build_region_gpu_mesh(key, pass, 1, &lod1_faces, origin, global_center, fit_scale).0
        });
        let lod2_mesh = (!lod2_faces.is_empty()
            && lod2_faces.len() < lod1_faces.len().max(faces.len()))
        .then(|| {
            build_region_gpu_mesh(key, pass, 2, &lod2_faces, origin, global_center, fit_scale).0
        });
        (lod1_mesh, lod2_mesh)
    } else {
        (None, None)
    };
    Preview3dChunkMesh {
        gpu_mesh,
        lod1_mesh,
        lod2_mesh,
        world_origin: origin,
        local_bounds: bounds,
        material_table,
        face_metadata,
        region_key: key,
        pass,
    }
}

fn build_region_gpu_mesh(
    key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
    lod: u8,
    faces: &[RegionFace],
    origin: [i32; 3],
    global_center: [f32; 3],
    fit_scale: f32,
) -> (
    Arc<GpuMesh3d>,
    Arc<[Arc<str>]>,
    Arc<[Preview3dFaceMetadata]>,
    Preview3dMeshBounds,
) {
    let mut vertices = Vec::with_capacity(faces.len().saturating_mul(4));
    let mut indices = Vec::with_capacity(faces.len().saturating_mul(6));
    let mut vertex_map = FxHashMap::<([u32; 3], [u32; 4]), u32>::default();
    let mut materials = Vec::<Arc<str>>::new();
    let mut material_ids = FxHashMap::<Arc<str>, u16>::default();
    let mut metadata = Vec::with_capacity(faces.len());
    let mut bounds = Preview3dMeshBounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for face in faces {
        let material_id = if let Some(id) = material_ids.get(&face.material).copied() {
            id
        } else {
            let id = u16::try_from(materials.len()).unwrap_or(u16::MAX);
            materials.push(face.material.clone());
            material_ids.insert(face.material.clone(), id);
            id
        };
        let mut face_indices = [0u32; 4];
        for (slot, corner) in face.corners.iter().copied().enumerate() {
            let position = [
                corner[0] - origin[0] as f32,
                corner[1] - origin[1] as f32,
                corner[2] - origin[2] as f32,
            ];
            for axis in 0..3 {
                bounds.min[axis] = bounds.min[axis].min(position[axis]);
                bounds.max[axis] = bounds.max[axis].max(position[axis]);
            }
            let vertex_key = (
                position.map(canonical_f32_bits),
                face.color.map(canonical_f32_bits),
            );
            let index = if let Some(index) = vertex_map.get(&vertex_key).copied() {
                index
            } else {
                let index = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
                vertices.push(GpuMesh3dVertex {
                    position,
                    color: face.color,
                });
                vertex_map.insert(vertex_key, index);
                index
            };
            face_indices[slot] = index;
        }
        indices.extend([
            face_indices[0],
            face_indices[1],
            face_indices[2],
            face_indices[0],
            face_indices[2],
            face_indices[3],
        ]);
        metadata.push(Preview3dFaceMetadata {
            material_id,
            uv: face.uv,
        });
    }
    if !bounds.min[0].is_finite() {
        bounds = Preview3dMeshBounds {
            min: [0.0; 3],
            max: [0.0; 3],
        };
    }
    let range = GpuMesh3dRange {
        start: 0,
        count: u32::try_from(indices.len()).unwrap_or(u32::MAX),
    };
    let ranges = match pass {
        Preview3dRegionPass::Opaque | Preview3dRegionPass::Cutout => GpuMesh3dDrawRanges {
            opaque: range,
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        Preview3dRegionPass::Transparent => GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange::default(),
            glass: range,
            water: GpuMesh3dRange::default(),
        },
    };
    let id = stable_region_mesh_id(key, pass, lod);
    let generation = region_mesh_generation(&vertices, &indices, &materials, &metadata);
    let center = [
        global_center[0] - origin[0] as f32,
        global_center[1] - origin[1] as f32,
        global_center[2] - origin[2] as f32,
    ];
    let mesh = GpuMesh3d {
        id,
        generation,
        vertices: Arc::from(vertices.into_boxed_slice()),
        indices: Arc::from(indices.into_boxed_slice()),
        ranges,
        center,
        fit_scale,
        vertical_scale: PREVIEW_3D_VERTICAL_SCALE,
        shader: region_shader(pass),
    };
    (
        Arc::new(mesh),
        Arc::from(materials.into_boxed_slice()),
        Arc::from(metadata.into_boxed_slice()),
        bounds,
    )
}

fn region_shader(pass: Preview3dRegionPass) -> Arc<GpuMesh3dShader> {
    static OPAQUE: OnceLock<Arc<GpuMesh3dShader>> = OnceLock::new();
    static CUTOUT: OnceLock<Arc<GpuMesh3dShader>> = OnceLock::new();
    static TRANSPARENT: OnceLock<Arc<GpuMesh3dShader>> = OnceLock::new();
    let slot = match pass {
        Preview3dRegionPass::Opaque => &OPAQUE,
        Preview3dRegionPass::Cutout => &CUTOUT,
        Preview3dRegionPass::Transparent => &TRANSPARENT,
    };
    slot.get_or_init(|| {
        let source = WgslShaderSource::from_source(
            "src/ui/window/map_viewer/preview_3d_surface.wgsl",
            REGION_SHADER_SOURCE,
        )
        .expect("preview 3D region shader should validate");
        let fragment = match pass {
            Preview3dRegionPass::Opaque => "fs_preview_3d_opaque",
            Preview3dRegionPass::Cutout => "fs_preview_3d_cutout",
            Preview3dRegionPass::Transparent => "fs_preview_3d_transparent",
        };
        Arc::new(GpuMesh3dShader::new(
            Arc::new(source),
            "vs_preview_3d",
            fragment,
        ))
    })
    .clone()
}

fn stable_region_mesh_id(
    key: Preview3dRegionKey,
    pass: Preview3dRegionPass,
    lod: u8,
) -> GpuMesh3dId {
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
    GpuMesh3dId(hash as usize)
}

fn region_mesh_generation(
    vertices: &[GpuMesh3dVertex],
    indices: &[u32],
    materials: &[Arc<str>],
    metadata: &[Preview3dFaceMetadata],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut push = |value: u32| {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for vertex in vertices {
        for value in vertex.position.map(canonical_f32_bits) {
            push(value);
        }
        for value in vertex.color.map(canonical_f32_bits) {
            push(value);
        }
    }
    for index in indices {
        push(*index);
    }
    for material in materials {
        for byte in material.as_bytes() {
            push(u32::from(*byte));
        }
    }
    for metadata in metadata {
        push(u32::from(metadata.material_id));
        push(u32::from(metadata.uv.is_some()));
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

pub(super) fn preview_3d_chunk_mesh_is_visible(
    mesh: &Preview3dChunkMesh,
    parameters: &gpui::GpuMesh3dDrawParameters,
) -> bool {
    let mut outside_left = true;
    let mut outside_right = true;
    let mut outside_top = true;
    let mut outside_bottom = true;
    let mut outside_near = true;
    let mut outside_far = true;
    let mut behind_camera = true;
    for x in [mesh.local_bounds.min[0], mesh.local_bounds.max[0]] {
        for y in [mesh.local_bounds.min[1], mesh.local_bounds.max[1]] {
            for z in [mesh.local_bounds.min[2], mesh.local_bounds.max[2]] {
                let clip = mat4_mul_vec4(parameters.view_projection_model, [x, y, z, 1.0]);
                outside_left &= clip[0] < -clip[3];
                outside_right &= clip[0] > clip[3];
                outside_bottom &= clip[1] < -clip[3];
                outside_top &= clip[1] > clip[3];
                outside_near &= clip[2] < 0.0;
                outside_far &= clip[2] > clip[3];
                behind_camera &= clip[3] <= 0.0;
            }
        }
    }
    !(outside_left
        || outside_right
        || outside_top
        || outside_bottom
        || outside_near
        || outside_far
        || behind_camera)
}

fn mat4_mul_vec4(matrix: [[f32; 4]; 4], value: [f32; 4]) -> [f32; 4] {
    [
        matrix[0][0] * value[0]
            + matrix[1][0] * value[1]
            + matrix[2][0] * value[2]
            + matrix[3][0] * value[3],
        matrix[0][1] * value[0]
            + matrix[1][1] * value[1]
            + matrix[2][1] * value[2]
            + matrix[3][1] * value[3],
        matrix[0][2] * value[0]
            + matrix[1][2] * value[1]
            + matrix[2][2] * value[2]
            + matrix[3][2] * value[3],
        matrix[0][3] * value[0]
            + matrix[1][3] * value[1]
            + matrix[2][3] * value[2]
            + matrix[3][3] * value[3],
    ]
}

pub(super) use super::preview_3d_legacy::{
    Preview3dBuildStatus, Preview3dCamera, Preview3dChunkMesh, Preview3dDragMode,
    Preview3dDragState, Preview3dFaceMetadata, Preview3dMesh, Preview3dModelRotation,
    Preview3dSelectionSignature, Preview3dSource, Preview3dState, Preview3dStatus,
    preview_3d_bounds_depth, preview_3d_bounds_width, preview_3d_chunk_mesh_is_visible,
    preview_3d_draw_parameters,
};

use bedrock_block_model::BlockModelRepository;
use bedrock_render::ChunkPos;
use bedrock_world::{CancelFlag, SlimeChunkBounds};
use gpui::{GpuMesh3d, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dVertex};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum OptimizedFaceLayer {
    Opaque,
    Glass,
    Water,
}

#[derive(Clone, Debug)]
struct OptimizedFace {
    layer: OptimizedFaceLayer,
    corners: [[f32; 3]; 4],
    color: [f32; 4],
    metadata: Preview3dFaceMetadata,
}

#[derive(Clone, Copy, Debug, Default)]
struct Preview3dMeshOptimizationStats {
    faces_before: usize,
    faces_after: usize,
    vertices_before: usize,
    vertices_after: usize,
    degenerate_faces_removed: usize,
    duplicate_faces_removed: usize,
    coplanar_faces_merged: usize,
}

impl Preview3dMeshOptimizationStats {
    fn merge(mut self, other: Self) -> Self {
        self.faces_before = self.faces_before.saturating_add(other.faces_before);
        self.faces_after = self.faces_after.saturating_add(other.faces_after);
        self.vertices_before = self.vertices_before.saturating_add(other.vertices_before);
        self.vertices_after = self.vertices_after.saturating_add(other.vertices_after);
        self.degenerate_faces_removed = self
            .degenerate_faces_removed
            .saturating_add(other.degenerate_faces_removed);
        self.duplicate_faces_removed = self
            .duplicate_faces_removed
            .saturating_add(other.duplicate_faces_removed);
        self.coplanar_faces_merged = self
            .coplanar_faces_merged
            .saturating_add(other.coplanar_faces_merged);
        self
    }
}

pub(super) fn load_preview_3d_mesh_blocking_incremental(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    cancel: Option<CancelFlag>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    let mesh = super::preview_3d_legacy::load_preview_3d_mesh_blocking_incremental(
        world_path,
        bounds,
        cancel,
        move |mesh, status| update(optimize_preview_3d_mesh_arc(mesh), status),
    )?;
    Ok(optimize_preview_3d_mesh(mesh))
}

pub(super) fn load_preview_3d_mesh_blocking_incremental_with_block_models(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    block_models: Option<Arc<BlockModelRepository>>,
    cancel: Option<CancelFlag>,
    mut update: impl FnMut(Arc<Preview3dMesh>, Preview3dBuildStatus) + Send + 'static,
) -> Result<Preview3dMesh, String> {
    let mesh =
        super::preview_3d_legacy::load_preview_3d_mesh_blocking_incremental_with_block_models(
            world_path,
            bounds,
            block_models,
            cancel,
            move |mesh, status| update(optimize_preview_3d_mesh_arc(mesh), status),
        )?;
    Ok(optimize_preview_3d_mesh(mesh))
}

pub(super) fn load_preview_3d_mesh_from_mcstructure_blocking(
    structure: &bedrock_world::McStructureFile,
    anchor_chunk: ChunkPos,
    origin_y: i32,
) -> Result<Preview3dMesh, String> {
    super::preview_3d_legacy::load_preview_3d_mesh_from_mcstructure_blocking(
        structure,
        anchor_chunk,
        origin_y,
    )
    .map(optimize_preview_3d_mesh)
}

pub(super) fn load_preview_3d_mesh_from_copied_chunk_blocking(
    copied_chunk: &super::model::CopiedChunkData,
) -> Result<Preview3dMesh, String> {
    super::preview_3d_legacy::load_preview_3d_mesh_from_copied_chunk_blocking(copied_chunk)
        .map(optimize_preview_3d_mesh)
}

fn optimize_preview_3d_mesh_arc(mesh: Arc<Preview3dMesh>) -> Arc<Preview3dMesh> {
    let mesh = Arc::try_unwrap(mesh).unwrap_or_else(|mesh| (*mesh).clone());
    Arc::new(optimize_preview_3d_mesh(mesh))
}

fn optimize_preview_3d_mesh(mut mesh: Preview3dMesh) -> Preview3dMesh {
    let started_at = std::time::Instant::now();
    let stats = mesh
        .chunk_meshes
        .par_iter_mut()
        .map(optimize_preview_3d_chunk_mesh)
        .reduce(Preview3dMeshOptimizationStats::default, Preview3dMeshOptimizationStats::merge);

    mesh.face_count = mesh
        .chunk_meshes
        .iter()
        .map(|mesh| mesh.gpu_mesh.ranges.opaque.count as usize / 6)
        .sum();
    mesh.glass_face_count = mesh
        .chunk_meshes
        .iter()
        .map(|mesh| mesh.gpu_mesh.ranges.glass.count as usize / 6)
        .sum();
    let (water_faces, lava_faces) = mesh
        .chunk_meshes
        .iter()
        .map(preview_3d_fluid_face_counts)
        .fold((0usize, 0usize), |left, right| {
            (left.0.saturating_add(right.0), left.1.saturating_add(right.1))
        });
    mesh.water_face_count = water_faces;
    mesh.lava_face_count = lava_faces;

    let face_reduction_percent = reduction_percent(stats.faces_before, stats.faces_after);
    let vertex_reduction_percent = reduction_percent(stats.vertices_before, stats.vertices_after);
    tracing::debug!(
        chunk_meshes = mesh.chunk_meshes.len(),
        faces_before = stats.faces_before,
        faces_after = stats.faces_after,
        vertices_before = stats.vertices_before,
        vertices_after = stats.vertices_after,
        degenerate_faces_removed = stats.degenerate_faces_removed,
        duplicate_faces_removed = stats.duplicate_faces_removed,
        coplanar_faces_merged = stats.coplanar_faces_merged,
        vertices_reused = stats
            .faces_after
            .saturating_mul(4)
            .saturating_sub(stats.vertices_after),
        face_reduction_percent,
        vertex_reduction_percent,
        elapsed_ms = started_at.elapsed().as_millis(),
        strategy = "safe_duplicate_cull+opaque_coplanar_merge+vertex_dedup",
        "map_viewer preview_3d_mesh_optimized"
    );
    mesh
}

fn optimize_preview_3d_chunk_mesh(
    mesh: &mut Preview3dChunkMesh,
) -> Preview3dMeshOptimizationStats {
    let original = mesh.gpu_mesh.as_ref();
    let mut stats = Preview3dMeshOptimizationStats {
        faces_before: original.indices.len() / 6,
        vertices_before: original.vertices.len(),
        ..Preview3dMeshOptimizationStats::default()
    };
    let Some(mut faces) = extract_optimized_faces(mesh) else {
        stats.faces_after = stats.faces_before;
        stats.vertices_after = stats.vertices_before;
        return stats;
    };

    let before_degenerate = faces.len();
    faces.retain(face_is_non_degenerate);
    stats.degenerate_faces_removed = before_degenerate.saturating_sub(faces.len());

    let before_duplicates = faces.len();
    faces = remove_exact_duplicate_faces(faces);
    stats.duplicate_faces_removed = before_duplicates.saturating_sub(faces.len());

    let before_merge = faces.len();
    faces = merge_safe_coplanar_faces(faces);
    stats.coplanar_faces_merged = before_merge.saturating_sub(faces.len());

    faces.sort_by_key(|face| face.layer);
    let (vertices, indices, face_metadata, ranges) = build_deduplicated_mesh_buffers(&faces);
    stats.faces_after = faces.len();
    stats.vertices_after = vertices.len();

    let old = mesh.gpu_mesh.as_ref();
    let optimized = GpuMesh3d::new(
        Arc::from(vertices.into_boxed_slice()),
        Arc::from(indices.into_boxed_slice()),
        ranges,
        old.center,
        old.fit_scale,
        old.vertical_scale,
        old.shader.clone(),
    )
    .with_generation(old.generation);
    mesh.gpu_mesh = Arc::new(optimized);
    mesh.face_metadata = Arc::from(face_metadata.into_boxed_slice());
    stats
}

fn extract_optimized_faces(mesh: &Preview3dChunkMesh) -> Option<Vec<OptimizedFace>> {
    let gpu_mesh = mesh.gpu_mesh.as_ref();
    if gpu_mesh.indices.len() % 6 != 0
        || mesh.face_metadata.len() < gpu_mesh.indices.len() / 6
    {
        return None;
    }
    let mut faces = Vec::with_capacity(gpu_mesh.indices.len() / 6);
    for (face_index, indices) in gpu_mesh.indices.chunks_exact(6).enumerate() {
        let first_index = face_index.saturating_mul(6);
        let layer = layer_for_index(first_index, gpu_mesh.ranges)?;
        let corner_indices = [indices[0], indices[1], indices[2], indices[5]];
        let corners = corner_indices.map(|index| {
            gpu_mesh
                .vertices
                .get(index as usize)
                .map(|vertex| vertex.position)
        });
        let [Some(a), Some(b), Some(c), Some(d)] = corners else {
            return None;
        };
        let color = gpu_mesh.vertices.get(indices[0] as usize)?.color;
        faces.push(OptimizedFace {
            layer,
            corners: [a, b, c, d],
            color,
            metadata: mesh.face_metadata.get(face_index)?.clone(),
        });
    }
    Some(faces)
}

fn layer_for_index(index: usize, ranges: GpuMesh3dDrawRanges) -> Option<OptimizedFaceLayer> {
    let index = u32::try_from(index).ok()?;
    for (layer, range) in [
        (OptimizedFaceLayer::Opaque, ranges.opaque),
        (OptimizedFaceLayer::Glass, ranges.glass),
        (OptimizedFaceLayer::Water, ranges.water),
    ] {
        if index >= range.start && index < range.start.saturating_add(range.count) {
            return Some(layer);
        }
    }
    None
}

fn face_is_non_degenerate(face: &OptimizedFace) -> bool {
    if face
        .corners
        .iter()
        .flatten()
        .chain(face.color.iter())
        .any(|value| !value.is_finite())
    {
        return false;
    }
    let ab = vec3_sub(face.corners[1], face.corners[0]);
    let ac = vec3_sub(face.corners[2], face.corners[0]);
    vec3_length_squared(vec3_cross(ab, ac)) > 1.0e-10
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ExactFaceGeometryKey([[u32; 3]; 4]);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ExactFaceStyleKey {
    layer: OptimizedFaceLayer,
    oriented_corners: [[u32; 3]; 4],
    color: [u32; 4],
    material: Arc<str>,
    uv: Option<[u32; 8]>,
}

fn remove_exact_duplicate_faces(faces: Vec<OptimizedFace>) -> Vec<OptimizedFace> {
    let mut seen = FxHashMap::<ExactFaceGeometryKey, Vec<ExactFaceStyleKey>>::default();
    let mut output = Vec::with_capacity(faces.len());
    for face in faces {
        let geometry = exact_face_geometry_key(&face);
        let style = exact_face_style_key(&face);
        let styles = seen.entry(geometry).or_default();
        if styles.contains(&style) {
            continue;
        }
        styles.push(style);
        output.push(face);
    }
    output
}

fn exact_face_geometry_key(face: &OptimizedFace) -> ExactFaceGeometryKey {
    let mut corners = face.corners.map(|corner| corner.map(canonical_f32_bits));
    corners.sort_unstable();
    ExactFaceGeometryKey(corners)
}

fn exact_face_style_key(face: &OptimizedFace) -> ExactFaceStyleKey {
    ExactFaceStyleKey {
        layer: face.layer,
        oriented_corners: face.corners.map(|corner| corner.map(canonical_f32_bits)),
        color: face.color.map(canonical_f32_bits),
        material: face.metadata.material.clone(),
        uv: face.metadata.uv.map(|uv| {
            let mut bits = [0u32; 8];
            for (index, value) in uv.into_iter().flatten().enumerate() {
                bits[index] = canonical_f32_bits(value);
            }
            bits
        }),
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct CoplanarMergeKey {
    axis: u8,
    normal_positive: bool,
    plane: i64,
    color: [u32; 4],
    material: Arc<str>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct QuantizedRect {
    u0: i64,
    v0: i64,
    u1: i64,
    v1: i64,
}

fn merge_safe_coplanar_faces(faces: Vec<OptimizedFace>) -> Vec<OptimizedFace> {
    let mut passthrough = Vec::with_capacity(faces.len());
    let mut groups = BTreeMap::<CoplanarMergeKey, Vec<QuantizedRect>>::new();
    for face in faces {
        let Some((key, rect)) = coplanar_merge_candidate(&face) else {
            passthrough.push(face);
            continue;
        };
        groups.entry(key).or_default().push(rect);
    }

    for (key, rectangles) in groups {
        for rectangle in merge_quantized_rectangles(rectangles) {
            passthrough.push(face_from_quantized_rectangle(&key, rectangle));
        }
    }
    passthrough
}

fn coplanar_merge_candidate(face: &OptimizedFace) -> Option<(CoplanarMergeKey, QuantizedRect)> {
    if face.layer != OptimizedFaceLayer::Opaque || face.metadata.uv.is_some() {
        return None;
    }
    let normal = face_normal(face)?;
    let axis = dominant_axis(normal)?;
    let dominant = normal[axis];
    if dominant.abs() < 1.0e-6
        || normal
            .iter()
            .enumerate()
            .any(|(index, value)| index != axis && value.abs() > dominant.abs() * 1.0e-4)
    {
        return None;
    }
    let normal_positive = dominant > 0.0;
    let plane = quantize_coordinate(face.corners[0][axis]);
    if face
        .corners
        .iter()
        .any(|corner| quantize_coordinate(corner[axis]) != plane)
    {
        return None;
    }
    let (u_axis, v_axis) = match axis {
        0 => (2, 1),
        1 => (0, 2),
        2 => (0, 1),
        _ => return None,
    };
    let u0 = face
        .corners
        .iter()
        .map(|corner| quantize_coordinate(corner[u_axis]))
        .min()?;
    let u1 = face
        .corners
        .iter()
        .map(|corner| quantize_coordinate(corner[u_axis]))
        .max()?;
    let v0 = face
        .corners
        .iter()
        .map(|corner| quantize_coordinate(corner[v_axis]))
        .min()?;
    let v1 = face
        .corners
        .iter()
        .map(|corner| quantize_coordinate(corner[v_axis]))
        .max()?;
    if u0 >= u1 || v0 >= v1 {
        return None;
    }
    Some((
        CoplanarMergeKey {
            axis: axis as u8,
            normal_positive,
            plane,
            color: face.color.map(canonical_f32_bits),
            material: face.metadata.material.clone(),
        },
        QuantizedRect { u0, v0, u1, v1 },
    ))
}

fn merge_quantized_rectangles(mut rectangles: Vec<QuantizedRect>) -> Vec<QuantizedRect> {
    rectangles.sort_unstable();
    rectangles.dedup();
    for _ in 0..8 {
        let before = rectangles.len();
        rectangles = merge_rectangles_horizontally(rectangles);
        rectangles = merge_rectangles_vertically(rectangles);
        if rectangles.len() == before {
            break;
        }
    }
    rectangles
}

fn merge_rectangles_horizontally(rectangles: Vec<QuantizedRect>) -> Vec<QuantizedRect> {
    let mut rows = BTreeMap::<(i64, i64), Vec<QuantizedRect>>::new();
    for rectangle in rectangles {
        rows.entry((rectangle.v0, rectangle.v1))
            .or_default()
            .push(rectangle);
    }
    let mut output = Vec::new();
    for (_, mut row) in rows {
        row.sort_unstable_by_key(|rectangle| (rectangle.u0, rectangle.u1));
        for rectangle in row {
            if let Some(last) = output.last_mut()
                && last.v0 == rectangle.v0
                && last.v1 == rectangle.v1
                && last.u1 == rectangle.u0
            {
                last.u1 = rectangle.u1;
            } else {
                output.push(rectangle);
            }
        }
    }
    output
}

fn merge_rectangles_vertically(rectangles: Vec<QuantizedRect>) -> Vec<QuantizedRect> {
    let mut columns = BTreeMap::<(i64, i64), Vec<QuantizedRect>>::new();
    for rectangle in rectangles {
        columns
            .entry((rectangle.u0, rectangle.u1))
            .or_default()
            .push(rectangle);
    }
    let mut output = Vec::new();
    for (_, mut column) in columns {
        column.sort_unstable_by_key(|rectangle| (rectangle.v0, rectangle.v1));
        for rectangle in column {
            if let Some(last) = output.last_mut()
                && last.u0 == rectangle.u0
                && last.u1 == rectangle.u1
                && last.v1 == rectangle.v0
            {
                last.v1 = rectangle.v1;
            } else {
                output.push(rectangle);
            }
        }
    }
    output
}

fn face_from_quantized_rectangle(
    key: &CoplanarMergeKey,
    rectangle: QuantizedRect,
) -> OptimizedFace {
    let p = dequantize_coordinate(key.plane);
    let u0 = dequantize_coordinate(rectangle.u0);
    let u1 = dequantize_coordinate(rectangle.u1);
    let v0 = dequantize_coordinate(rectangle.v0);
    let v1 = dequantize_coordinate(rectangle.v1);
    let corners = match (key.axis, key.normal_positive) {
        (0, true) => [[p, v0, u0], [p, v0, u1], [p, v1, u1], [p, v1, u0]],
        (0, false) => [[p, v0, u1], [p, v0, u0], [p, v1, u0], [p, v1, u1]],
        (1, true) => [[u0, p, v0], [u1, p, v0], [u1, p, v1], [u0, p, v1]],
        (1, false) => [[u0, p, v1], [u1, p, v1], [u1, p, v0], [u0, p, v0]],
        (2, true) => [[u1, v0, p], [u0, v0, p], [u0, v1, p], [u1, v1, p]],
        (2, false) => [[u0, v0, p], [u1, v0, p], [u1, v1, p], [u0, v1, p]],
        _ => [[0.0; 3]; 4],
    };
    OptimizedFace {
        layer: OptimizedFaceLayer::Opaque,
        corners,
        color: key.color.map(f32::from_bits),
        metadata: Preview3dFaceMetadata {
            material: key.material.clone(),
            uv: None,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DeduplicatedVertexKey {
    position: [u32; 3],
    color: [u32; 4],
}

fn build_deduplicated_mesh_buffers(
    faces: &[OptimizedFace],
) -> (
    Vec<GpuMesh3dVertex>,
    Vec<u32>,
    Vec<Preview3dFaceMetadata>,
    GpuMesh3dDrawRanges,
) {
    let mut vertices = Vec::with_capacity(faces.len().saturating_mul(2));
    let mut indices = Vec::with_capacity(faces.len().saturating_mul(6));
    let mut metadata = Vec::with_capacity(faces.len());
    let mut vertex_index = FxHashMap::<DeduplicatedVertexKey, u32>::default();
    let mut ranges = GpuMesh3dDrawRanges::default();

    for layer in [
        OptimizedFaceLayer::Opaque,
        OptimizedFaceLayer::Glass,
        OptimizedFaceLayer::Water,
    ] {
        let start = u32::try_from(indices.len()).unwrap_or(u32::MAX);
        for face in faces.iter().filter(|face| face.layer == layer) {
            let mut face_indices = [0u32; 4];
            for (corner_index, position) in face.corners.into_iter().enumerate() {
                let key = DeduplicatedVertexKey {
                    position: position.map(canonical_f32_bits),
                    color: face.color.map(canonical_f32_bits),
                };
                face_indices[corner_index] = *vertex_index.entry(key).or_insert_with(|| {
                    let index = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
                    vertices.push(GpuMesh3dVertex {
                        position,
                        color: face.color,
                    });
                    index
                });
            }
            indices.extend([
                face_indices[0],
                face_indices[1],
                face_indices[2],
                face_indices[0],
                face_indices[2],
                face_indices[3],
            ]);
            metadata.push(face.metadata.clone());
        }
        let range = GpuMesh3dRange {
            start,
            count: u32::try_from(indices.len().saturating_sub(start as usize))
                .unwrap_or(u32::MAX),
        };
        match layer {
            OptimizedFaceLayer::Opaque => ranges.opaque = range,
            OptimizedFaceLayer::Glass => ranges.glass = range,
            OptimizedFaceLayer::Water => ranges.water = range,
        }
    }
    (vertices, indices, metadata, ranges)
}

fn preview_3d_fluid_face_counts(mesh: &Preview3dChunkMesh) -> (usize, usize) {
    let range = mesh.gpu_mesh.ranges.water;
    let start_face = range.start as usize / 6;
    let face_count = range.count as usize / 6;
    let mut water = 0usize;
    let mut lava = 0usize;
    for metadata in mesh
        .face_metadata
        .iter()
        .skip(start_face)
        .take(face_count)
    {
        if metadata.material.to_ascii_lowercase().contains("lava") {
            lava = lava.saturating_add(1);
        } else {
            water = water.saturating_add(1);
        }
    }
    (water, lava)
}

fn face_normal(face: &OptimizedFace) -> Option<[f32; 3]> {
    let ab = vec3_sub(face.corners[1], face.corners[0]);
    let ac = vec3_sub(face.corners[2], face.corners[0]);
    let normal = vec3_cross(ab, ac);
    (vec3_length_squared(normal) > 1.0e-10).then_some(normal)
}

fn dominant_axis(vector: [f32; 3]) -> Option<usize> {
    (0..3).max_by(|left, right| {
        vector[*left]
            .abs()
            .total_cmp(&vector[*right].abs())
    })
}

fn vec3_sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[0] - right[0],
        left[1] - right[1],
        left[2] - right[2],
    ]
}

fn vec3_cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn vec3_length_squared(vector: [f32; 3]) -> f32 {
    vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]
}

fn canonical_f32_bits(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

const POSITION_QUANTIZATION: f32 = 4096.0;

fn quantize_coordinate(value: f32) -> i64 {
    (value * POSITION_QUANTIZATION).round() as i64
}

fn dequantize_coordinate(value: i64) -> f32 {
    value as f32 / POSITION_QUANTIZATION
}

fn reduction_percent(before: usize, after: usize) -> f32 {
    if before == 0 || after >= before {
        return 0.0;
    }
    (before - after) as f32 * 100.0 / before as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Preview3dFaceMetadata {
        Preview3dFaceMetadata {
            material: Arc::from("minecraft_stone"),
            uv: None,
        }
    }

    fn top_face(x0: f32, x1: f32) -> OptimizedFace {
        OptimizedFace {
            layer: OptimizedFaceLayer::Opaque,
            corners: [
                [x0, 1.0, 0.0],
                [x1, 1.0, 0.0],
                [x1, 1.0, 1.0],
                [x0, 1.0, 1.0],
            ],
            color: [0.4, 0.5, 0.6, 1.0],
            metadata: metadata(),
        }
    }

    #[test]
    fn removes_only_exact_same_direction_duplicates() {
        let face = top_face(0.0, 1.0);
        let output = remove_exact_duplicate_faces(vec![face.clone(), face]);
        assert_eq!(output.len(), 1);
    }

    #[test]
    fn merges_adjacent_untextured_opaque_faces() {
        let output = merge_safe_coplanar_faces(vec![top_face(0.0, 1.0), top_face(1.0, 2.0)]);
        assert_eq!(output.len(), 1);
        assert_eq!(
            output[0]
                .corners
                .iter()
                .map(|corner| corner[0])
                .fold(f32::NEG_INFINITY, f32::max),
            2.0
        );
    }

    #[test]
    fn deduplicates_vertices_shared_by_adjacent_faces() {
        let faces = vec![top_face(0.0, 1.0), top_face(1.0, 2.0)];
        let (vertices, indices, metadata, ranges) = build_deduplicated_mesh_buffers(&faces);
        assert_eq!(indices.len(), 12);
        assert_eq!(metadata.len(), 2);
        assert_eq!(ranges.opaque.count, 12);
        assert_eq!(vertices.len(), 6);
    }
}

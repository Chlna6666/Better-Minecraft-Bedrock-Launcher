use super::*;

pub(super) fn test_material() -> MaterialName {
    Arc::from("minecraft_test")
}

pub(super) fn test_mesh_with_faces(faces: Vec<Face>, bounds: SlimeChunkBounds) -> SceneMesh {
    test_mesh_with_layers(faces, Vec::new(), Vec::new(), bounds)
}

pub(super) fn test_mesh_with_layers(
    opaque_faces: Vec<Face>,
    glass_faces: Vec<Face>,
    water_faces: Vec<Face>,
    bounds: SlimeChunkBounds,
) -> SceneMesh {
    let has_faces = !opaque_faces.is_empty() || !glass_faces.is_empty() || !water_faces.is_empty();
    let (min_y, max_y) = if !has_faces {
        (0, 16)
    } else {
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for corner in opaque_faces
            .iter()
            .chain(glass_faces.iter())
            .chain(water_faces.iter())
            .flat_map(|face| face.corners)
        {
            min_y = min_y.min(corner[1]);
            max_y = max_y.max(corner[1]);
        }
        (min_y.floor() as i16, max_y.ceil() as i16)
    };
    let min_x = (bounds.min_chunk_x.saturating_mul(16)) as f32;
    let max_x = (bounds.max_chunk_x.saturating_add(1).saturating_mul(16)) as f32;
    let min_z = (bounds.min_chunk_z.saturating_mul(16)) as f32;
    let max_z = (bounds.max_chunk_z.saturating_add(1).saturating_mul(16)) as f32;
    let min_y_f32 = f32::from(min_y);
    let max_y_f32 = f32::from(max_y).max(min_y_f32 + 1.0);
    let center = [
        (min_x + max_x) * 0.5,
        (min_y_f32 + max_y_f32) * 0.5,
        (min_z + max_z) * 0.5,
    ];
    let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0);
    let vertical_span = (max_y_f32 - min_y_f32).max(1.0);
    let face_count = opaque_faces.len();
    let glass_face_count = glass_faces.len();
    let water_face_count = water_faces.len();
    let chunk_meshes = build_meshes(
        &opaque_faces,
        &glass_faces,
        &water_faces,
        &[],
        center,
        horizontal_span,
        vertical_span,
        7,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    SceneMesh {
        face_count,
        glass_face_count,
        water_face_count,
        lava_face_count: 0,
        chunk_meshes,
        min_y,
        max_y,
        min_x: bounds.min_chunk_x.saturating_mul(16),
        max_x: bounds
            .max_chunk_x
            .saturating_add(1)
            .saturating_mul(16)
            .saturating_sub(1),
        min_z: bounds.min_chunk_z.saturating_mul(16),
        max_z: bounds
            .max_chunk_z
            .saturating_add(1)
            .saturating_mul(16)
            .saturating_sub(1),
        missing_chunks: 0,
        chunk_count: 1,
        processed_chunk_count: 1,
        subchunk_count: 1,
        solid_block_count: usize::from(face_count > 0),
        glass_block_count: usize::from(glass_face_count > 0),
        water_block_count: usize::from(water_face_count > 0),
        lava_block_count: 0,
        culled_face_count: 0,
        omitted_face_count: 0,
        truncated_chunk_count: 0,
        vertex_budget: GPU_VERTEX_BUDGET,
    }
}

pub(super) fn test_block_state(name: &str) -> BlockState {
    BlockState {
        name: name.to_string(),
        states: Default::default(),
        version: None,
    }
}

pub(super) fn test_block_state_with_tag(name: &str, key: &str, value: NbtTag) -> BlockState {
    let mut states = BTreeMap::new();
    states.insert(key.to_string(), value);
    BlockState {
        name: name.to_string(),
        states,
        version: None,
    }
}

pub(super) fn test_block_record(
    key: BlockKey,
    color: [f32; 4],
    material: MaterialName,
) -> BlockRecord {
    BlockRecord::uniform(key, color, material)
}

pub(super) fn test_water_color() -> [f32; 4] {
    [0.18, 0.48, 0.78, WATER_ALPHA]
}

pub(super) fn test_render_chunk_with_subchunks(
    subchunks: BTreeMap<i8, bedrock_world::SubChunk>,
) -> ChunkData {
    ChunkData {
        pos: ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_world::Dimension::Overworld,
        },
        is_loaded: true,
        height_map: None,
        legacy_biomes: None,
        legacy_biome_colors: None,
        biome_data: BTreeMap::new(),
        subchunks,
        block_entities: Vec::new(),
        legacy_terrain: None,
        column_samples: None,
        version: bedrock_world::ChunkVersion::New,
    }
}

pub(super) fn test_mesh(mesh: &SceneMesh) -> &Mesh {
    mesh.chunk_meshes
        .first()
        .map(|chunk_mesh| chunk_mesh.mesh.as_ref())
        .unwrap_or_else(|| panic!("test mesh should contain a mesh"))
}

pub(super) fn test_chunk_mesh(mesh: &SceneMesh) -> &ChunkMesh {
    mesh.chunk_meshes
        .first()
        .unwrap_or_else(|| panic!("test mesh should contain a mesh"))
}

pub(super) fn position_array(position: Vec3) -> [f32; 3] {
    [position.x, position.y, position.z]
}

pub(super) fn test_biome_chunk(chunk_x: i32, biome_id: u32) -> ChunkData {
    let mut chunk = test_render_chunk_with_subchunks(BTreeMap::new());
    chunk.pos.x = chunk_x;
    chunk.biome_data.insert(
        0,
        BiomeStorage {
            y: Some(0),
            palette: vec![biome_id],
            indices: Some(vec![0; 16 * 16 * 16]),
            counts: vec![0],
        },
    );
    chunk
}

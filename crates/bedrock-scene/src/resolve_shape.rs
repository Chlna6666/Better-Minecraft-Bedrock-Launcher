use super::*;

pub(super) fn detail_shape(
    state: &BlockState,
    block_entity_state: Option<&BlockEntityModelState>,
) -> Option<DetailShape> {
    let mut query = block_state_query(state);
    if let Some(block_entity_state) = block_entity_state {
        if let Some(pair_direction) = block_entity_state.chest_pair_direction {
            query = query
                .with_state("pair_direction", pair_direction.as_str())
                .with_state("pair_lead", block_entity_state.chest_pair_lead);
        }
        // A bed's colour is a block-entity field, so it reaches the shape as a synthetic state.
        if let Some(colour) = block_entity_state.bed_colour {
            query = query.with_state("bed_colour", colour);
        }
        // The golem statue pose likewise; `pose()` looks it up under this key.
        if let Some(pose) = block_entity_state.pose {
            query = query.with_state("block_entity_data.Pose", pose);
        }
        // The skull yaw likewise; floor/ceiling mounts rotate the head to it.
        if let Some(yaw) = block_entity_state.skull_yaw {
            query = query.with_state("block_entity_data.skull_yaw", yaw);
        }
    }
    let shape = model_family::resolve_shape(&query)
        .map(detail_shape_from_model)
        .filter(|shape| {
            model_family::has_detail_shape(&query.name) || !shape_is_full_cube(shape)
        })?;
    Some(shape)
}

pub(super) fn resolved_detail_shape(
    repository: &BlockModelRepository,
    state: &BlockState,
    block_class: BlockClass,
) -> Option<DetailShape> {
    if matches!(block_class, BlockClass::Water | BlockClass::Lava) {
        return None;
    }
    // Shulker boxes resolve through the procedural family, which unwraps the 64x64 entity
    // skin per face. `geometry.shulker.v1.8` carries per-cube UV origins that this converter
    // does not unwrap, so using it produced fragmented faces.
    let query = block_state_query(state);
    // Detail-class blocks still need full-cube geometry for oriented UVs and
    // material slots. Dropping it leaves no opaque-cube fallback in collection.
    let keep_full_cube = matches!(
        block_class,
        BlockClass::DetailOpaque | BlockClass::DetailGlass
    );
    // Exact registry state wins for non-contextual detail geometry. Incomplete
    // neighbor-driven states stay on the established wall/fence/stair logic.
    if let Some(shape) = repository
        .captured_shape_shared(&query)
        .map(|shape| detail_shape_from_model_ref(&shape))
        .filter(|shape| !shape.is_empty() && (keep_full_cube || !shape_is_full_cube(shape)))
    {
        return Some(shape);
    }
    if model_family::needs_engine_fallback(&state.name) {
        // Beds resolve from the pack's own `geometry.bed` entity model so the shape and UVs
        // follow the installation instead of a built-in table; the procedural fallback only
        // covers packs that ship no geometry.
        let shaped = if is_bed_state(&state.name) {
            repository
                .geometry(bedrock_block_model::model_family::bed::GEOMETRY_IDENTIFIER)
                .and_then(|geometry| {
                    bedrock_block_model::model_family::bed::shape_from_geometry(geometry, &query)
                })
        } else {
            model_family::resolve_shape(&query)
        };
        if let Some(shape) = shaped
            .map(detail_shape_from_model)
            .filter(|shape| !shape.is_empty())
        {
            return Some(shape);
        }
    }
    let resolved = repository.resolve_block(&query);
    if resolved
        .warnings
        .iter()
        .any(|warning| matches!(warning, ModelWarning::MissingBlockDefinition(_)))
    {
        return None;
    }
    let geometry = resolved.geometry.as_ref()?;
    shape_from_block_geometry(geometry)
        .filter(|shape| !shape.is_empty())
        .filter(|shape| keep_full_cube || !shape_is_full_cube(shape))
}

pub(super) fn block_state_query(state: &BlockState) -> BlockStateQuery {
    BlockStateQuery::from_world_state(state)
}

pub(super) fn shape_from_block_geometry(geometry: &BlockGeometry) -> Option<DetailShape> {
    geometry::model_shape(geometry).map(detail_shape_from_model)
}

pub(super) fn shape_from_named_geometry(
    repository: &BlockModelRepository,
    identifier: &str,
) -> Option<DetailShape> {
    let geometry = repository.geometries.get(identifier)?;
    let mut shape = shape_from_block_geometry(geometry)?;
    if identifier == "geometry.shulker.v1.8" {
        normalize_shulker_geometry_shape(&mut shape);
    }
    Some(shape).filter(|shape| !shape.is_empty())
}

pub(super) fn is_bed_state(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name == "bed" || name.ends_with("_bed")
}

pub(super) fn normalize_shulker_geometry_shape(shape: &mut DetailShape) {
    shape.cuboids.retain(|cuboid| {
        let width = cuboid.max[0] - cuboid.min[0];
        let height = cuboid.max[1] - cuboid.min[1];
        let depth = cuboid.max[2] - cuboid.min[2];
        !(width <= 0.4 && height <= 0.4 && depth <= 0.4)
    });
    for cuboid in &mut shape.cuboids {
        *cuboid = detail_cuboid_with_local_uv(cuboid.clone())
            .with_face_material_slot(BlockFace::Down, "down")
            .with_face_material_slot(BlockFace::Up, "up")
            .with_face_material_slot(BlockFace::Side, "side");
    }
    shape.planes.clear();
}

pub(super) fn material_slot_from_value(value: &str) -> Option<MaterialSlot> {
    let value = value.trim();
    (!value.is_empty()).then(|| Arc::from(value))
}

pub(super) fn shape_is_full_cube(shape: &DetailShape) -> bool {
    shape.planes.is_empty()
        && shape.cuboids.len() == 1
        && shape.cuboids.first().is_some_and(|cuboid| {
            nearly_eq(cuboid.min, [0.0, 0.0, 0.0]) && nearly_eq(cuboid.max, [1.0, 1.0, 1.0])
        })
}

pub(super) fn nearly_eq(left: [f32; 3], right: [f32; 3]) -> bool {
    left.iter()
        .zip(right)
        .all(|(left, right)| (*left - right).abs() < 0.001)
}

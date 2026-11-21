use super::*;

/// flowing material name resolves through the terrain key of the same name.
pub(super) fn fluid_material(block: &FluidBlock, normal: [i32; 3]) -> MaterialName {
    if normal[1] == 0 {
        if block.material.as_ref().contains("lava") {
            Arc::from("minecraft_flowing_lava")
        } else {
            Arc::from("minecraft_flowing_water")
        }
    } else {
        block.material.clone()
    }
}

pub(super) fn fluid_height(state: &BlockState) -> f32 {
    let depth = match state.states.get("liquid_depth") {
        Some(NbtTag::Byte(value)) => i32::from(*value),
        Some(NbtTag::Short(value)) => i32::from(*value),
        Some(NbtTag::Int(value)) => *value,
        Some(NbtTag::Long(value)) => i32::try_from(*value).unwrap_or(0),
        _ => 0,
    }
    .clamp(0, 15);
    // Official render height (minecraft.wiki/w/Water): the surface sits at
    // level/9, so a source block tops out at 8/9 of the cell — one full block
    // only for falling columns (8+).
    match depth {
        0..=7 => (8.0 - depth as f32) / 9.0,
        _ => 1.0,
    }
}

/// Surface height of an ordinary source fluid cell (level 0 -> 8/9). Cells
/// whose state height matches this value stay out of `partial_fluid_heights`;
/// a missing neighbour entry means this height.
pub(super) const SOURCE_FLUID_HEIGHT: f32 = 8.0 / 9.0;

/// Vanilla/Sodium render fluid quads inset by this much into the fluid cell so
/// the co-occupying block's coplanar faces win the depth test instead of
/// blending a water tint over the block geometry.
pub(super) const FLUID_FACE_INSET: f32 = 0.001;

/// Cube face UVs oriented like the detail-cuboid formula so full blocks are not mirrored.
pub(super) fn cube_face_uv(normal: [i32; 3]) -> [[f32; 2]; 4] {
    match normal {
        [0, 1, 0] | [0, -1, 0] => [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        _ => [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
    }
}

/// A flowing liquid's horizontal side may be exposed only above the
/// neighbouring liquid level. Emit that strip without generating invisible
/// geometry below the adjacent surface.
pub(super) fn fluid_face_segment(
    block: BlockKey,
    face: FaceDefinition,
    color: [f32; 4],
    material: MaterialName,
    bottom: f32,
    height: f32,
) -> Face {
    let height = height.clamp(0.05, 1.0);
    let bottom = if face.normal[1] == 0 {
        bottom.clamp(0.0, height)
    } else {
        0.0
    };
    let mut corners = stream_face_corners(face.normal).map(|corner| {
        [
            f64::from(block.x) + f64::from(corner[0]),
            f64::from(block.y) + f64::from(if corner[1] > 0.5 { height } else { bottom }),
            f64::from(block.z) + f64::from(corner[2]),
        ]
    });
    // Vanilla/Sodium inset fluid side quads into their cell so the
    // co-occupying block's coplanar faces win the depth test (no water tint
    // over the block geometry) while the exposed part stays visible. Vertical
    // faces get no inset: the GPU instance path scales Y by the fluid height,
    // so an absolute y offset cannot be baked into the shared template.
    if face.normal[1] == 0 {
        let inset = f64::from(FLUID_FACE_INSET);
        let axis = face
            .normal
            .iter()
            .position(|&value| value != 0)
            .expect("fluid faces are axis aligned");
        for corner in &mut corners {
            corner[axis] -= f64::from(face.normal[axis]) * inset;
        }
    }
    Face {
        corners,
        color,
        shade: face.shade,
        normal: face.normal,
        material,
        uv: Some(fluid_face_uv(face.normal, bottom, height)),
    }
}

/// UVs for a liquid face. A horizontal side is not a full-cell face: its texture is pinned to
/// the liquid surface, so a `height`-tall column samples only the TOP `height` of the flowing
/// tile (`v = 1-height .. 1`), stretching it over the visible band rather than squeezing the
/// whole tile into a short strip. A neighbor that hides the lower part shifts `bottom` up and
/// samples the correspondingly higher slice. Top/bottom faces keep the whole tile.
pub(super) fn fluid_face_uv(normal: [i32; 3], bottom: f32, height: f32) -> [[f32; 2]; 4] {
    if normal[1] != 0 {
        return cube_face_uv(normal);
    }
    let h = height.clamp(0.05, 1.0);
    let b = bottom.clamp(0.0, h);
    let v_bottom = 1.0 - h + b;
    let v_top = 1.0;
    // stream_face_corners side order: [bottom, bottom, top, top] in the y component.
    [[0.0, v_bottom], [1.0, v_bottom], [1.0, v_top], [0.0, v_top]]
}

pub(super) fn stream_fluid_face_segment(
    block: BlockKey,
    face: FaceDefinition,
    color: [f32; 4],
    material: MaterialName,
    pass: StreamPass,
    bottom: f32,
    height: f32,
) -> StreamFace {
    stream_face_from_preview(
        fluid_face_segment(block, face, color, material, bottom, height),
        pass,
    )
}

pub(super) fn stream_fluid_face(
    block: BlockKey,
    face: FaceDefinition,
    color: [f32; 4],
    material: MaterialName,
    pass: StreamPass,
    height: f32,
) -> StreamFace {
    stream_fluid_face_segment(block, face, color, material, pass, 0.0, height)
}

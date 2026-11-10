//! Shared GPU binary encodings and 3D shader ABI for standalone and Sidecar renderers.
//! No GPUI or device-owner dependency is permitted here.

use crate::{AlphaMode, Mat4, Mesh, NodeId, PreparedLight, PreparedScene, ShadingModel, TriangleEdgeMask, Vertex};
use anyhow::{anyhow, Context as _};
use gfx_core::IndexFormat;
use std::collections::HashMap;

pub(crate) const PACKED_VERTEX_STRIDE: u32 = 64;
pub(crate) const VOXEL_VERTEX_STRIDE: u32 = 24;
// WGSL DrawParams has 68 bytes of fields and 16-byte struct alignment, so its storage
// stride is 80 bytes. Keeping this smaller makes DX12/Vulkan expose only a prefix of the
// record: alpha_mode/shading_model/texture_flags then read as zero or adjacent garbage.
pub(crate) const DRAW_PARAMS_STRIDE: u32 = 80;
pub(crate) const DRAW_SLOT_STRIDE: usize = 512;
pub(crate) const FRAME_PARAMS_STRIDE: usize = 272;
pub(crate) const INSTANCE_STRIDE: u32 = 144;
pub(crate) const LIGHT_STRIDE: u32 = 80;
pub(crate) const VERTEX_BINDING: u32 = 0;
pub(crate) const DRAW_BINDING: u32 = 1;
pub(crate) const LIGHT_BINDING: u32 = 2;
pub(crate) const ALBEDO_BINDING: u32 = 3;
pub(crate) const SAMPLER_BINDING: u32 = 4;
pub(crate) const NORMAL_MAP_BINDING: u32 = 5;
pub(crate) const OCCLUSION_BINDING: u32 = 6;
pub(crate) const TANGENT_BINDING: u32 = 7;
pub(crate) const INSTANCE_BINDING: u32 = 8;
pub(crate) const FRAME_BINDING: u32 = 9;
pub(crate) const METALLIC_ROUGHNESS_BINDING: u32 = 10;
pub(crate) const VISIBLE_INSTANCE_BINDING: u32 = 11;


const ALPHA_MODE_OPAQUE: u32 = 0;
const ALPHA_MODE_MASK: u32 = 1;
const ALPHA_MODE_BLEND: u32 = 2;
const SHADING_MODEL_METALLIC_ROUGHNESS: u32 = 0;
const SHADING_MODEL_UNLIT: u32 = 1;
const TEXTURE_FLAG_ALBEDO: u32 = 1;
const TEXTURE_FLAG_OCCLUSION: u32 = 2;
const TEXTURE_FLAG_UV: u32 = 4;
const TEXTURE_FLAG_METALLIC_ROUGHNESS: u32 = 8;
const LIGHT_KIND_AMBIENT: u32 = 0;
const LIGHT_KIND_DIRECTIONAL: u32 = 1;
const LIGHT_KIND_POINT: u32 = 2;
const LIGHT_KIND_SPOT: u32 = 3;

pub(crate) struct EncodedMesh {
    pub(crate) vertices: Vec<u8>,
    pub(crate) tangents: Option<Vec<u8>>,
    pub(crate) indices: Vec<u8>,
    pub(crate) index_format: IndexFormat,
}

pub(crate) fn encode_mesh(mesh: &Mesh) -> anyhow::Result<EncodedMesh> {
    let mut vertices = Vec::new();
    let mut tangents = mesh.tangents().map(|_| Vec::new());
    let mut indices = Vec::new();
    let edge_masks = mesh
        .edge_masks()
        .filter(|masks| masks.iter().any(|mask| mask.bits() != 0));
    let use_u16_indices = if edge_masks.is_some() {
        mesh.indices().len() <= u16::MAX as usize + 1
    } else {
        mesh.vertices().len() <= u16::MAX as usize + 1
    };
    let index_width = if use_u16_indices { 2 } else { 4 };

    if let Some(edge_masks) = edge_masks {
        if edge_masks.len() != mesh.indices().len() / 3 {
            return Err(anyhow!("GPUI 3D mesh edge mask count is invalid").into());
        }
        vertices.reserve(mesh.indices().len() * PACKED_VERTEX_STRIDE as usize);
        indices.reserve(mesh.indices().len() * index_width);
        for (triangle, (triangle_indices, mask)) in mesh
            .indices()
            .chunks_exact(3)
            .zip(edge_masks.iter().copied())
            .enumerate()
        {
            for (corner, &index) in triangle_indices.iter().enumerate() {
                let vertex = mesh
                    .vertices()
                    .get(index as usize)
                    .ok_or_else(|| anyhow!("GPUI 3D mesh index refers to a missing vertex"))?;
                let barycentrics = [
                    if corner == 0 { 1.0 } else { 0.0 },
                    if corner == 1 { 1.0 } else { 0.0 },
                    if corner == 2 { 1.0 } else { 0.0 },
                ];
                write_vertex(&mut vertices, vertex, barycentrics, mask.bits());
                if let Some(encoded_tangents) = &mut tangents {
                    let tangent = mesh
                        .tangents()
                        .and_then(|tangents| tangents.get(index as usize))
                        .ok_or_else(|| anyhow!("GPUI 3D mesh tangent data is incomplete"))?;
                    write_vec4(encoded_tangents, *tangent);
                }
                let expanded_index = u32::try_from(triangle * 3 + corner)
                    .context("GPUI 3D expanded mesh exceeds u32 draw range")?;
                if use_u16_indices {
                    indices.extend_from_slice(&(expanded_index as u16).to_ne_bytes());
                } else {
                    indices.extend_from_slice(&expanded_index.to_ne_bytes());
                }
            }
        }
    } else {
        vertices.reserve(mesh.vertices().len() * PACKED_VERTEX_STRIDE as usize);
        for (index, vertex) in mesh.vertices().iter().enumerate() {
            write_vertex(
                &mut vertices,
                vertex,
                [1.0, 0.0, 0.0],
                TriangleEdgeMask::NONE.bits(),
            );
            if let Some(encoded_tangents) = &mut tangents {
                let tangent = mesh
                    .tangents()
                    .and_then(|tangents| tangents.get(index))
                    .ok_or_else(|| anyhow!("GPUI 3D mesh tangent data is incomplete"))?;
                write_vec4(encoded_tangents, *tangent);
            }
        }
        indices.reserve(mesh.indices().len() * index_width);
        for index in mesh.indices() {
            if use_u16_indices {
                indices.extend_from_slice(&(*index as u16).to_ne_bytes());
            } else {
                indices.extend_from_slice(&index.to_ne_bytes());
            }
        }
    }

    Ok(EncodedMesh {
        vertices,
        tangents,
        indices,
        index_format: if use_u16_indices {
            IndexFormat::Uint16
        } else {
            IndexFormat::Uint32
        },
    })
}

fn write_vertex(bytes: &mut Vec<u8>, vertex: &Vertex, barycentrics: [f32; 3], edge_mask: u8) {
    write_vec4(
        bytes,
        [
            vertex.position.x,
            vertex.position.y,
            vertex.position.z,
            barycentrics[0],
        ],
    );
    write_vec4(
        bytes,
        [
            vertex.normal.x,
            vertex.normal.y,
            vertex.normal.z,
            barycentrics[1],
        ],
    );
    write_vec4(
        bytes,
        [
            vertex.uv.x,
            vertex.uv.y,
            barycentrics[2],
            f32::from(edge_mask),
        ],
    );
    write_vec4(bytes, vertex.color);
}

pub(crate) fn encode_draw_slot(
    draw: &crate::PreparedDraw,
    bytes: &mut Vec<u8>,
) -> anyhow::Result<()> {
    encode_draw_record(draw, bytes, DRAW_SLOT_STRIDE)
}

pub(crate) fn encode_draw_record(
    draw: &crate::PreparedDraw,
    bytes: &mut Vec<u8>,
    stride: usize,
) -> anyhow::Result<()> {
    let slot_start = bytes.len();
    write_vec4(bytes, draw.material.base_color);
    write_vec4(bytes, draw.material.emissive);
    write_f32(bytes, draw.material.metallic);
    write_f32(bytes, draw.material.roughness);
    write_f32(bytes, draw.material.alpha_cutoff);
    let normal_mapping_enabled = draw.material.shading_model == ShadingModel::MetallicRoughness
        && draw.material.normal_texture.is_some();
    write_u32(bytes, u32::from(normal_mapping_enabled));
    write_f32(bytes, draw.material.occlusion_strength);
    write_u32(bytes, alpha_mode_value(draw.material.alpha_mode));
    // Light count is a frame uniform; this word stays zero to keep the record layout.
    write_u32(bytes, 0);
    let shading_model = match draw.material.shading_model {
        ShadingModel::MetallicRoughness => SHADING_MODEL_METALLIC_ROUGHNESS,
        ShadingModel::Unlit => SHADING_MODEL_UNLIT,
    };
    write_u32(bytes, shading_model);
    let mut texture_flags = 0;
    match draw.material.surface {
        crate::SurfaceKind::Standard => {}
        crate::SurfaceKind::Water => texture_flags |= 64,
        crate::SurfaceKind::Glass => texture_flags |= 128,
    }
    if draw.material.albedo_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_ALBEDO;
    }
    if draw.material.occlusion_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_OCCLUSION;
    }
    if draw.mesh.uses_uv_regions() {
        texture_flags |= TEXTURE_FLAG_UV;
    }
    if draw.material.metallic_roughness_texture.is_some() {
        texture_flags |= TEXTURE_FLAG_METALLIC_ROUGHNESS;
    }
    if draw.material.pbr_texture_layout == crate::PbrTextureLayout::MetallicEmissiveRoughness {
        texture_flags |= 16;
    }
    if draw.material.normal_texture.is_some() {
        texture_flags |= 32;
    }
    write_u32(bytes, texture_flags);

    let encoded_size = bytes.len() - slot_start;
    if encoded_size > DRAW_PARAMS_STRIDE as usize {
        return Err(anyhow!(
                "GPUI 3D DrawParams encoding is {encoded_size} bytes but WGSL stride is {DRAW_PARAMS_STRIDE}"
            )
            .into());
    }
    let slot_end = slot_start
        .checked_add(stride)
        .ok_or_else(|| anyhow!("GPUI 3D draw slot size overflow"))?;
    bytes.resize(slot_end, 0);

    Ok(())
}

pub(crate) fn encode_instances_into(
    scene: &PreparedScene,
    bytes: &mut Vec<u8>,
) -> anyhow::Result<()> {
    encode_instance_records(&scene.draws, bytes)
}

pub(crate) fn encode_instance_records(
    draws: &[crate::PreparedDraw],
    bytes: &mut Vec<u8>,
) -> anyhow::Result<()> {
    let start_len = bytes.len();
    // A streamed section commonly emits many material parts from one node.
    // Matrix inversion is independent of the material part, so compute it only
    // once per node/world transform for each table encode.
    let mut inverses = HashMap::<NodeId, (Mat4, Mat4)>::new();
    for draw in draws {
        let inverse = match inverses.get(&draw.node) {
            Some((world, inverse)) if *world == draw.world => *inverse,
            _ => {
                let inverse = draw
                    .world
                    .inverse()
                    .ok_or_else(|| anyhow!("GPUI 3D draw has a singular world transform"))?;
                inverses.insert(draw.node, (draw.world, inverse));
                inverse
            }
        };
        write_matrix(bytes, draw.world.columns());
        write_matrix(bytes, transpose(inverse).columns());
        write_vec4(
            bytes,
            [
                draw.pixel_offset.x,
                draw.pixel_offset.y,
                draw.depth_bias,
                0.0,
            ],
        );
    }
    if draws.is_empty() && bytes.len() == start_len {
        bytes.resize(INSTANCE_STRIDE as usize, 0);
    }
    debug_assert_eq!((bytes.len() - start_len) % INSTANCE_STRIDE as usize, 0);
    Ok(())
}

pub(crate) fn encode_lights_into(lights: &[PreparedLight], bytes: &mut Vec<u8>) {
    let start_len = bytes.len();
    for light in lights {
        match *light {
            PreparedLight::Ambient { color, intensity } => {
                write_light_header(bytes, LIGHT_KIND_AMBIENT);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Directional {
                direction,
                color,
                intensity,
            } => {
                write_light_header(bytes, LIGHT_KIND_DIRECTIONAL);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [direction.x, direction.y, direction.z, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Point {
                position,
                color,
                intensity,
                range,
            } => {
                write_light_header(bytes, LIGHT_KIND_POINT);
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, 0.0, 0.0);
            }
            PreparedLight::Spherical {
                position,
                color,
                intensity,
                range,
                radius,
                diffuse_only,
            } => {
                write_light_header(bytes, if diffuse_only { 4 } else { 5 });
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [0.; 4]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                // Point/spherical records reuse the spot-only cone slot for source radius.
                write_spot_cone_cosines(bytes, radius, 0.);
            }
            PreparedLight::Spot {
                position,
                direction,
                color,
                intensity,
                range,
                inner_angle,
                outer_angle,
            } => {
                write_light_header(bytes, LIGHT_KIND_SPOT);
                write_vec4(bytes, [position.x, position.y, position.z, range]);
                write_vec4(bytes, [direction.x, direction.y, direction.z, 0.0]);
                write_vec4(bytes, [color[0], color[1], color[2], intensity]);
                write_spot_cone_cosines(bytes, outer_angle.cos(), inner_angle.cos());
            }
        }
    }
    if lights.is_empty() && bytes.len() == start_len {
        bytes.resize(LIGHT_STRIDE as usize, 0);
    }
    debug_assert_eq!((bytes.len() - start_len) % LIGHT_STRIDE as usize, 0);
}

fn write_light_header(bytes: &mut Vec<u8>, light_kind: u32) {
    write_u32(bytes, light_kind);
    bytes.resize(bytes.len() + 12, 0);
}

fn write_spot_cone_cosines(bytes: &mut Vec<u8>, outer_cone_cosine: f32, inner_cone_cosine: f32) {
    write_vec2(bytes, [outer_cone_cosine, inner_cone_cosine]);
    bytes.resize(bytes.len() + 8, 0);
}

pub(crate) fn alpha_mode_value(mode: AlphaMode) -> u32 {
    match mode {
        AlphaMode::Opaque => ALPHA_MODE_OPAQUE,
        AlphaMode::Mask => ALPHA_MODE_MASK,
        AlphaMode::Blend => ALPHA_MODE_BLEND,
    }
}

pub(crate) fn transpose(matrix: crate::Mat4) -> crate::Mat4 {
    let columns = matrix.columns();
    crate::Mat4::from_columns(std::array::from_fn(|column| {
        std::array::from_fn(|row| columns[row][column])
    }))
}

pub(crate) fn write_matrix(bytes: &mut Vec<u8>, matrix: [[f32; 4]; 4]) {
    for column in matrix {
        write_vec4(bytes, column);
    }
}

pub(crate) fn write_vec4(bytes: &mut Vec<u8>, vector: [f32; 4]) {
    for value in vector {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
}

pub(crate) fn write_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

pub(crate) fn write_vec2(bytes: &mut Vec<u8>, vector: [f32; 2]) {
    for value in vector {
        write_f32(bytes, value);
    }
}

pub(crate) fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, Material, Node, Scene, Vec3};
    use std::sync::Arc;

    #[test]
    fn standalone_core_encodes_same_shader_record_layout() {
        let mesh = Arc::new(Mesh::cube());
        let mut scene = Scene::new();
        scene.insert(None, Node::new().with_mesh(mesh.clone())
            .with_materials([Arc::new(Material::new())])).unwrap();
        let camera = Camera::perspective(
            Vec3::new(0., 0., 5.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.,
        );
        let prepared = PreparedScene::new(&scene, camera, 1.).unwrap();
        let encoded = encode_mesh(&mesh).unwrap();
        assert_eq!(encoded.vertices.len(), mesh.vertex_count() * PACKED_VERTEX_STRIDE as usize);
        let mut draw = Vec::new();
        encode_draw_slot(&prepared.draws[0], &mut draw).unwrap();
        assert_eq!(draw.len(), DRAW_SLOT_STRIDE);
        let mut instance = Vec::new();
        encode_instances_into(&prepared, &mut instance).unwrap();
        assert_eq!(instance.len(), INSTANCE_STRIDE as usize);
        let mut lights = Vec::new();
        encode_lights_into(&prepared.lights, &mut lights);
        assert_eq!(lights.len(), LIGHT_STRIDE as usize);
    }
}

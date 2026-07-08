use serde_json::Value;

use super::color::Face;
use super::custom_geometry_json::{array2, array4};

#[derive(Clone, Copy)]
pub(super) struct GeometryTextureRegion {
    pub(super) u: f32,
    pub(super) v: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Copy)]
pub(super) struct CubeUvRegions {
    top: Option<GeometryTextureRegion>,
    bottom: Option<GeometryTextureRegion>,
    right: Option<GeometryTextureRegion>,
    front: Option<GeometryTextureRegion>,
    left: Option<GeometryTextureRegion>,
    back: Option<GeometryTextureRegion>,
}

impl CubeUvRegions {
    pub(super) fn region(self, face: Face) -> Option<GeometryTextureRegion> {
        match face {
            Face::Top => self.top,
            Face::Bottom => self.bottom,
            Face::Right => self.right,
            Face::Front => self.front,
            Face::Left => self.left,
            Face::Back => self.back,
        }
    }
}

pub(super) fn cube_uv_regions(uv: &Value, size: [f32; 3]) -> Option<CubeUvRegions> {
    if let Some([u, v]) = array2(Some(uv)) {
        if let Some(regions) = uv_plane_regions(u, v, size) {
            return Some(regions);
        }
        let width = texture_units(size[0]);
        let height = texture_units(size[1]);
        let depth = texture_units(size[2]);
        return Some(uv_box_regions(u, v, width, height, depth));
    }

    let object = uv.as_object()?;
    Some(CubeUvRegions {
        top: face_region_from_object(object, &["up", "top"], Face::Top, size),
        bottom: face_region_from_object(object, &["down", "bottom"], Face::Bottom, size),
        right: face_region_from_object(object, &["west", "right"], Face::Right, size),
        front: face_region_from_object(object, &["south", "front"], Face::Front, size),
        left: face_region_from_object(object, &["east", "left"], Face::Left, size),
        back: face_region_from_object(object, &["north", "back"], Face::Back, size),
    })
}

fn uv_plane_regions(u: f32, v: f32, size: [f32; 3]) -> Option<CubeUvRegions> {
    let zero_axis = zero_dimension_axis(size)?;
    let width = texture_units(size[0]);
    let height = texture_units(size[1]);
    let depth = texture_units(size[2]);

    match zero_axis {
        0 => Some(CubeUvRegions {
            top: None,
            bottom: None,
            right: Some(GeometryTextureRegion {
                u,
                v,
                width: depth,
                height,
            }),
            front: None,
            left: Some(GeometryTextureRegion {
                u,
                v,
                width: depth,
                height,
            }),
            back: None,
        }),
        1 => Some(CubeUvRegions {
            top: Some(GeometryTextureRegion {
                u,
                v,
                width,
                height: depth,
            }),
            bottom: Some(GeometryTextureRegion {
                u,
                v,
                width,
                height: depth,
            }),
            right: None,
            front: None,
            left: None,
            back: None,
        }),
        2 => Some(CubeUvRegions {
            top: None,
            bottom: None,
            right: None,
            front: Some(GeometryTextureRegion {
                u,
                v,
                width,
                height,
            }),
            left: None,
            back: Some(GeometryTextureRegion {
                u,
                v,
                width,
                height,
            }),
        }),
        _ => None,
    }
}

fn uv_box_regions(u: f32, v: f32, width: f32, height: f32, depth: f32) -> CubeUvRegions {
    CubeUvRegions {
        top: Some(GeometryTextureRegion {
            u: u + depth,
            v,
            width,
            height: depth,
        }),
        bottom: Some(GeometryTextureRegion {
            u: u + depth + width,
            v,
            width,
            height: depth,
        }),
        right: Some(GeometryTextureRegion {
            u,
            v: v + depth,
            width: depth,
            height,
        }),
        front: Some(GeometryTextureRegion {
            u: u + depth,
            v: v + depth,
            width,
            height,
        }),
        left: Some(GeometryTextureRegion {
            u: u + depth + width,
            v: v + depth,
            width: depth,
            height,
        }),
        back: Some(GeometryTextureRegion {
            u: u + depth * 2.0 + width,
            v: v + depth,
            width,
            height,
        }),
    }
}

fn face_region_from_object(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    face: Face,
    size: [f32; 3],
) -> Option<GeometryTextureRegion> {
    let value = keys.iter().find_map(|key| object.get(*key))?;
    if let Some([u, v, width, height]) = array4(Some(value)) {
        return Some(GeometryTextureRegion {
            u,
            v,
            width,
            height,
        });
    }

    let uv = value.get("uv").and_then(|value| array2(Some(value)))?;
    let uv_size = value
        .get("uv_size")
        .and_then(|value| array2(Some(value)))
        .unwrap_or_else(|| default_face_uv_size(face, size));
    Some(GeometryTextureRegion {
        u: uv[0],
        v: uv[1],
        width: uv_size[0],
        height: uv_size[1],
    })
}

fn default_face_uv_size(face: Face, size: [f32; 3]) -> [f32; 2] {
    match face {
        Face::Front | Face::Back => [texture_units(size[0]), texture_units(size[1])],
        Face::Left | Face::Right => [texture_units(size[2]), texture_units(size[1])],
        Face::Top | Face::Bottom => [texture_units(size[0]), texture_units(size[2])],
    }
}

fn texture_units(value: f32) -> f32 {
    value.abs().ceil().max(1.0)
}

fn zero_dimension_axis(size: [f32; 3]) -> Option<usize> {
    let mut axis = None;
    for (index, value) in size.iter().enumerate() {
        if value.abs() <= f32::EPSILON {
            if axis.is_some() {
                return None;
            }
            axis = Some(index);
        }
    }
    axis
}

pub(super) fn texture_grid_count(value: f32, preview_scale: u32) -> u32 {
    let base = value.abs().ceil().max(1.0) as u32;
    base.saturating_mul(preview_scale.max(1)).max(1)
}

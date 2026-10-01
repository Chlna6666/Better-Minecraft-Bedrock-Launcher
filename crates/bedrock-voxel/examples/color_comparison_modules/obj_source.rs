use std::{collections::HashMap, path::PathBuf, sync::Arc};

use super::image::{linear_srgb, srgb_linear};
use ::image::{ImageReader, Rgba, RgbaImage};
use bedrock_voxel::ObjModel;

pub(super) struct Projection {
    pub(super) pixels: RgbaImage,
    pub(super) mask: Vec<bool>,
    pub(super) alpha: Vec<f32>,
    pub(super) top_face: Vec<bool>,
}

pub(super) fn obj_xy_source_projection(
    model: &ObjModel,
    longest_side: u16,
    view_positive_z: bool,
) -> Result<Projection, String> {
    let (minimum, scale, dimensions) = obj_projection_dimensions(model, longest_side)?;
    let width = dimensions[0] as u32;
    let height = dimensions[1] as u32;
    let mut projection = blank_projection(width, height);
    let initial_depth = if view_positive_z {
        f32::NEG_INFINITY
    } else {
        f32::INFINITY
    };
    let mut depth = vec![initial_depth; projection.mask.len()];
    let textures = load_model_textures(model)?;

    for triangle in &model.triangles {
        let (diffuse, opacity, texture_id) = obj_material(model, triangle)?;
        let texture = texture_id.and_then(|id| textures[id].as_deref());
        if texture.is_some() && triangle.vertices.iter().any(|vertex| vertex.uv.is_none()) {
            return Err("textured OBJ face is missing UV coordinates".to_owned());
        }
        let vertices = triangle.vertices.map(|vertex| {
            [
                (vertex.position[0] - minimum[0]) * scale,
                height as f32 - (vertex.position[1] - minimum[1]) * scale,
                (vertex.position[2] - minimum[2]) * scale,
            ]
        });
        let edge_a = [0, 1, 2].map(|axis| vertices[1][axis] - vertices[0][axis]);
        let edge_b = [0, 1, 2].map(|axis| vertices[2][axis] - vertices[0][axis]);
        let normal = [
            edge_a[1] * edge_b[2] - edge_a[2] * edge_b[1],
            edge_a[2] * edge_b[0] - edge_a[0] * edge_b[2],
            edge_a[0] * edge_b[1] - edge_a[1] * edge_b[0],
        ];
        let top_face = normal[1].abs() >= normal[0].abs().max(normal[2].abs());
        let projected_area = (vertices[1][1] - vertices[2][1]) * (vertices[0][0] - vertices[2][0])
            + (vertices[2][0] - vertices[1][0]) * (vertices[0][1] - vertices[2][1]);
        if projected_area.abs() < 1e-8 {
            continue;
        }
        let left = vertices
            .iter()
            .map(|vertex| vertex[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let right = vertices
            .iter()
            .map(|vertex| vertex[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(width as f32) as u32;
        let top = vertices
            .iter()
            .map(|vertex| vertex[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as u32;
        let bottom = vertices
            .iter()
            .map(|vertex| vertex[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(height as f32) as u32;
        for y in top..bottom {
            for x in left..right {
                let point = [x as f32 + 0.5, y as f32 + 0.5];
                let Some(weights) = projected_barycentric(vertices, point) else {
                    continue;
                };
                if weights.iter().any(|weight| *weight < -1e-5) {
                    continue;
                }
                let z = (0..3)
                    .map(|index| vertices[index][2] * weights[index])
                    .sum::<f32>();
                let pixel_index = (y * width + x) as usize;
                if (view_positive_z && z <= depth[pixel_index])
                    || (!view_positive_z && z >= depth[pixel_index])
                {
                    continue;
                }
                let sampled_texture = if let Some(texture) = texture {
                    let uv = [0, 1].map(|axis| {
                        (0..3)
                            .map(|index| {
                                triangle.vertices[index].uv.unwrap_or([0.0; 2])[axis]
                                    * weights[index]
                            })
                            .sum::<f32>()
                    });
                    Some(sample_obj_texture(texture, uv))
                } else {
                    None
                };
                let Some((color, alpha)) = source_material_color(sampled_texture, diffuse, opacity)
                else {
                    continue;
                };
                depth[pixel_index] = z;
                projection.pixels.put_pixel(x, y, Rgba(color));
                projection.mask[pixel_index] = true;
                projection.alpha[pixel_index] = alpha;
                projection.top_face[pixel_index] = top_face;
            }
        }
    }
    Ok(projection)
}

pub(super) fn blank_projection(width: u32, height: u32) -> Projection {
    Projection {
        pixels: RgbaImage::from_pixel(width, height, Rgba([255; 4])),
        mask: vec![false; (width * height) as usize],
        alpha: vec![0.0; (width * height) as usize],
        top_face: vec![false; (width * height) as usize],
    }
}

fn obj_projection_dimensions(
    model: &bedrock_voxel::ObjModel,
    longest_side: u16,
) -> Result<([f32; 3], f32, [usize; 3]), String> {
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for triangle in &model.triangles {
        for vertex in triangle.vertices {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
    }
    let extent = [0, 1, 2].map(|axis| maximum[axis] - minimum[axis]);
    let longest = extent.into_iter().fold(0.0_f32, f32::max);
    if !longest.is_finite() || longest <= 0.0 {
        return Err("OBJ has no usable spatial extent".to_owned());
    }
    let scale = f32::from(longest_side) / longest;
    let dimensions = extent.map(|value| (value * scale).ceil().max(1.0) as usize);
    Ok((minimum, scale, dimensions))
}

fn load_model_textures(
    model: &bedrock_voxel::ObjModel,
) -> Result<Vec<Option<Arc<RgbaImage>>>, String> {
    let mut cache = HashMap::<PathBuf, Arc<RgbaImage>>::new();
    model
        .materials
        .iter()
        .map(|material| {
            let Some(path) = &material.diffuse_texture else {
                return Ok(None);
            };
            if let Some(image) = cache.get(path) {
                return Ok(Some(Arc::clone(image)));
            }
            let image = Arc::new(
                ImageReader::open(path)
                    .map_err(|error| error.to_string())?
                    .decode()
                    .map_err(|error| error.to_string())?
                    .to_rgba8(),
            );
            cache.insert(path.clone(), Arc::clone(&image));
            Ok(Some(image))
        })
        .collect()
}

fn obj_material(
    model: &bedrock_voxel::ObjModel,
    triangle: &bedrock_voxel::ObjTriangle,
) -> Result<([u8; 3], f32, Option<usize>), String> {
    let Some(material_id) = triangle.material else {
        return Ok(([192; 3], 1.0, None));
    };
    let material = model
        .materials
        .get(material_id)
        .ok_or_else(|| "OBJ triangle references a missing material".to_owned())?;
    let diffuse = material
        .diffuse
        .unwrap_or(if material.diffuse_texture.is_some() {
            [1.0; 3]
        } else {
            [0.75; 3]
        });
    let diffuse = diffuse.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    Ok((
        diffuse,
        material.opacity,
        material.diffuse_texture.as_ref().map(|_| material_id),
    ))
}

fn projected_barycentric(vertices: [[f32; 3]; 3], point: [f32; 2]) -> Option<[f32; 3]> {
    let [a, b, c] = vertices;
    let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denominator.abs() < 1e-8 {
        return None;
    }
    let first =
        ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1])) / denominator;
    let second =
        ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1])) / denominator;
    Some([first, second, 1.0 - first - second])
}

fn sample_obj_texture(texture: &RgbaImage, uv: [f32; 2]) -> [u8; 4] {
    let width = texture.width();
    let height = texture.height();
    let u = uv[0].rem_euclid(1.0) * width as f32 - 0.5;
    let v = (1.0 - uv[1]).rem_euclid(1.0) * height as f32 - 0.5;
    let x0 = u.floor();
    let y0 = v.floor();
    let tx = u - x0;
    let ty = v - y0;
    let wrap = |value: f32, size: u32| value.rem_euclid(size as f32) as u32;
    let x0 = wrap(x0, width);
    let x1 = (x0 + 1) % width;
    let y0 = wrap(y0, height);
    let y1 = (y0 + 1) % height;
    let corners = [
        texture.get_pixel(x0, y0).0,
        texture.get_pixel(x1, y0).0,
        texture.get_pixel(x0, y1).0,
        texture.get_pixel(x1, y1).0,
    ];
    let channel = |index: usize| {
        let top = f32::from(corners[0][index]) * (1.0 - tx) + f32::from(corners[1][index]) * tx;
        let bottom = f32::from(corners[2][index]) * (1.0 - tx) + f32::from(corners[3][index]) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let alpha = channel(3) / 255.0;
    if alpha <= f32::EPSILON {
        return [0; 4];
    }
    let rgb = [0, 1, 2].map(|index| {
        let premultiplied =
            corners.map(|pixel| srgb_linear(pixel[index]) * f32::from(pixel[3]) / 255.0);
        let top = premultiplied[0] * (1.0 - tx) + premultiplied[1] * tx;
        let bottom = premultiplied[2] * (1.0 - tx) + premultiplied[3] * tx;
        linear_srgb((top * (1.0 - ty) + bottom * ty) / alpha)
    });
    [rgb[0], rgb[1], rgb[2], (alpha * 255.0).round() as u8]
}

fn source_material_color(
    texture: Option<[u8; 4]>,
    diffuse: [u8; 3],
    opacity: f32,
) -> Option<([u8; 4], f32)> {
    let texture = texture.unwrap_or([255; 4]);
    let alpha = f32::from(texture[3]) / 255.0 * opacity;
    if alpha * 255.0 < 1.0 {
        return None;
    }
    let rgb = [0, 1, 2].map(|channel| {
        linear_srgb(
            srgb_linear(texture[channel]) * srgb_linear(diffuse[channel]) * alpha + (1.0 - alpha),
        )
    });
    Some(([rgb[0], rgb[1], rgb[2], 255], alpha))
}

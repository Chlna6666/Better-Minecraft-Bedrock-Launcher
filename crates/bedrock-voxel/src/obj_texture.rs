//! Bounded source texture decoding and UV sampling for OBJ conversion.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use image::{ImageReader, Limits, RgbaImage};

use crate::{
    Result,
    block_image::{linear_srgb, srgb_linear},
    obj::ObjModel,
    validation,
};

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;

pub(super) fn load_textures(model: &ObjModel) -> Result<Vec<Option<Arc<RgbaImage>>>> {
    let mut total_bytes = 0_u64;
    let mut decoded = HashMap::<PathBuf, Arc<RgbaImage>>::new();
    model
        .materials
        .iter()
        .map(|material| {
            let Some(path) = &material.diffuse_texture else {
                return Ok(None);
            };
            if let Some(texture) = decoded.get(path) {
                return Ok(Some(texture.clone()));
            }
            let file_size = path
                .metadata()
                .map_err(|error| validation(error.to_string()))?
                .len();
            if file_size > MAX_FILE_BYTES {
                return Err(validation("OBJ texture exceeds 64 MiB input limit"));
            }
            let mut reader =
                ImageReader::open(path).map_err(|error| validation(error.to_string()))?;
            let mut limits = Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(MAX_DECODED_BYTES);
            reader.limits(limits);
            let image = reader
                .decode()
                .map_err(|error| validation(error.to_string()))?;
            let rgba_bytes = u64::from(image.width())
                .checked_mul(u64::from(image.height()))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or_else(|| validation("OBJ texture dimensions overflow"))?;
            total_bytes = total_bytes
                .checked_add(rgba_bytes)
                .ok_or_else(|| validation("OBJ texture budget overflows"))?;
            if total_bytes > MAX_DECODED_BYTES {
                return Err(validation("OBJ textures exceed 256 MiB decoded budget"));
            }
            let texture = Arc::new(image.to_rgba8());
            decoded.insert(path.clone(), texture.clone());
            Ok(Some(texture))
        })
        .collect()
}

/// Bilinear sample with repeating U/V and OBJ's bottom-up V coordinates.
pub(super) fn sample(texture: &RgbaImage, uv: [f32; 2]) -> [u8; 4] {
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
    let sample_channel = |channel: usize| {
        let top = f32::from(corners[0][channel]) * (1.0 - tx) + f32::from(corners[1][channel]) * tx;
        let bottom =
            f32::from(corners[2][channel]) * (1.0 - tx) + f32::from(corners[3][channel]) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let sampled_alpha = sample_channel(3) / 255.0;
    if sampled_alpha <= f32::EPSILON {
        return [0; 4];
    }
    let rgb = [0, 1, 2].map(|channel| {
        let premultiplied =
            corners.map(|pixel| srgb_linear(pixel[channel]) * f32::from(pixel[3]) / 255.0);
        let top = premultiplied[0] * (1.0 - tx) + premultiplied[1] * tx;
        let bottom = premultiplied[2] * (1.0 - tx) + premultiplied[3] * tx;
        let linear = (top * (1.0 - ty) + bottom * ty) / sampled_alpha;
        linear_srgb(linear)
    });
    [
        rgb[0],
        rgb[1],
        rgb[2],
        (sampled_alpha * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obj::ObjMaterial;

    #[test]
    fn uv_sampler_repeats_and_flips_v() {
        let texture = RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, // top row
                0, 0, 255, 255, 255, 255, 0, 255, // bottom row
            ],
        )
        .unwrap();
        assert_eq!(sample(&texture, [0.25, 0.75]), [255, 0, 0, 255]);
        assert_eq!(sample(&texture, [1.25, 0.75]), [255, 0, 0, 255]);
        assert_eq!(sample(&texture, [0.25, 0.25]), [0, 0, 255, 255]);
    }

    #[test]
    fn uv_sampler_filters_srgb_in_linear_light() {
        let texture =
            RgbaImage::from_raw(2, 1, vec![0, 0, 0, 255, 255, 255, 255, 255]).expect("texture");

        assert_eq!(sample(&texture, [0.5, 0.5]), [188, 188, 188, 255]);
    }

    #[test]
    fn uv_sampler_premultiplies_transparent_texels_before_filtering() {
        let texture = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).expect("texture");

        assert_eq!(sample(&texture, [0.5, 0.5]), [255, 0, 0, 128]);
    }

    #[test]
    fn repeated_material_texture_is_decoded_once() {
        let path = std::env::temp_dir().join(format!(
            "bmcbl-shared-obj-texture-{}.png",
            std::process::id()
        ));
        RgbaImage::from_raw(1, 1, vec![42, 84, 126, 255])
            .expect("pixel")
            .save(&path)
            .expect("fixture");
        let material = ObjMaterial {
            diffuse: None,
            opacity: 1.0,
            diffuse_texture: Some(path.clone()),
        };
        let model = ObjModel {
            triangles: Vec::new(),
            materials: vec![material.clone(), material],
        };
        let textures = load_textures(&model).expect("textures");
        assert!(Arc::ptr_eq(
            textures[0].as_ref().expect("first"),
            textures[1].as_ref().expect("second")
        ));
        std::fs::remove_file(path).expect("remove fixture");
    }
}

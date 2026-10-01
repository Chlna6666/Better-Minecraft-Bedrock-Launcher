//! Explicit crop, resize and tiling for Bedrock 128-pixel map records.

use anyhow::{Result, anyhow};
use bedrock_world::map_item::Pixels;
use image::RgbaImage;
use image::imageops::{self, FilterType};

const MAP_SIDE: u32 = 128;
const MAX_TILES: u32 = 1024;

/// Rectangle in source-image pixels; its right and bottom edges are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapImageCrop {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width, greater than zero.
    pub width: u32,
    /// Height, greater than zero.
    pub height: u32,
}

/// Image resampling used before splitting a map mosaic into 128×128 records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapResample {
    /// Keep hard pixel edges.
    Nearest,
    /// Smooth photographic input.
    Lanczos3,
}

/// Requested crop and grid resolution for direct Bedrock map pixels.
///
/// The output resolution is exactly `columns × 128` by `rows × 128` pixels. This does not
/// produce blocks, map records, inventory items or any LevelDB writes. The caller chooses crop
/// bounds explicitly and may present the resulting tiles before creating map records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapImageOptions {
    /// Source rectangle to display across all tiles.
    pub crop: MapImageCrop,
    /// Number of map records from west to east.
    pub columns: u32,
    /// Number of map records from north to south.
    pub rows: u32,
    /// Sampling used when the crop is scaled to the map grid.
    pub resample: MapResample,
    /// Alpha below this value becomes transparent; other pixels become fully opaque.
    pub alpha_threshold: u8,
}

/// One map's RGBA pixels and its position in a north-up mosaic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapImageTile {
    /// Zero-based column, increasing eastward.
    pub column: u32,
    /// Zero-based row, increasing southward.
    pub row: u32,
    /// Bedrock `colors` bytes in row-major RGBA order.
    pub pixels: Pixels,
}

/// Crops decoded RGBA input, resamples it to the requested map grid, then returns each tile.
///
/// No map id is allocated and no stored record is modified. Transparent pixels have zero RGB
/// so subsequent export cannot leak invisible source color. The 1024-tile limit bounds both
/// result size (64 MiB) and intermediate image allocations.
///
/// # Errors
///
/// Returns validation errors for bad dimensions, RGBA byte counts, crop bounds, arithmetic
/// overflow, or a grid exceeding 1024 maps.
pub fn tile_map_image(
    source_rgba: Vec<u8>,
    source_width: u32,
    source_height: u32,
    options: MapImageOptions,
) -> Result<Vec<MapImageTile>> {
    tile_map_image_with_progress(source_rgba, source_width, source_height, options, |_| {})
}

/// Performs the same conversion as [`tile_map_image`], reporting each completed 128×128 tile.
///
/// The callback runs on the caller's thread after a tile is ready, in north-up row-major order.
/// It can update a visible conversion task without moving image decoding or resizing into UI
/// rendering. This function reads no world data and writes no map records.
///
/// # Errors
/// Returns the same input validation and image errors as [`tile_map_image`].
pub fn tile_map_image_with_progress(
    source_rgba: Vec<u8>,
    source_width: u32,
    source_height: u32,
    options: MapImageOptions,
    mut completed: impl FnMut(u32),
) -> Result<Vec<MapImageTile>> {
    let Some(tile_count) = options.columns.checked_mul(options.rows) else {
        return Err(anyhow!("map grid dimensions overflow"));
    };
    if tile_count == 0 || tile_count > MAX_TILES {
        return Err(anyhow!("map grid must contain 1 to {MAX_TILES} tiles"));
    }
    let Some(output_width) = options.columns.checked_mul(MAP_SIDE) else {
        return Err(anyhow!("map grid width overflows"));
    };
    let Some(output_height) = options.rows.checked_mul(MAP_SIDE) else {
        return Err(anyhow!("map grid height overflows"));
    };
    let crop = options.crop;
    if crop.width == 0
        || crop.height == 0
        || crop
            .x
            .checked_add(crop.width)
            .is_none_or(|right| right > source_width)
        || crop
            .y
            .checked_add(crop.height)
            .is_none_or(|bottom| bottom > source_height)
    {
        return Err(anyhow!("map crop is outside the source image"));
    }
    let source = RgbaImage::from_raw(source_width, source_height, source_rgba)
        .ok_or_else(|| anyhow!("source RGBA byte count does not match dimensions"))?;
    let cropped = imageops::crop_imm(&source, crop.x, crop.y, crop.width, crop.height).to_image();
    let filter = match options.resample {
        MapResample::Nearest => FilterType::Nearest,
        MapResample::Lanczos3 => FilterType::Lanczos3,
    };
    let mut scaled = resize_rgba(&cropped, output_width, output_height, filter);
    for pixel in scaled.pixels_mut() {
        if pixel[3] < options.alpha_threshold {
            pixel.0 = [0, 0, 0, 0];
        } else {
            pixel[3] = 255;
        }
    }
    let mut tiles = Vec::with_capacity(tile_count as usize);
    for row in 0..options.rows {
        for column in 0..options.columns {
            let rgba = imageops::crop_imm(
                &scaled,
                column * MAP_SIDE,
                row * MAP_SIDE,
                MAP_SIDE,
                MAP_SIDE,
            )
            .to_image()
            .into_raw();
            tiles.push(MapImageTile {
                column,
                row,
                pixels: Pixels {
                    width: MAP_SIDE,
                    height: MAP_SIDE,
                    rgba,
                },
            });
            completed(tiles.len() as u32);
        }
    }
    Ok(tiles)
}

fn resize_rgba(source: &RgbaImage, width: u32, height: u32, filter: FilterType) -> RgbaImage {
    let mut premultiplied = source.clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }

    let mut scaled = imageops::resize(&premultiplied, width, height, filter);
    for pixel in scaled.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel.0 = [0, 0, 0, 0];
            continue;
        }
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    scaled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_and_grid_preserve_north_up_tile_order() {
        let mut source = Vec::new();
        for color in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]] {
            source.extend_from_slice(&color);
        }
        let mut completed = Vec::new();
        let tiles = tile_map_image_with_progress(
            source,
            3,
            1,
            MapImageOptions {
                crop: MapImageCrop {
                    x: 1,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                columns: 2,
                rows: 1,
                resample: MapResample::Nearest,
                alpha_threshold: 1,
            },
            |done| completed.push(done),
        )
        .expect("tiles");
        assert_eq!(completed, [1, 2]);
        assert_eq!(tiles.len(), 2);
        assert_eq!((tiles[0].column, tiles[0].row), (0, 0));
        assert_eq!(&tiles[0].pixels.rgba[..4], &[0, 255, 0, 255]);
        assert_eq!(&tiles[1].pixels.rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn invalid_crop_and_oversized_grid_are_rejected() {
        let options = MapImageOptions {
            crop: MapImageCrop {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            },
            columns: 1,
            rows: 1,
            resample: MapResample::Nearest,
            alpha_threshold: 1,
        };
        assert!(tile_map_image(vec![0; 4], 1, 1, options).is_err());
        assert!(
            tile_map_image(
                vec![0; 4],
                1,
                1,
                MapImageOptions {
                    crop: MapImageCrop {
                        x: 0,
                        ..options.crop
                    },
                    columns: 1025,
                    ..options
                }
            )
            .is_err()
        );
    }

    #[test]
    fn lanczos_keeps_transparent_edge_color_premultiplied() {
        let source = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba([120, 220, 80, 255])
            }
        });
        let image = resize_rgba(&source, 16, 1, FilterType::Lanczos3);
        for pixel in image.pixels() {
            if (16..=239).contains(&pixel[3]) {
                assert!(pixel[0] >= 118);
                assert!(pixel[1] >= 218);
                assert!(pixel[2] >= 78);
            }
        }
    }
}

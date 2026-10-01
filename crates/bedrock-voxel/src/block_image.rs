//! North-up flat image conversion into an immutable block placement plan.

use crate::{Result, validation};
use bedrock_world::{
    BlockState,
    editor::{BlockOffset, BlockPlacementPlan, PlacementBlock},
    surface::CancelFlag,
};

/// Color quantization behavior for image-to-block matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dithering {
    /// Select the closest block independently for each pixel.
    None,
    /// Diffuse sRGB channel error with serpentine Floyd–Steinberg, matching each pixel in OKLab.
    FloydSteinberg,
}

/// A block state and its Bedrock resource-pack appearance colors for voxel matching.
///
/// The built-in candidate set comes from the versioned palette in this crate. Callers supplying
/// custom candidates are responsible for checking that each identifier exists in the target
/// Bedrock version and that the colors match the block's actual texture. This type does not
/// persist or validate a world version.
#[derive(Debug, Clone)]
pub struct FlatBlockCandidate {
    /// Bedrock block state that the placement plan will write.
    pub state: BlockState,
    /// Top-face appearance color used for overhead Flat and 2.5D image plans.
    pub top_color: [u8; 4],
    /// Side-face appearance color used for vertical OBJ surfaces.
    pub side_color: [u8; 4],
}

/// Transparency handling for a flat north-up block image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatImageOptions<'a> {
    /// Whether color quantization error is diffused to adjacent pixels.
    pub dithering: Dithering,
    /// Pixels below this alpha are omitted from the plan.
    pub alpha_threshold: u8,
    /// RGB background used to composite retained semitransparent pixels.
    pub background: [u8; 3],
    /// A caller-selected block for pixels below the alpha threshold; `None` leaves them empty.
    pub transparent_fill: Option<&'a BlockState>,
}

/// A decoded north-up RGBA image supplied to the incremental Flat matcher.
#[derive(Debug, Clone, Copy)]
pub struct FlatImageInput<'a> {
    /// Row-major RGBA bytes.
    pub rgba: &'a [u8],
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
}

/// Matches RGBA image pixels to approved blocks in OKLab and returns local X/Z placements.
///
/// Image top is north (negative Z at the placement boundary only); local offsets start at
/// `(0, 0, 0)` and increase southward with each row. Width and height need not be multiples of
/// 128. This is a pure conversion: it reads or writes no world records. World-height, chunk,
/// collision, protected-record and source-snapshot checks belong to
/// [`BlockPlacementPlan::validate`]. Fully opaque pixels only match non-glass blocks. Pixels
/// with partial alpha may also match glass after both source and candidate colors are composited
/// over the selected background.
///
/// # Errors
///
/// Returns validation errors for empty or oversized dimensions, mismatched RGBA byte count,
/// empty candidates or an entirely transparent image without a transparent fill block.
pub fn flat_block_plan(
    rgba: &[u8],
    width: u32,
    height: u32,
    candidates: &[FlatBlockCandidate],
    options: FlatImageOptions<'_>,
) -> Result<BlockPlacementPlan> {
    flat_block_plan_with_progress(
        FlatImageInput {
            rgba,
            width,
            height,
        },
        candidates,
        options,
        &CancelFlag::new(),
        |_, _| {},
    )
}

/// Matches one decoded Flat image with per-row progress and cooperative cancellation.
///
/// `progress` receives completed and total pixel counts every 16 rows and at
/// completion. A cancelled conversion returns before building a placement plan;
/// no world or file has been modified.
///
/// # Errors
/// Returns the validation errors of [`flat_block_plan`] or a cancellation error.
pub fn flat_block_plan_with_progress(
    input: FlatImageInput<'_>,
    candidates: &[FlatBlockCandidate],
    options: FlatImageOptions<'_>,
    cancel: &CancelFlag,
    mut progress: impl FnMut(u32, u32),
) -> Result<BlockPlacementPlan> {
    let FlatImageInput {
        rgba,
        width,
        height,
    } = input;
    let Some(pixel_count) = width.checked_mul(height) else {
        return Err(validation("image dimensions overflow"));
    };
    let Some(byte_count) = pixel_count.checked_mul(4) else {
        return Err(validation("image byte count overflows"));
    };
    if pixel_count == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(validation("image dimensions are invalid"));
    }
    if rgba.len() != byte_count as usize {
        return Err(validation("RGBA byte count does not match image"));
    }
    if candidates.is_empty() {
        return Err(validation("no approved block colors were supplied"));
    }
    let candidate_rgb = candidates
        .iter()
        .map(|candidate| composite_over_background(&candidate.top_color, options.background))
        .collect::<Vec<_>>();
    let candidate_lab = candidate_rgb.iter().copied().map(oklab).collect::<Vec<_>>();
    let all_candidate_indices = (0..candidates.len()).collect::<Vec<_>>();
    let opaque_candidate_indices = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (!is_glass_candidate(candidate)).then_some(index))
        .collect::<Vec<_>>();
    let mut current_error = vec![[0.0; 3]; width as usize + 2];
    let mut next_error = vec![[0.0; 3]; width as usize + 2];
    let mut blocks = Vec::new();
    for (row, row_bytes) in rgba.chunks_exact(width as usize * 4).enumerate() {
        if cancel.is_cancelled() {
            return Err(validation("flat image conversion cancelled"));
        }
        let reverse = row % 2 == 1;
        for scan_column in 0..width as usize {
            let column = if reverse {
                width as usize - scan_column - 1
            } else {
                scan_column
            };
            let pixel = &row_bytes[column * 4..column * 4 + 4];
            let alpha = pixel[3];
            if alpha < options.alpha_threshold {
                if let Some(state) = options.transparent_fill {
                    blocks.push(PlacementBlock {
                        offset: BlockOffset {
                            x: column as i32,
                            y: 0,
                            z: row as i32,
                        },
                        state: state.clone(),
                    });
                }
                continue;
            }
            let color = composite_over_background(pixel, options.background);
            let target_rgb = match options.dithering {
                Dithering::None => color.map(f32::from),
                Dithering::FloydSteinberg => [0, 1, 2].map(|channel| {
                    (f32::from(color[channel]) + current_error[column + 1][channel])
                        .clamp(0.0, 255.0)
                }),
            };
            let target = oklab(target_rgb.map(|channel| channel.round() as u8));
            let match_indices = if pixel[3] > 0 && pixel[3] < u8::MAX {
                &all_candidate_indices
            } else {
                &opaque_candidate_indices
            };
            let best = match_indices
                .iter()
                .copied()
                .min_by(|a_index, b_index| {
                    distance_squared(target, candidate_lab[*a_index])
                        .total_cmp(&distance_squared(target, candidate_lab[*b_index]))
                        .then(a_index.cmp(b_index))
                })
                .ok_or_else(|| validation("no block color matched"))?;
            if options.dithering == Dithering::FloydSteinberg {
                diffuse_color_error(
                    subtract_color(target_rgb, candidate_rgb[best].map(f32::from)),
                    &mut current_error,
                    &mut next_error,
                    column,
                    width as usize,
                    reverse,
                );
            }
            blocks.push(PlacementBlock {
                offset: BlockOffset {
                    x: column as i32,
                    y: 0,
                    z: row as i32,
                },
                state: candidates[best].state.clone(),
            });
        }
        let completed_rows = row as u32 + 1;
        if completed_rows % 16 == 0 || completed_rows == height {
            progress(completed_rows * width, pixel_count);
        }
        std::mem::swap(&mut current_error, &mut next_error);
        next_error.fill([0.0; 3]);
    }
    BlockPlacementPlan::new(blocks).map_err(|error| error.to_string())
}

pub(crate) fn is_glass_candidate(candidate: &FlatBlockCandidate) -> bool {
    candidate.state.name == "minecraft:glass" || candidate.state.name.ends_with("_glass")
}

pub(crate) fn srgb_linear(channel: u8) -> f32 {
    let value = f32::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn linear_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

pub(crate) fn composite_over_background(pixel: &[u8], background: [u8; 3]) -> [u8; 3] {
    let alpha = f32::from(pixel[3]) / 255.0;
    [0, 1, 2].map(|channel| {
        let foreground = srgb_linear(pixel[channel]);
        let background = srgb_linear(background[channel]);
        linear_srgb(foreground * alpha + background * (1.0 - alpha))
    })
}

pub(crate) fn oklab(color: [u8; 3]) -> [f32; 3] {
    let [red, green, blue] = color.map(srgb_linear);
    let l = (0.41222146 * red + 0.53633255 * green + 0.051445995 * blue).cbrt();
    let m = (0.2119035 * red + 0.6806995 * green + 0.10739696 * blue).cbrt();
    let s = (0.08830246 * red + 0.28171885 * green + 0.6299787 * blue).cbrt();
    [
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
    ]
}

pub(crate) fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn subtract_color(color: [f32; 3], other: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|channel| color[channel] - other[channel])
}

fn diffuse_color_error(
    error: [f32; 3],
    current_row: &mut [[f32; 3]],
    next_row: &mut [[f32; 3]],
    column: usize,
    width: usize,
    reverse: bool,
) {
    let direction = if reverse { -1_isize } else { 1 };
    add_weighted_error(
        &mut current_row[(column as isize + 1 + direction) as usize],
        error,
        7.0 / 16.0,
    );
    add_weighted_error(
        &mut next_row[(column as isize + 1 - direction) as usize],
        error,
        3.0 / 16.0,
    );
    add_weighted_error(&mut next_row[column + 1], error, 5.0 / 16.0);
    add_weighted_error(
        &mut next_row[(column as isize + 1 + direction) as usize],
        error,
        1.0 / 16.0,
    );
    debug_assert_eq!(current_row.len(), width + 2);
    debug_assert_eq!(next_row.len(), width + 2);
}

fn add_weighted_error(target: &mut [f32; 3], error: [f32; 3], weight: f32) {
    for channel in 0..3 {
        target[channel] += error[channel] * weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn candidate(name: &str, rgb: [u8; 3]) -> FlatBlockCandidate {
        FlatBlockCandidate {
            state: BlockState {
                name: name.into(),
                states: BTreeMap::new(),
                version: None,
            },
            top_color: [rgb[0], rgb[1], rgb[2], 255],
            side_color: [rgb[0], rgb[1], rgb[2], 255],
        }
    }

    #[test]
    fn error_diffusion_uses_both_sides_of_a_midpoint_palette() {
        let candidates = [
            candidate("minecraft:black_wool", [0, 0, 0]),
            candidate("minecraft:white_wool", [255, 255, 255]),
        ];
        let image = [128, 128, 128, 255].repeat(8);
        let plan = flat_block_plan(
            &image,
            8,
            1,
            &candidates,
            FlatImageOptions {
                dithering: Dithering::FloydSteinberg,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
        )
        .expect("dithered plan");
        let black = plan
            .blocks()
            .iter()
            .filter(|block| block.state.name == "minecraft:black_wool")
            .count();
        let white = plan.blocks().len() - black;

        assert!((3..=5).contains(&black));
        assert!((3..=5).contains(&white));
    }

    #[test]
    fn nearest_color_matches_each_pixel_without_spreading_error() {
        let candidates = [
            candidate("minecraft:black_wool", [0, 0, 0]),
            candidate("minecraft:white_wool", [255, 255, 255]),
        ];
        let plan = flat_block_plan(
            &[128, 128, 128, 255].repeat(8),
            8,
            1,
            &candidates,
            FlatImageOptions {
                dithering: Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
        )
        .expect("nearest-color plan");

        assert!(
            plan.blocks()
                .iter()
                .all(|block| block.state == plan.blocks()[0].state)
        );
    }

    #[test]
    fn dithering_preserves_srgb_average_between_saturated_palette_colors() {
        let candidates = [
            candidate("minecraft:red_wool", [255, 0, 0]),
            candidate("minecraft:blue_wool", [0, 0, 255]),
        ];
        let plan = flat_block_plan(
            &[128, 0, 128, 255].repeat(128),
            16,
            8,
            &candidates,
            FlatImageOptions {
                dithering: Dithering::FloydSteinberg,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
        )
        .expect("dithered purple plane");
        let red_count = plan
            .blocks()
            .iter()
            .filter(|block| block.state.name == "minecraft:red_wool")
            .count();
        let average_red = red_count as f32 * 255.0 / 128.0;
        assert!(
            (average_red - 128.0).abs() < 8.0,
            "purple hue shifted: {average_red}"
        );
    }

    #[test]
    fn arbitrary_width_preserves_north_up_positions_and_skips_alpha() {
        let candidates = [
            candidate("minecraft:red_wool", [255, 0, 0]),
            candidate("minecraft:blue_wool", [0, 0, 255]),
        ];
        let blocks = flat_block_plan(
            &[255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 255],
            3,
            1,
            &candidates,
            FlatImageOptions {
                dithering: Dithering::None,
                alpha_threshold: 1,
                background: [255, 255, 255],
                transparent_fill: None,
            },
        )
        .expect("flat plan");
        assert_eq!(blocks.blocks().len(), 2);
        assert_eq!(blocks.blocks()[0].offset.x, 0);
        assert_eq!(blocks.blocks()[1].offset.x, 2);
        assert_eq!(blocks.blocks()[1].state.name, "minecraft:blue_wool");
        assert_eq!(blocks.blocks()[1].state, candidates[1].state);
    }

    #[test]
    fn progress_counts_pixels_and_cancel_discards_plan() {
        let rgba = [255, 0, 0, 255, 255, 0, 0, 255];
        let candidates = [candidate("minecraft:red_wool", [255, 0, 0])];
        let input = FlatImageInput {
            rgba: &rgba,
            width: 1,
            height: 2,
        };
        let options = FlatImageOptions {
            dithering: Dithering::None,
            alpha_threshold: 1,
            background: [255; 3],
            transparent_fill: None,
        };
        let cancel = CancelFlag::new();
        let mut updates = Vec::new();
        flat_block_plan_with_progress(input, &candidates, options, &cancel, |done, total| {
            updates.push((done, total));
        })
        .expect("flat plan");
        assert_eq!(updates, [(2, 2)]);

        cancel.cancel();
        assert!(
            flat_block_plan_with_progress(input, &candidates, options, &cancel, |_, _| {}).is_err()
        );
    }

    #[test]
    fn transparent_fill_uses_selected_block_without_color_matching() {
        let candidates = [candidate("minecraft:red_wool", [255, 0, 0])];
        let fill = candidate("minecraft:quartz_block", [255, 255, 255]);
        let plan = flat_block_plan(
            &[255, 0, 0, 255, 0, 0, 0, 0],
            2,
            1,
            &candidates,
            FlatImageOptions {
                dithering: Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: Some(&fill.state),
            },
        )
        .expect("filled plan");
        assert_eq!(plan.blocks().len(), 2);
        assert_eq!(plan.blocks()[0].state.name, "minecraft:red_wool");
        assert_eq!(plan.blocks()[1].state.name, "minecraft:quartz_block");
    }

    #[test]
    fn semitransparent_pixels_composite_in_linear_light() {
        assert_eq!(
            composite_over_background(&[0, 0, 0, 128], [255; 3]),
            [187, 187, 187]
        );
    }

    #[test]
    fn glass_candidates_are_limited_to_translucent_source_pixels() {
        let glass = FlatBlockCandidate {
            state: BlockState {
                name: "minecraft:glass".into(),
                states: BTreeMap::new(),
                version: None,
            },
            top_color: [255, 255, 255, 128],
            side_color: [255, 255, 255, 128],
        };
        let opaque_gray = candidate("minecraft:gray_wool", [188, 188, 188]);
        let options = FlatImageOptions {
            dithering: Dithering::None,
            alpha_threshold: 1,
            background: [0; 3],
            transparent_fill: None,
        };

        let opaque = flat_block_plan(
            &[255, 255, 255, 255],
            1,
            1,
            &[glass.clone(), opaque_gray.clone()],
            options,
        )
        .expect("opaque image plan");
        assert_eq!(opaque.blocks()[0].state.name, "minecraft:gray_wool");

        let translucent =
            flat_block_plan(&[255, 255, 255, 128], 1, 1, &[glass, opaque_gray], options)
                .expect("translucent image plan");
        assert_eq!(translucent.blocks()[0].state.name, "minecraft:glass");
    }
}

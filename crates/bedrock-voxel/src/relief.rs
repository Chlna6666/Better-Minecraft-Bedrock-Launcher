//! Image luminance relief plans.

use bedrock_world::{
    editor::{BlockOffset, BlockPlacementPlan, PlacementBlock},
    surface::CancelFlag,
};

use crate::{
    FlatBlockCandidate, FlatImageInput, FlatImageOptions, Result, block_image,
    flat_block_plan_with_progress, validation,
};

/// Maximum dense structure volume accepted by luminance-relief conversion.
pub const MAX_RELIEF_BLOCK_VOLUME: u64 = 8_000_000;

/// How an image relief fills space below each matched surface block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReliefImageFill {
    /// Keep one matched block at each pixel's solved height.
    SurfaceOnly,
    /// Fill each pixel column from the base plane up to the matched surface block.
    Solid,
}

/// Height and volume settings for an image luminance relief.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReliefImageOptions {
    /// Number of vertical levels, from 2 to 128 blocks.
    pub max_height: u16,
    /// Whether each pixel is a surface point or a filled column.
    pub fill: ReliefImageFill,
}

/// Converts a north-up RGBA image to a luminance-height block relief.
///
/// Block colors are selected by the same appearance-color OKLab matcher as Flat images. Relative
/// height comes from linear-light luminance, with image north at negative Z. This is an image
/// relief mode; it does not simulate or claim calibrated Bedrock map-color height shading. The
/// maximum dense structure volume is bounded to eight million cells.
///
/// # Errors
///
/// Returns the Flat matcher validation errors, cancellation, an invalid height range, or a
/// validation error when the worst-case structure volume exceeds the in-memory budget.
pub fn relief_block_plan_with_progress(
    input: FlatImageInput<'_>,
    candidates: &[FlatBlockCandidate],
    image_options: FlatImageOptions<'_>,
    relief_options: ReliefImageOptions,
    cancel: &CancelFlag,
    mut progress: impl FnMut(u64, u64),
) -> Result<BlockPlacementPlan> {
    if !(2..=128).contains(&relief_options.max_height) {
        return Err(validation("relief height must be between 2 and 128 blocks"));
    }
    let pixel_count = u64::from(input.width)
        .checked_mul(u64::from(input.height))
        .ok_or_else(|| validation("image pixel count overflows"))?;
    let worst_case_volume = pixel_count
        .checked_mul(u64::from(relief_options.max_height))
        .ok_or_else(|| validation("relief volume overflows"))?;
    if worst_case_volume > MAX_RELIEF_BLOCK_VOLUME {
        return Err(validation(format!(
            "relief structure may exceed the eight-million-cell limit ({worst_case_volume})"
        )));
    }

    let flat_plan =
        flat_block_plan_with_progress(input, candidates, image_options, cancel, |done, total| {
            progress(u64::from(done), u64::from(total))
        })?;
    let mut blocks = Vec::with_capacity(flat_plan.blocks().len());
    let image_columns = flat_plan.blocks().len() as u64;
    let total = pixel_count.saturating_add(image_columns);

    for (index, source_block) in flat_plan.blocks().iter().enumerate() {
        if index.is_multiple_of(input.width.max(1) as usize) && cancel.is_cancelled() {
            return Err(validation("relief image conversion cancelled"));
        }
        let pixel_index = (source_block.offset.z as usize)
            .checked_mul(input.width as usize)
            .and_then(|row| row.checked_add(source_block.offset.x as usize))
            .ok_or_else(|| validation("relief pixel position overflows"))?;
        let start = pixel_index
            .checked_mul(4)
            .ok_or_else(|| validation("relief pixel byte offset overflows"))?;
        let end = start
            .checked_add(4)
            .ok_or_else(|| validation("relief pixel byte offset overflows"))?;
        let pixel = input
            .rgba
            .get(start..end)
            .ok_or_else(|| validation("relief plan contains an invalid pixel position"))?;
        let top_y = if pixel[3] < image_options.alpha_threshold {
            0
        } else {
            luminance_height(pixel, image_options.background, relief_options.max_height)
        };

        if relief_options.fill == ReliefImageFill::Solid {
            for y in 0..top_y {
                blocks.push(PlacementBlock {
                    offset: BlockOffset {
                        x: source_block.offset.x,
                        y,
                        z: source_block.offset.z,
                    },
                    state: source_block.state.clone(),
                });
            }
        }
        blocks.push(PlacementBlock {
            offset: BlockOffset {
                x: source_block.offset.x,
                y: top_y,
                z: source_block.offset.z,
            },
            state: source_block.state.clone(),
        });
        if (index + 1).is_multiple_of(1024) {
            progress(pixel_count.saturating_add(index as u64 + 1), total);
        }
    }
    if cancel.is_cancelled() {
        return Err(validation("relief image conversion cancelled"));
    }
    progress(total, total);
    BlockPlacementPlan::new(blocks).map_err(|error| error.to_string())
}

fn luminance_height(pixel: &[u8], background: [u8; 3], max_height: u16) -> i32 {
    let color = block_image::composite_over_background(pixel, background);
    let [red, green, blue] = color.map(block_image::srgb_linear);
    let luminance = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    (luminance * f32::from(max_height - 1)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::BlockState;
    use std::collections::BTreeMap;

    fn candidate(name: &str, color: [u8; 4]) -> FlatBlockCandidate {
        FlatBlockCandidate {
            state: BlockState {
                name: name.to_owned(),
                states: BTreeMap::new(),
                version: None,
            },
            top_color: color,
            side_color: color,
        }
    }

    #[test]
    fn luminance_relief_keeps_north_up_and_fills_solid_columns() {
        let candidates = [candidate("minecraft:white_wool", [255, 255, 255, 255])];
        let plan = relief_block_plan_with_progress(
            FlatImageInput {
                rgba: &[0, 0, 0, 255, 255, 255, 255, 255],
                width: 1,
                height: 2,
            },
            &candidates,
            FlatImageOptions {
                dithering: crate::Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
            ReliefImageOptions {
                max_height: 4,
                fill: ReliefImageFill::Solid,
            },
            &CancelFlag::new(),
            |_, _| {},
        )
        .expect("relief plan");

        assert_eq!(plan.blocks().len(), 5);
        assert_eq!(plan.blocks()[0].offset, BlockOffset { x: 0, y: 0, z: 0 });
        assert_eq!(plan.blocks()[1].offset, BlockOffset { x: 0, y: 0, z: 1 });
        assert_eq!(plan.blocks()[4].offset, BlockOffset { x: 0, y: 3, z: 1 });
        assert!(
            plan.blocks()
                .iter()
                .all(|block| block.state.name == "minecraft:white_wool")
        );
    }

    #[test]
    fn surface_relief_places_one_block_at_the_luminance_height() {
        let plan = relief_block_plan_with_progress(
            FlatImageInput {
                rgba: &[255, 255, 255, 255],
                width: 1,
                height: 1,
            },
            &[candidate("minecraft:white_wool", [255; 4])],
            FlatImageOptions {
                dithering: crate::Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
            ReliefImageOptions {
                max_height: 8,
                fill: ReliefImageFill::SurfaceOnly,
            },
            &CancelFlag::new(),
            |_, _| {},
        )
        .expect("surface relief plan");

        assert_eq!(plan.blocks().len(), 1);
        assert_eq!(plan.blocks()[0].offset, BlockOffset { x: 0, y: 7, z: 0 });
    }

    #[test]
    fn relief_rejects_a_height_volume_beyond_the_structure_budget() {
        let error = relief_block_plan_with_progress(
            FlatImageInput {
                rgba: &[],
                width: 512,
                height: 512,
            },
            &[candidate("minecraft:white_wool", [255; 4])],
            FlatImageOptions {
                dithering: crate::Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
            ReliefImageOptions {
                max_height: 128,
                fill: ReliefImageFill::SurfaceOnly,
            },
            &CancelFlag::new(),
            |_, _| {},
        )
        .expect_err("large relief is rejected before matching");
        assert!(error.contains("eight-million-cell limit"));
    }
}

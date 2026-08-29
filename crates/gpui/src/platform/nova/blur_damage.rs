use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BlurDamageScissors {
    /// Scene-color pixels that must be reconstructed for the horizontal convolution.
    pub(super) source_capture: ScissorRect,
    /// Horizontal-pass output that must be updated in the retained ping target.
    pub(super) horizontal_output: ScissorRect,
    /// Final vertical-pass output that is visible and affected by this frame's source damage.
    pub(super) final_output: ScissorRect,
}

/// Computes the minimal conservative convolution footprint for a cached backdrop filter.
///
/// The scene-color source is scratch storage and is cleared on every refresh, while the horizontal
/// and final Gaussian targets are retained between frames. Given source pixels changed by this
/// frame, the final affected output is one kernel support around that damage. The vertical pass
/// needs one extra support in Y from the horizontal cache, and the horizontal pass in turn needs
/// one support in X from the source capture. This keeps a small animation under a full-window blur
/// local without reading undefined pixels from the cleared scratch source.
pub(super) fn blur_damage_scissors(
    config: BackdropBlurConfig,
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
) -> Option<BlurDamageScissors> {
    if dirty_region.is_empty() {
        return None;
    }
    if dirty_region.is_full() {
        let source_capture = blur_full_source_scissor(config, drawable_size)?;
        let horizontal_output = source_capture;
        let final_output = blur_output_scissor(config, drawable_size)?;
        return Some(BlurDamageScissors {
            source_capture,
            horizontal_output,
            final_output,
        });
    }

    let full_source = blur_full_source_scissor(config, drawable_size)?;
    let output_bounds = blur_output_scissor(config, drawable_size)?;
    let mut dirty_source = None::<ScissorRect>;
    for rect in dirty_region.rects() {
        let Some(rect_scissor) = bounds_to_scissor(rect.bounds, drawable_size) else {
            continue;
        };
        let affected = intersect_scissor_rects(rect_scissor, full_source);
        if affected.is_empty() {
            continue;
        }
        dirty_source = Some(match dirty_source {
            Some(current) => union_scissor_rects(current, affected),
            None => affected,
        });
    }
    let dirty_source = dirty_source?;

    let support = gaussian_support_pixels(config, drawable_size);
    // A changed source texel affects final Gaussian output by one support on both axes.
    let final_output = intersect_scissor_rects(
        dilate_scissor(dirty_source, support, support, drawable_size),
        output_bounds,
    );
    if final_output.is_empty() {
        return None;
    }

    // Vertical convolution reads retained horizontal rows one support above and below the final
    // output. Recompute exactly those horizontal rows; unchanged rows remain valid in the target.
    let horizontal_output = intersect_scissor_rects(
        dilate_scissor(final_output, 0, support, drawable_size),
        full_source,
    );
    if horizontal_output.is_empty() {
        return None;
    }

    // Horizontal convolution reads source texels one support to either side. The source texture is
    // scratch and starts cleared, so reconstruct this dependency halo before filtering.
    let source_capture = intersect_scissor_rects(
        dilate_scissor(horizontal_output, support, 0, drawable_size),
        full_source,
    );
    if source_capture.is_empty() {
        return None;
    }

    Some(BlurDamageScissors {
        source_capture,
        horizontal_output,
        final_output,
    })
}

pub(super) fn blur_full_source_scissor(
    config: BackdropBlurConfig,
    drawable_size: DrawableSize,
) -> Option<ScissorRect> {
    let [x, y, width, height] = config.bounds();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let support = 3.0 * config.radius().max(0.0) + 1.0;
    let left = floor_clamped_u32(x - support, drawable_size.width);
    let top = floor_clamped_u32(y - support, drawable_size.height);
    let right = ceil_clamped_u32(x + width + support, drawable_size.width);
    let bottom = ceil_clamped_u32(y + height + support, drawable_size.height);
    nonempty_scissor(ScissorRect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    })
}

pub(super) fn blur_output_scissor(
    config: BackdropBlurConfig,
    drawable_size: DrawableSize,
) -> Option<ScissorRect> {
    let [x, y, width, height] = config.bounds();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let left = floor_clamped_u32(x, drawable_size.width);
    let top = floor_clamped_u32(y, drawable_size.height);
    let right = ceil_clamped_u32(x + width, drawable_size.width);
    let bottom = ceil_clamped_u32(y + height, drawable_size.height);
    nonempty_scissor(ScissorRect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    })
}

pub(super) fn bounds_to_scissor(
    bounds: Bounds<ScaledPixels>,
    drawable_size: DrawableSize,
) -> Option<ScissorRect> {
    let left = floor_clamped_u32(bounds.origin.x.0, drawable_size.width);
    let top = floor_clamped_u32(bounds.origin.y.0, drawable_size.height);
    let right = ceil_clamped_u32(bounds.right().0, drawable_size.width);
    let bottom = ceil_clamped_u32(bounds.bottom().0, drawable_size.height);
    nonempty_scissor(ScissorRect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    })
}

fn gaussian_support_pixels(config: BackdropBlurConfig, drawable_size: DrawableSize) -> u32 {
    let support = 3.0 * config.radius().max(0.0) + 1.0;
    ceil_clamped_u32(
        support,
        drawable_size.width.max(drawable_size.height).max(1),
    )
}

pub(super) fn dilate_scissor(
    scissor: ScissorRect,
    x_amount: u32,
    y_amount: u32,
    drawable_size: DrawableSize,
) -> ScissorRect {
    let left = scissor.x.saturating_sub(x_amount);
    let top = scissor.y.saturating_sub(y_amount);
    let right = scissor
        .x
        .saturating_add(scissor.width)
        .saturating_add(x_amount)
        .min(drawable_size.width);
    let bottom = scissor
        .y
        .saturating_add(scissor.height)
        .saturating_add(y_amount)
        .min(drawable_size.height);
    ScissorRect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    }
}

pub(super) fn union_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let right_edge = left
        .x
        .saturating_add(left.width)
        .max(right.x.saturating_add(right.width));
    let bottom_edge = left
        .y
        .saturating_add(left.height)
        .max(right.y.saturating_add(right.height));
    ScissorRect {
        x,
        y,
        width: right_edge.saturating_sub(x),
        height: bottom_edge.saturating_sub(y),
    }
}

pub(super) fn intersect_scissor_rects(left: ScissorRect, right: ScissorRect) -> ScissorRect {
    let x = left.x.max(right.x);
    let y = left.y.max(right.y);
    let right_edge = left
        .x
        .saturating_add(left.width)
        .min(right.x.saturating_add(right.width));
    let bottom_edge = left
        .y
        .saturating_add(left.height)
        .min(right.y.saturating_add(right.height));
    ScissorRect {
        x,
        y,
        width: right_edge.saturating_sub(x),
        height: bottom_edge.saturating_sub(y),
    }
}

fn nonempty_scissor(scissor: ScissorRect) -> Option<ScissorRect> {
    (!scissor.is_empty()).then_some(scissor)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite clamped blur bounds are converted to integer scissor coordinates"
)]
fn floor_clamped_u32(value: f32, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= limit as f32 {
        limit
    } else {
        value.floor() as u32
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite clamped blur bounds are converted to integer scissor coordinates"
)]
fn ceil_clamped_u32(value: f32, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        0
    } else if value >= limit as f32 {
        limit
    } else {
        value.ceil() as u32
    }
}

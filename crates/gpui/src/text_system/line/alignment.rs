use crate::{LineLayout, Pixels, Point, TextAlign, WrapBoundary, px};

fn snap_local_baseline_offset_to_device_pixels(
    baseline_offset_y: Pixels,
    scale_factor: f32,
) -> Pixels {
    px((baseline_offset_y.0 * scale_factor).round() / scale_factor)
}

pub(super) fn snap_baseline_offset_to_device_pixels(
    origin_y: Pixels,
    baseline_offset_y: Pixels,
    scale_factor: f32,
) -> Pixels {
    if crate::element::layout_animation_text_motion_active() {
        // The row/card origin moves continuously while a layout spring is active. Snapping the
        // absolute baseline on every sample quantizes only the glyph to whole device pixels while
        // its background keeps moving fractionally, producing the visible one-pixel stepping of
        // small labels such as version numbers. Keep one local raster phase during motion and let
        // paint_glyph move the already-rasterized sprite continuously with the element. The
        // LayoutAnimationTarget settle frame rebuilds the subtree once with the normal static snap.
        return snap_local_baseline_offset_to_device_pixels(baseline_offset_y, scale_factor);
    }

    px((((origin_y + baseline_offset_y).0 * scale_factor).round() / scale_factor) - origin_y.0)
}

pub(super) fn aligned_origin_x(
    origin: Point<Pixels>,
    align_width: Pixels,
    last_glyph_x: Pixels,
    align: &TextAlign,
    layout: &LineLayout,
    wrap_boundary: Option<&&WrapBoundary>,
) -> Pixels {
    let end_of_line = if let Some(WrapBoundary { run_ix, glyph_ix }) = wrap_boundary {
        layout.runs[*run_ix].glyphs[*glyph_ix].position.x
    } else {
        layout.width
    };

    let line_width = end_of_line - last_glyph_x;

    match align {
        TextAlign::Left => origin.x,
        TextAlign::Center => (origin.x * 2.0 + align_width - line_width) / 2.0,
        TextAlign::Right => origin.x + align_width - line_width,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approximately_eq(left: Pixels, right: Pixels) {
        assert!(
            (left.0 - right.0).abs() < 0.0001,
            "{left:?} was not approximately {right:?}",
        );
    }

    #[test]
    fn baseline_offset_snaps_to_device_pixels_once() {
        let snapped = snap_baseline_offset_to_device_pixels(px(0.25), px(12.3), 1.5);

        assert_approximately_eq(snapped, px(12.416667));
        assert_approximately_eq(px((px(0.25) + snapped).0 * 1.5), px(19.0));
    }

    #[test]
    fn baseline_offset_preserves_logical_snap_at_one_x_scale() {
        let snapped = snap_baseline_offset_to_device_pixels(px(2.2), px(10.4), 1.0);

        assert_approximately_eq(snapped, px(10.8));
        assert_approximately_eq(px(2.2) + snapped, px(13.0));
    }

    #[test]
    fn layout_motion_local_phase_is_device_aligned() {
        let snapped = snap_local_baseline_offset_to_device_pixels(px(12.3), 1.5);

        assert_approximately_eq(snapped, px(12.0));
        assert_approximately_eq(px(snapped.0 * 1.5), px(18.0));
    }

    #[test]
    fn layout_motion_local_phase_does_not_quantize_parent_origin() {
        let local = snap_local_baseline_offset_to_device_pixels(px(10.4), 1.0);

        assert_approximately_eq(local, px(10.0));
        assert_approximately_eq(px(4.25) + local, px(14.25));
        assert_approximately_eq(px(4.75) + local, px(14.75));
    }
}

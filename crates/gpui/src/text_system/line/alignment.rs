use crate::{LineLayout, Pixels, Point, TextAlign, WrapBoundary, px};

pub(super) fn snap_baseline_offset_to_device_pixels(
    origin_y: Pixels,
    baseline_offset_y: Pixels,
    scale_factor: f32,
) -> Pixels {
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
}

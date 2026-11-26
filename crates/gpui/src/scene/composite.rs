use super::PaintOperation;
use crate::ScaledPixels;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentMask, Quad, bounds, point, size};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> crate::Bounds<ScaledPixels> {
        bounds(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        )
    }

    #[test]
    fn composite_capture_uses_real_pixel_bounds_and_zero_filter() {
        let layout_bounds = rect(0.0, 0.0, 1000.0, 700.0);
        let pixel_bounds = rect(40.0, 96.0, 420.0, 240.0);
        let mut scene = crate::scene::Painter::default();

        scene.begin_blur(super::super::BlurCapture {
            animation_id: None,
            layer_id: None,
            bounds: layout_bounds,
            content_mask: ContentMask::new(layout_bounds),
            radius: ScaledPixels(0.0),
            opacity: 1.0,
        });
        scene.push_layer(layout_bounds);
        scene.insert_primitive(Quad {
            bounds: pixel_bounds,
            content_mask: ContentMask::new(layout_bounds),
            ..Default::default()
        });
        scene.pop_layer();
        scene.end_blur();

        assert_eq!(scene.blurs.len(), 1);
        assert_eq!(scene.blurs[0].bounds, pixel_bounds);
        assert_eq!(scene.blurs[0].radius, ScaledPixels(0.0));

        let Some(PaintOperation::StartBlur(capture)) = scene.paint_operations.first() else {
            panic!("composite capture must keep its retained start marker");
        };
        assert_eq!(capture.bounds, pixel_bounds);
        assert_eq!(capture.radius, ScaledPixels(0.0));

        let layer_bounds = scene
            .paint_operations
            .iter()
            .find_map(|operation| match operation {
                PaintOperation::StartLayer(bounds) => Some(*bounds),
                _ => None,
            });
        assert_eq!(layer_bounds, Some(pixel_bounds));
    }
}

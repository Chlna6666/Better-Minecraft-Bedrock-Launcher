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
    fn retained_composite_replay_shares_published_content() {
        use std::sync::Arc;

        let bounds = rect(0.0, 0.0, 32.0, 24.0);
        let mut previous = crate::scene::Painter::default();
        previous.begin_blur(super::super::BlurCapture {
            animation_id: None,
            layer_id: Some(7),
            bounds,
            content_mask: ContentMask::new(bounds),
            radius: ScaledPixels(0.0),
            opacity: 1.0,
        });
        previous.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask::new(bounds),
            ..Default::default()
        });
        previous.end_blur();
        previous.finish();

        let mut next = crate::scene::Painter::default();
        next.replay(0..previous.len(), &previous, std::iter::empty());
        next.finish();

        assert_eq!(next.blurs.len(), 1);
        // One root output was replayed, without rebuilding its child painter.
        assert_eq!(next.replayed_primitives, 1);
        assert_eq!(next.capture_groups[0].range, 0..previous.len());
        assert_eq!(next.capture_groups[0].bounds, previous.capture_groups[0].bounds);
        assert!(Arc::ptr_eq(&previous.blurs[0].content, &next.blurs[0].content));
        assert_eq!(previous.blurs[0].content.revision, next.blurs[0].content.revision);
    }

    #[test]
    fn static_capture_replay_rebinds_root_order_and_keeps_content() {
        use std::sync::Arc;

        let bounds = rect(0.0, 0.0, 32.0, 24.0);
        let mut previous = crate::scene::Painter::default();
        previous.begin_blur(super::super::BlurCapture {
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: ContentMask::new(bounds),
            radius: ScaledPixels(0.0),
            opacity: 1.0,
        });
        for _ in 0..4 {
            previous.insert_primitive(Quad {
                bounds,
                content_mask: ContentMask::new(bounds),
                ..Default::default()
            });
        }
        previous.end_blur();
        previous.finish();

        let mut next = crate::scene::Painter::default();
        next.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask::new(bounds),
            ..Default::default()
        });
        let start = next.len();
        assert!(!next.replay(0..previous.len(), &previous, []));
        next.finish();

        assert_eq!(next.replayed_primitives, 1);
        assert_eq!(next.capture_groups[0].range, start..start + previous.len());
        assert_ne!(next.blurs[0].order, previous.blurs[0].order);
        assert!(Arc::ptr_eq(&next.blurs[0].content, &previous.blurs[0].content));
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

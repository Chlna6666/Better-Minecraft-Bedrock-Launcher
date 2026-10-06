use super::{ProjectionRegion, SceneView};
use anyhow::bail;
use gfx_core::Extent2d;
use gpui::{Bounds, RendererExtensionContext, ScaledPixels, size};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FrameProjection {
    bounds: Bounds<ScaledPixels>,
    target_extent: Extent2d,
    aspect: f32,
    blend_edge_feather: f32,
}

impl FrameProjection {
    pub(super) fn new(
        scene_view: &SceneView,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let element_bounds = inset_bounds(
            context.bounds(),
            scene_view.inset_max_pixels,
            scene_view.inset_fraction,
        );
        let bounds = projection_bounds(
            element_bounds,
            context.content_mask().bounds,
            scene_view.projection_region,
        );
        let width = f64::from(bounds.size.width) as f32;
        let height = f64::from(bounds.size.height) as f32;
        let aspect = width / height;
        if !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || !aspect.is_finite()
        {
            bail!("GPUI 3D projection region is empty or invalid")
        }
        Ok(Self {
            bounds,
            target_extent: context.viewport(),
            aspect,
            blend_edge_feather: scene_view.blend_edge_feather,
        })
    }

    pub(super) fn bounds(self) -> Bounds<ScaledPixels> {
        self.bounds
    }

    pub(super) fn target_extent(self) -> Extent2d {
        self.target_extent
    }

    pub(super) fn aspect(self) -> f32 {
        self.aspect
    }

    pub(super) fn blend_edge_feather(self) -> f32 {
        self.blend_edge_feather
    }
}

fn inset_bounds(
    mut bounds: Bounds<ScaledPixels>,
    max_pixels: f32,
    fraction: f32,
) -> Bounds<ScaledPixels> {
    if max_pixels == 0.0 || fraction == 0.0 {
        return bounds;
    }
    let width = f64::from(bounds.size.width) as f32;
    let height = f64::from(bounds.size.height) as f32;
    let horizontal: ScaledPixels = max_pixels.min(width * fraction).into();
    let vertical: ScaledPixels = max_pixels.min(height * fraction).into();
    bounds.origin.x += horizontal;
    bounds.origin.y += vertical;
    bounds.size.width -= horizontal + horizontal;
    bounds.size.height -= vertical + vertical;
    bounds
}

fn projection_bounds(
    element_bounds: Bounds<ScaledPixels>,
    visible_bounds: Bounds<ScaledPixels>,
    region: ProjectionRegion,
) -> Bounds<ScaledPixels> {
    match region {
        ProjectionRegion::ElementBounds => element_bounds,
        ProjectionRegion::VisibleContent => element_bounds.intersect(&visible_bounds),
        ProjectionRegion::VisibleSquare => {
            let visible = element_bounds.intersect(&visible_bounds);
            let side = visible.size.width.min(visible.size.height);
            Bounds::centered_at(visible.center(), size(side, side))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{inset_bounds, projection_bounds};
    use crate::ProjectionRegion;
    use gpui::{Bounds, bounds, point, px, size};

    #[test]
    fn visible_projection_uses_the_content_mask_intersection() {
        let element = bounds(point(px(10.0), px(20.0)), size(px(300.0), px(200.0))).scale(1.0);
        let mask = bounds(point(px(40.0), px(50.0)), size(px(100.0), px(80.0))).scale(1.0);

        let actual = projection_bounds(element, mask, ProjectionRegion::VisibleContent);
        let expected = bounds(point(px(40.0), px(50.0)), size(px(100.0), px(80.0))).scale(1.0);

        assert_eq!(actual, expected);
    }

    #[test]
    fn non_overlapping_content_mask_produces_empty_projection_bounds() {
        let element = bounds(point(px(0.0), px(0.0)), size(px(20.0), px(20.0))).scale(1.0);
        let mask = bounds(point(px(30.0), px(30.0)), size(px(10.0), px(10.0))).scale(1.0);

        let actual: Bounds<_> = projection_bounds(element, mask, ProjectionRegion::VisibleContent);

        assert!(actual.size.width <= px(0.0).scale(1.0));
        assert!(actual.size.height <= px(0.0).scale(1.0));
    }

    #[test]
    fn visible_square_is_centered_inside_the_mask_intersection() {
        let element = bounds(point(px(10.0), px(20.0)), size(px(300.0), px(200.0))).scale(1.0);
        let mask = bounds(point(px(40.0), px(50.0)), size(px(100.0), px(80.0))).scale(1.0);

        let actual = projection_bounds(element, mask, ProjectionRegion::VisibleSquare);
        let expected = bounds(point(px(50.0), px(50.0)), size(px(80.0), px(80.0))).scale(1.0);

        assert_eq!(actual, expected);
    }

    #[test]
    fn responsive_inset_uses_the_smaller_pixel_or_fraction_value_per_axis() {
        let element = bounds(point(px(10.0), px(20.0)), size(px(300.0), px(200.0))).scale(1.0);

        let actual = inset_bounds(element, 6.0, 0.08);
        let expected = bounds(point(px(16.0), px(26.0)), size(px(288.0), px(188.0))).scale(1.0);

        assert_eq!(actual, expected);
    }

    #[test]
    fn inset_is_applied_to_element_bounds_before_mask_intersection() {
        let element = bounds(point(px(10.0), px(20.0)), size(px(300.0), px(200.0))).scale(1.0);
        let mask = bounds(point(px(0.0), px(0.0)), size(px(40.0), px(50.0))).scale(1.0);
        let inset = inset_bounds(element, 6.0, 0.08);

        let actual = projection_bounds(inset, mask, ProjectionRegion::VisibleContent);
        let expected = bounds(point(px(16.0), px(26.0)), size(px(24.0), px(24.0))).scale(1.0);

        assert_eq!(actual, expected);
    }
}

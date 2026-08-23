use refineable::Refineable as _;

use crate::{
    App, Background, Bounds, Element, ElementId, FillOptions, FillRule, GlobalElementId,
    HitboxBehavior, InspectorElementId, IntoElement, Path, PathBuilder, PathStyle, Pixels, Style,
    StyleRefinement, Styled, Window, point, px, size,
};

const CORNER_HITBOX_STEPS: usize = 8;

/// Creates an overlay element that fills its own layout bounds while leaving one rounded,
/// transparent cutout.
///
/// `cutout` is expressed in coordinates relative to this element's layout origin. The visual mask
/// is tessellated as a single even-odd path, so the cutout and its rounded corners do not require
/// multiple overlapping quads.
///
/// Mouse input is not blocked by default. Use [`RoundedCutout::block_mouse`] or
/// [`RoundedCutout::block_mouse_except_scroll`] when the area outside the cutout should occlude
/// elements behind the overlay while keeping the cutout interactive.
pub fn rounded_cutout(
    cutout: Bounds<Pixels>,
    radius: Pixels,
    background: impl Into<Background>,
) -> RoundedCutout {
    RoundedCutout {
        cutout,
        radius,
        background: Some(background.into()),
        hitbox_behavior: None,
        style: StyleRefinement::default(),
    }
}

/// A reusable rounded cutout overlay.
pub struct RoundedCutout {
    cutout: Bounds<Pixels>,
    radius: Pixels,
    background: Option<Background>,
    hitbox_behavior: Option<HitboxBehavior>,
    style: StyleRefinement,
}

impl RoundedCutout {
    /// Blocks all mouse interaction outside the rounded cutout.
    pub fn block_mouse(mut self) -> Self {
        self.hitbox_behavior = Some(HitboxBehavior::BlockMouse);
        self
    }

    /// Blocks mouse interaction outside the rounded cutout while allowing scroll events through.
    pub fn block_mouse_except_scroll(mut self) -> Self {
        self.hitbox_behavior = Some(HitboxBehavior::BlockMouseExceptScroll);
        self
    }

    /// Configures the hitbox behavior outside the rounded cutout.
    ///
    /// Passing `None` makes this a paint-only overlay.
    pub fn hitbox_behavior(mut self, behavior: Option<HitboxBehavior>) -> Self {
        self.hitbox_behavior = behavior;
        self
    }
}

impl IntoElement for RoundedCutout {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RoundedCutout {
    type RequestLayoutState = Style;
    type PrepaintState = Option<Path<Pixels>>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style.clone(), [], cx);
        (layout_id, style)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Style,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let cutout = resolve_cutout(bounds, self.cutout);
        let radius = clamp_radius(cutout, self.radius);

        if let Some(behavior) = self.hitbox_behavior {
            insert_outside_hitboxes(window, bounds, cutout, radius, behavior);
        }

        build_cutout_path(bounds, cutout, radius).ok()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Style,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let Some(path) = prepaint.take() else {
            return;
        };
        let Some(background) = self.background.take() else {
            return;
        };
        window.paint_path(path, background);
    }
}

impl Styled for RoundedCutout {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

fn resolve_cutout(bounds: Bounds<Pixels>, cutout: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(
        point(
            bounds.origin.x + cutout.origin.x,
            bounds.origin.y + cutout.origin.y,
        ),
        cutout.size,
    )
    .intersect(&bounds)
}

fn clamp_radius(cutout: Bounds<Pixels>, radius: Pixels) -> Pixels {
    px(radius.0.max(0.0).min(cutout.size.width.0 * 0.5).min(cutout.size.height.0 * 0.5))
}

fn build_cutout_path(
    bounds: Bounds<Pixels>,
    cutout: Bounds<Pixels>,
    radius: Pixels,
) -> anyhow::Result<Path<Pixels>> {
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::EvenOdd),
    ));

    add_rect(&mut builder, bounds);
    if !cutout.is_empty() {
        add_rounded_rect(&mut builder, cutout, radius);
    }
    builder.build()
}

fn add_rect(builder: &mut PathBuilder, bounds: Bounds<Pixels>) {
    let left = bounds.origin.x;
    let top = bounds.origin.y;
    let right = left + bounds.size.width;
    let bottom = top + bounds.size.height;

    builder.move_to(point(left, top));
    builder.line_to(point(right, top));
    builder.line_to(point(right, bottom));
    builder.line_to(point(left, bottom));
    builder.close();
}

fn add_rounded_rect(builder: &mut PathBuilder, bounds: Bounds<Pixels>, radius: Pixels) {
    if radius <= px(0.0) {
        add_rect(builder, bounds);
        return;
    }

    let left = bounds.origin.x;
    let top = bounds.origin.y;
    let right = left + bounds.size.width;
    let bottom = top + bounds.size.height;
    let radii = point(radius, radius);

    builder.move_to(point(left + radius, top));
    builder.line_to(point(right - radius, top));
    builder.arc_to(radii, px(0.0), false, true, point(right, top + radius));
    builder.line_to(point(right, bottom - radius));
    builder.arc_to(radii, px(0.0), false, true, point(right - radius, bottom));
    builder.line_to(point(left + radius, bottom));
    builder.arc_to(radii, px(0.0), false, true, point(left, bottom - radius));
    builder.line_to(point(left, top + radius));
    builder.arc_to(radii, px(0.0), false, true, point(left + radius, top));
    builder.close();
}

fn insert_outside_hitboxes(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    cutout: Bounds<Pixels>,
    radius: Pixels,
    behavior: HitboxBehavior,
) {
    if cutout.is_empty() {
        window.insert_hitbox(bounds, behavior);
        return;
    }

    let left = bounds.origin.x;
    let top = bounds.origin.y;
    let right = left + bounds.size.width;
    let bottom = top + bounds.size.height;
    let hole_left = cutout.origin.x;
    let hole_top = cutout.origin.y;
    let hole_right = hole_left + cutout.size.width;
    let hole_bottom = hole_top + cutout.size.height;

    insert_hitbox_if_nonempty(
        window,
        Bounds::new(point(left, top), size(bounds.size.width, hole_top - top)),
        behavior,
    );
    insert_hitbox_if_nonempty(
        window,
        Bounds::new(
            point(left, hole_bottom),
            size(bounds.size.width, bottom - hole_bottom),
        ),
        behavior,
    );
    insert_hitbox_if_nonempty(
        window,
        Bounds::new(
            point(left, hole_top),
            size(hole_left - left, cutout.size.height),
        ),
        behavior,
    );
    insert_hitbox_if_nonempty(
        window,
        Bounds::new(
            point(hole_right, hole_top),
            size(right - hole_right, cutout.size.height),
        ),
        behavior,
    );

    insert_rounded_corner_hitboxes(window, cutout, radius, behavior);
}

fn insert_hitbox_if_nonempty(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    behavior: HitboxBehavior,
) {
    if bounds.size.width > px(0.0) && bounds.size.height > px(0.0) {
        window.insert_hitbox(bounds, behavior);
    }
}

fn insert_rounded_corner_hitboxes(
    window: &mut Window,
    cutout: Bounds<Pixels>,
    radius: Pixels,
    behavior: HitboxBehavior,
) {
    let r = radius.0;
    if r <= 0.0 {
        return;
    }

    let left = cutout.origin.x.0;
    let top = cutout.origin.y.0;
    let right = left + cutout.size.width.0;
    let bottom = top + cutout.size.height.0;
    let band = r / CORNER_HITBOX_STEPS as f32;

    for index in 0..CORNER_HITBOX_STEPS {
        let y0 = index as f32 * band;
        let y1 = (index + 1) as f32 * band;
        let dy = r - y0;
        // Use the widest point in the band. This can block at most one narrow band more than the
        // mathematical arc, which is preferable to leaking clicks through a visually masked pixel.
        let outside_width = r - (r * r - dy * dy).max(0.0).sqrt();
        if outside_width <= 0.0 {
            continue;
        }

        let height = y1 - y0;
        let top_y = top + y0;
        let bottom_y = bottom - y1;

        for (x, y) in [
            (left, top_y),
            (right - outside_width, top_y),
            (left, bottom_y),
            (right - outside_width, bottom_y),
        ] {
            insert_hitbox_if_nonempty(
                window,
                Bounds::new(
                    point(px(x), px(y)),
                    size(px(outside_width), px(height)),
                ),
                behavior,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutout_radius_is_clamped_to_half_of_smallest_axis() {
        let cutout = Bounds::new(point(px(0.0), px(0.0)), size(px(40.0), px(20.0)));
        assert_eq!(clamp_radius(cutout, px(50.0)), px(10.0));
        assert_eq!(clamp_radius(cutout, px(-4.0)), px(0.0));
    }

    #[test]
    fn rounded_even_odd_cutout_path_builds() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(120.0)));
        let cutout = Bounds::new(point(px(40.0), px(30.0)), size(px(80.0), px(50.0)));
        assert!(build_cutout_path(bounds, cutout, px(12.0)).is_ok());
    }
}

use crate::{Bounds, ScaledPixels, Scene};

const MAX_DIRTY_RECTS: usize = 128;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub(crate) struct RequestFrameOptions {
    pub(crate) require_presentation: bool,
    pub(crate) force_render: bool,
}

impl RequestFrameOptions {
    pub(crate) fn from_refresh() -> Self {
        Self {
            require_presentation: false,
            force_render: true,
        }
    }

    pub(crate) fn requires_frame(self) -> bool {
        self.require_presentation || self.force_render
    }

    pub(crate) fn merge(self, options: Self) -> Self {
        Self {
            require_presentation: self.require_presentation || options.require_presentation,
            force_render: self.force_render || options.force_render,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DirtyRect {
    pub(crate) bounds: Bounds<ScaledPixels>,
}

impl DirtyRect {
    pub(crate) fn new(bounds: Bounds<ScaledPixels>) -> Option<Self> {
        (!bounds.is_empty()).then_some(Self { bounds })
    }

    pub(crate) fn area(&self) -> f32 {
        f64::from(self.bounds.size.width) as f32 * f64::from(self.bounds.size.height) as f32
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DirtyRegion {
    rects: Vec<DirtyRect>,
    full: bool,
}

impl DirtyRegion {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub(crate) fn full(bounds: Bounds<ScaledPixels>) -> Self {
        let mut region = Self {
            rects: Vec::new(),
            full: true,
        };
        region.push(bounds);
        region
    }

    pub(crate) fn push(&mut self, bounds: Bounds<ScaledPixels>) {
        let Some(mut rect) = DirtyRect::new(bounds) else {
            return;
        };

        let mut index = 0;
        while index < self.rects.len() {
            if self.rects[index].bounds.intersects(&rect.bounds) {
                let existing = self.rects.swap_remove(index);
                rect.bounds = rect.bounds.union(&existing.bounds);
                index = 0;
            } else {
                index += 1;
            }
        }
        self.rects.push(rect);

        if self.rects.len() > MAX_DIRTY_RECTS
            && let Some(bounds) = self.union_bounds()
        {
            self.rects.clear();
            self.rects.push(DirtyRect { bounds });
        }
    }

    pub(crate) fn mark_full(&mut self, bounds: Bounds<ScaledPixels>) {
        self.full = true;
        self.rects.clear();
        self.push(bounds);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub(crate) fn is_full(&self) -> bool {
        self.full
    }

    pub(crate) fn rects(&self) -> &[DirtyRect] {
        &self.rects
    }

    pub(crate) fn rect_count(&self) -> usize {
        self.rects.len()
    }

    pub(crate) fn union_bounds(&self) -> Option<Bounds<ScaledPixels>> {
        self.rects
            .iter()
            .map(|rect| rect.bounds)
            .reduce(|bounds, rect| bounds.union(&rect))
    }

    pub(crate) fn area(&self) -> f32 {
        self.rects.iter().map(DirtyRect::area).sum()
    }

    pub(crate) fn coalesce_if_large(
        &mut self,
        viewport: Bounds<ScaledPixels>,
        max_partial_area_ratio: f32,
    ) {
        if self.full || self.rects.is_empty() {
            return;
        }

        let viewport_area =
            f64::from(viewport.size.width) as f32 * f64::from(viewport.size.height) as f32;
        if viewport_area <= 0.0 || self.area() <= viewport_area * max_partial_area_ratio {
            return;
        }

        self.mark_full(viewport);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, bounds, size};

    fn rect(x: f32, width: f32) -> Bounds<ScaledPixels> {
        bounds(
            Point {
                x: ScaledPixels(x),
                y: ScaledPixels(0.0),
            },
            size(ScaledPixels(width), ScaledPixels(10.0)),
        )
    }

    #[test]
    fn merges_transitively_connected_damage() {
        let mut region = DirtyRegion::empty();
        region.push(rect(0.0, 10.0));
        region.push(rect(20.0, 10.0));
        region.push(rect(5.0, 20.0));

        assert_eq!(region.rect_count(), 1);
        assert_eq!(region.union_bounds(), Some(rect(0.0, 30.0)));
    }

    #[test]
    fn bounds_fragmented_damage_metadata() {
        let mut region = DirtyRegion::empty();
        for index in 0..=MAX_DIRTY_RECTS {
            region.push(rect(index as f32 * 20.0, 5.0));
        }

        assert_eq!(region.rect_count(), 1);
        assert_eq!(
            region.union_bounds(),
            Some(rect(0.0, MAX_DIRTY_RECTS as f32 * 20.0 + 5.0))
        );
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PartialPresentMode {
    #[default]
    FullRedraw,
    Partial,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RetainedResourceTrimPolicy {
    #[default]
    None,
    Light,
    Strong,
}

#[derive(Clone, Copy)]
pub(crate) struct FrameRenderPlan<'a> {
    pub(crate) scene: &'a Scene,
    pub(crate) dirty_region: &'a DirtyRegion,
    pub(crate) partial_present_mode: PartialPresentMode,
    pub(crate) trim_policy: RetainedResourceTrimPolicy,
    pub(crate) backdrop_blur_refresh_required: bool,
}

impl<'a> FrameRenderPlan<'a> {
    #[cfg(target_os = "macos")]
    pub(crate) fn full_redraw(scene: &'a Scene, dirty_region: &'a DirtyRegion) -> Self {
        Self {
            scene,
            dirty_region,
            partial_present_mode: PartialPresentMode::FullRedraw,
            trim_policy: RetainedResourceTrimPolicy::None,
            backdrop_blur_refresh_required: true,
        }
    }

    pub(crate) fn surface_requires_full_redraw(self) -> Self {
        Self {
            partial_present_mode: PartialPresentMode::FullRedraw,
            backdrop_blur_refresh_required: true,
            ..self
        }
    }
}

//! The single immutable publication shared by UI history, the owner queue and rendering.
use super::*;

/// Geometry provenance belongs to the Paint generation, not the current native drawable.
#[derive(Clone, Copy)]
pub(crate) struct PaintGeometry {
    pub(crate) layout_extent: Option<(u32, u32)>,
    pub(crate) layout_epoch: u64,
    pub(crate) scale_factor: f32,
}

impl Default for PaintGeometry {
    fn default() -> Self {
        Self {
            layout_extent: None,
            layout_epoch: 0,
            scale_factor: 1.0,
        }
    }
}

/// One authoritative Paint payload and its paired CPU retention proofs.
/// All consumers share this publication; there is no second drawing stream or resource registry.
#[derive(Default)]
pub(crate) struct Paint {
    pub(crate) scene: Scene,
    pub(crate) retained_paint: Arc<crate::window::retained::RetainedPaint>,
    pub(crate) geometry: PaintGeometry,
}

impl Paint {
    pub(crate) fn new(
        scene: Scene,
        retained_paint: Arc<crate::window::retained::RetainedPaint>,
        geometry: PaintGeometry,
    ) -> Self {
        Self {
            scene,
            retained_paint,
            geometry,
        }
    }
}

impl std::ops::Deref for Paint {
    type Target = Scene;
    fn deref(&self) -> &Scene {
        &self.scene
    }
}

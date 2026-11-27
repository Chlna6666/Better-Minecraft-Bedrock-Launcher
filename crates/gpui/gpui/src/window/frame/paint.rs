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
    pub(crate) layers: SceneLayers,
    // Created only after an owner-side raster miss. The original logical
    // publication stays authoritative for UI replay and subsequent retries.
    materialized_scroll_root: std::sync::OnceLock<Arc<Paint>>,
    fallback_atlas_tiles: Box<[(crate::AtlasTextureId, u32)]>,
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
            layers: SceneLayers::default(),
            materialized_scroll_root: std::sync::OnceLock::new(),
            fallback_atlas_tiles: Box::default(),
        }
    }

    /// Create a new GPU-visible immutable Paint while the committed UI
    /// frame keeps ownership of its existing input and layout records.
    pub(in crate::window) fn with_scroll_placement(
        &self, index: usize, placement: crate::window::scroll_paint::ScrollPlacement,
        window_opaque: bool,
    ) -> Option<Arc<Self>> {
        let scene = self.scene.with_scroll_placement(index, placement.clone())?;
        let retention = Arc::new(self.retained_paint.placement_variant(&self.scene, &scene)?);
        let mut result = Self::new(scene, retention, self.geometry);
        result.layers = self.layers.with_scroll_placement(index, &placement, window_opaque)
            .unwrap_or_else(|| SceneLayers::capture_against(
                &result.scene, Some((&self.scene, &self.layers)), window_opaque,
            ));
        Some(Arc::new(result))
    }

    /// Owner-side fallback reconstruction is intentionally independent of
    /// UI history. The materialized Scene has new operation indices and
    /// therefore receives fresh retention metadata.
    fn materialize_deferred_scroll(&self) -> Option<Self> {
        if !self.scene.paint_operations.range_is_replayable(0..self.scene.len()) {
            return None;
        }
        let mut painter = crate::scene::Painter::default();
        painter.replay(0..self.scene.len(), &self.scene, std::iter::empty());
        painter.finish();
        let scene = painter.into_scene();
        let mut materialized = Self::new(scene, Arc::default(), self.geometry);
        // The expanded visible Scene is not the complete liveness proof:
        // a retry or a later scroll may still need the original overscan.
        let mut live = collections::FxHashSet::default();
        self.collect_atlas_tile_ids_into(&mut live);
        materialized.fallback_atlas_tiles = live.into_iter().collect();
        Some(materialized)
    }

    pub(crate) fn collect_atlas_tile_ids_into(
        &self,
        live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>,
    ) {
        self.scene.collect_atlas_tile_ids_into(live);
        self.layers.collect_atlas_tile_ids_into(live);
        live.extend(self.fallback_atlas_tiles.iter().copied());
    }

    /// Static image eviction needs polychrome sprites and frozen overscan
    /// dependencies, not the root monochrome glyph stream.
    pub(in crate::window) fn collect_image_tile_ids_into(
        &self,
        live: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>,
    ) {
        self.scene.collect_polychrome_tile_ids_into(live);
        self.layers.collect_atlas_tile_ids_into(live);
        live.extend(self.fallback_atlas_tiles.iter().copied());
    }

    /// Ordinary Paint has no deferred work. Only a real deferred publication
    /// allocates a materialized root, and subsequent owner retries reuse it.
    pub(crate) fn owner_fallback(&self) -> crate::Result<Option<Arc<Self>>> {
        if !self.scene.paint_operations.has_deferred_scroll() {
            return Ok(None);
        }
        if let Some(materialized) = self.materialized_scroll_root.get() {
            return Ok(Some(Arc::clone(materialized)));
        }
        let materialized = self.materialize_deferred_scroll()
            .ok_or_else(|| anyhow::anyhow!("cannot materialize deferred scroll root"))?;
        Ok(Some(Arc::clone(self.materialized_scroll_root.get_or_init(|| Arc::new(materialized)))))
    }
}

impl std::ops::Deref for Paint {
    type Target = Scene;
    fn deref(&self) -> &Scene {
        &self.scene
    }
}

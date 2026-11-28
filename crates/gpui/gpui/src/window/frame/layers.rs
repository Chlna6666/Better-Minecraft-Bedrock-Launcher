//! Immutable scroll content publication, adapted from GPUI Fast's LayerContent.
use crate::{Bounds, ContentMask, Point, Rgba, ScaledPixels, TransformationMatrix, point, scene::{Operations, PaintOperationRef, PrimitiveRef, Scene}};
#[cfg(test)]
use crate::Pixels;
use collections::{FxHashMap, FxHashSet};
use std::{ops::Range, sync::{Arc, atomic::{AtomicU64, Ordering}}};

static NEXT_LAYER_ID: AtomicU64 = AtomicU64::new(1);

pub(in crate::window) fn next_layer_id() -> u64 {
    NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) struct LayerContent {
    // Reuses the original immutable operation pages instead of copying a
    // Scene per row. This payload never retains the enclosing Scene itself.
    operations: Arc<Operations>,
    raster: ContentRaster,
    // Immutable logical glyph identities collected once with ScrollPaint.
    raster_atlas_tiles: Arc<[(crate::AtlasTextureId, u32)]>,
}

impl LayerContent {
    fn from_scroll(
        operations: &Arc<Operations>,
        source_coverage: Bounds<ScaledPixels>,
        delta: Point<ScaledPixels>,
        translation: [i32; 2],
        source_viewport: ContentMask<ScaledPixels>,
        atlas_tiles: &Arc<[(crate::AtlasTextureId, u32)]>,
        previous: Option<&Arc<Self>>,
    ) -> Option<Arc<Self>> {
        let [x, y] = translation;
        // Capture coordinates = stable content coordinates + source origin.
        // The viewport moved by delta since this capture was painted.
        let origin = translation_point([x, y]) - delta;
        let integral = |v: ScaledPixels| {
            (v.0.is_finite() && v.0.fract() == 0. && v.0.abs() <= 1_048_576.)
                .then_some(v.0 as i32)
        };
        let source_origin = [integral(origin.x)?, integral(origin.y)?];
        let coverage = translated_bounds(source_coverage, Point::default() - origin);
        let visible = LayerFrame::coverage(source_viewport, translation);
        if coverage.intersect(&visible) != visible
            || source_viewport.corner_radii != Default::default()
        {
            return None;
        }
        let raster = ContentRaster { source_origin, coverage };
        // Placement/viewport changes do not change the frozen source.
        // Validate the new viewport above, then reuse the exact source
        // allocation even when the clipping rectangle has changed.
        if let Some(previous) = previous
            && previous.raster == raster
            && Arc::ptr_eq(&previous.operations, operations)
            && Arc::ptr_eq(&previous.raster_atlas_tiles, atlas_tiles)
        {
            return Some(Arc::clone(previous));
        }
        Some(Arc::new(Self {
            operations: Arc::clone(operations),
            raster,
            raster_atlas_tiles: Arc::clone(atlas_tiles),
        }))
    }

    // Native captures use a common content anchor but can paint at
    // different window origins. Compare the ordered records after moving
    // old geometry into the new capture coordinate system. The two
    // capture masks agree in their common coverage. Nested masks can
    // render correctly but require a full refresh on recapture.
    fn native_damage_from(&self, previous: &Self) -> LayerDamage {
        let (current, old) = (self.raster, previous.raster);
        if self.operations.len() != previous.operations.len() {
            return LayerDamage::Full;
        }
        let current_origin = translation_point(current.source_origin);
        let old_origin = translation_point(old.source_origin);
        let delta = current_origin - old_origin;
        let current_mask = ContentMask::new(translated_bounds(current.coverage, current_origin));
        let old_mask = ContentMask::new(translated_bounds(old.coverage, old_origin));
        let mut damage = None;
        for (current_op, old_op) in self.operations.views().zip(previous.operations.views()) {
            match (current_op, old_op) {
                (PaintOperationRef::StartLayer(a), PaintOperationRef::StartLayer(b))
                    if *a == translated_bounds(*b, delta) => {}
                (PaintOperationRef::EndLayer, PaintOperationRef::EndLayer) => {}
                (PaintOperationRef::Primitive(a), PaintOperationRef::Primitive(b)) => {
                    let (Some(a_bounds), Some(b_bounds)) = (static_raster_bounds(a), static_raster_bounds(b))
                        else { return LayerDamage::Full; };
                    if *a.content_mask() != current_mask || *b.content_mask() != old_mask {
                        return LayerDamage::Full;
                    }
                    macro_rules! equal {
                        ($a:ident, $b:ident) => {{
                            let mut moved = *$b;
                            moved.bounds = translated_bounds(moved.bounds, delta);
                            moved.content_mask = current_mask;
                            *$a == moved
                        }};
                    }
                    let equal = match (a, b) {
                        (PrimitiveRef::Quad(a), PrimitiveRef::Quad(b)) => equal!(a, b),
                        (PrimitiveRef::Shadow(a), PrimitiveRef::Shadow(b)) => equal!(a, b),
                        (PrimitiveRef::Underline(a), PrimitiveRef::Underline(b)) => equal!(a, b),
                        (PrimitiveRef::MonochromeSprite(a), PrimitiveRef::MonochromeSprite(b)) => equal!(a, b),
                        (PrimitiveRef::PolychromeSprite(a), PrimitiveRef::PolychromeSprite(b)) => equal!(a, b),
                        _ => return LayerDamage::Full,
                    };
                    if !equal {
                        let changed = translated_bounds(a_bounds, Point::default() - current_origin)
                            .union(&translated_bounds(b_bounds, Point::default() - old_origin));
                        union_visual(&mut damage, changed);
                    }
                }
                _ => return LayerDamage::Full,
            }
        }
        damage.map_or(LayerDamage::Clean, LayerDamage::Bounds)
    }


}

/// Rasterization uses the source's original coordinates; only the signed
/// RasterTile origin changes. No UI-side translation or new primitive stream.
#[derive(Clone, Copy, PartialEq)]
struct ContentRaster {
    source_origin: [i32; 2],
    coverage: Bounds<ScaledPixels>,
}

impl ContentRaster {
    fn source_origin_for_tile(self, rect: [i64; 4]) -> Option<[i32; 2]> {
        let [left, top, right, bottom] = rect;
        if left >= right || top >= bottom
            || (left as f64) < f64::from(self.coverage.left().0)
            || (top as f64) < f64::from(self.coverage.top().0)
            || right as f64 > f64::from(self.coverage.right().0)
            || bottom as f64 > f64::from(self.coverage.bottom().0)
        {
            return None;
        }
        Some([
            i32::try_from(left.checked_add(i64::from(self.source_origin[0]))?).ok()?,
            i32::try_from(top.checked_add(i64::from(self.source_origin[1]))?).ok()?,
        ])
    }
}

// GPU raster coordinates are stable content pixels, with an integral
// device-pixel translation into the original captured window coordinates.
// Fractional positions and lists with unknown row heights have no proof.
fn translation_point(translation: [i32; 2]) -> Point<ScaledPixels> {
    point(ScaledPixels(translation[0] as f32), ScaledPixels(translation[1] as f32))
}

fn translated_bounds(mut bounds: Bounds<ScaledPixels>, delta: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    bounds.origin = bounds.origin + delta;
    bounds
}

#[derive(Clone, Copy)]
enum LayerDamage {
    Full,
    Clean,
    Bounds(Bounds<ScaledPixels>),
}

fn static_raster_bounds(primitive: PrimitiveRef<'_>) -> Option<Bounds<ScaledPixels>> {
    if primitive.animation_id().is_some() {
        return None;
    }
    match primitive {
        PrimitiveRef::Quad(_) | PrimitiveRef::Shadow(_) | PrimitiveRef::Underline(_) => {}
        PrimitiveRef::PolychromeSprite(sprite) if sprite.sampling != 2
            && matches!(sprite.tile.texture_id.kind, crate::AtlasTextureKind::Bgra | crate::AtlasTextureKind::Rgba) => {}
        PrimitiveRef::MonochromeSprite(sprite)
            if sprite.transformation == TransformationMatrix::unit()
                && (sprite.tile.texture_id.kind == crate::AtlasTextureKind::Monochrome
                    || (cfg!(target_os = "windows")
                        && sprite.tile.texture_id.kind == crate::AtlasTextureKind::Subpixel)) => {}
        _ => return None,
    }
    // Include the antialiased boundary; never claim a clean neighbor from
    // a fractional geometry edge or filter footprint.
    let bounds = primitive.visual_bounds().dilate(ScaledPixels(2.));
    [bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0]
        .into_iter().all(f32::is_finite).then_some(bounds)
}

// Rectangular local clips stay in the captured primitive. The raster source
// is bounded by the capture and uses its original coordinates.
// Effects and transformed glyphs keep the ordinary path. LCD coverage
// requires the opaque underlay proof at publication.
pub(in crate::window) fn static_scroll_raster_operation(
    operation: PaintOperationRef<'_>,
    coverage: Bounds<ScaledPixels>,
) -> bool {
    match operation {
        PaintOperationRef::StartLayer(bounds) => [
            bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0,
        ].into_iter().all(f32::is_finite),
        PaintOperationRef::EndLayer => true,
        PaintOperationRef::Primitive(primitive) => {
            contains_visual(coverage, primitive.content_mask().bounds)
                && primitive.content_mask().corner_radii == Default::default()
                && static_raster_bounds(primitive).is_some()
        }
        _ => false,
    }
}

#[derive(Clone)]
pub(crate) struct LayerFrame {
    /// Raster identity, stable while the capture and its underlay are valid.
    pub(crate) id: u64,
    source_id: u64,
    background: Option<Rgba>,
    pub(crate) generation: u64,
    /// Exact original operation span, certified against native root order.
    /// The owner derives complementary intervals for the selected layers.
    static_root_span: Option<Range<usize>>,
    damage: LayerDamage,
    /// Certified coverage shared with the immediately previous generation,
    /// in stable content-space device pixels.
    previous_coverage: Option<Bounds<ScaledPixels>>,
    translation: [i32; 2],
    source_viewport: ContentMask<ScaledPixels>,
    scale: f32,
    pub(crate) content: Arc<LayerContent>,
    row_raster: Option<Bounds<ScaledPixels>>,
}

impl LayerFrame {
    fn coverage(viewport: ContentMask<ScaledPixels>, translation: [i32; 2]) -> Bounds<ScaledPixels> {
        let [x, y] = translation;
        translated_bounds(viewport.bounds, translation_point([-x, -y]))
    }

    /// A static quad root may contain ordinary drawing around this layer.
    /// The certified outside ranges retain their original paint order.
    pub(crate) fn root_static_raster(&self) -> Option<(Bounds<ScaledPixels>, [i32; 2])> {
        self.static_root_span.as_ref()?;
        Some((self.source_viewport.bounds, self.translation))
    }

    pub(crate) fn native_row_raster(&self) -> Option<Bounds<ScaledPixels>> {
        self.row_raster
    }

    pub(crate) fn raster_background(&self) -> Option<Rgba> {
        self.background
    }

    pub(crate) fn raster_atlas_tiles(&self) -> &[(crate::AtlasTextureId, u32)] {
        &self.content.raster_atlas_tiles
    }

    pub(crate) fn root_static_span(&self) -> Option<&Range<usize>> {
        self.static_root_span.as_ref()
    }

    /// Borrow the original capture, along with the signed projection origin
    /// for this content-space tile. The cache key itself must remain in
    /// content coordinates. The renderer can compile this source on a miss;
    /// a resident hit need not inspect a single primitive.
    pub(crate) fn raster_source_for_tile(
        &self, rect: [i64; 4],
    ) -> Option<(&Operations, [i32; 2])> {
        let origin = self.content.raster.source_origin_for_tile(rect)?;
        Some((&self.content.operations, origin))
    }

    /// Test changed logical atlas IDs against the exact tile footprint in
    /// capture coordinates. Called only for affected layer sources on an
    /// atlas damage epoch, never on the usual resident-hit path.
    pub(crate) fn raster_tile_uses_atlas_damage(&self, rect: [i64; 4], dirty: &FxHashSet<u32>) -> bool {
        let Some((operations, origin)) = self.raster_source_for_tile(rect) else { return true; };
        let Some(width) = rect[2].checked_sub(rect[0]) else { return true; };
        let Some(height) = rect[3].checked_sub(rect[1]) else { return true; };
        // The direct raster admission uses bounded integral f32 coordinates.
        // Do not certify an unrelated tile with lossy coordinate arithmetic.
        if width <= 0 || height <= 0 || width > 2_097_152 || height > 2_097_152
            || origin.iter().any(|&v| i64::from(v).abs() > 1_048_576)
        {
            return true;
        }
        let region = Bounds::new(point(ScaledPixels(origin[0] as f32), ScaledPixels(origin[1] as f32)),
            crate::size(ScaledPixels(width as f32), ScaledPixels(height as f32)));
        operations.views().any(|operation| {
            let PaintOperationRef::Primitive(primitive) = operation else {
                return !matches!(operation, PaintOperationRef::StartLayer(_) | PaintOperationRef::EndLayer);
            };
            let id = match primitive {
                PrimitiveRef::MonochromeSprite(sprite) => sprite.tile.tile_id.0,
                PrimitiveRef::PolychromeSprite(sprite) => sprite.tile.tile_id.0,
                _ => return false,
            };
            dirty.contains(&id) && static_raster_bounds(primitive)
                .is_none_or(|bounds| bounds.intersects(&region))
        })
    }

    /// Source identity remains stable during translation. Only an adjacent
    /// generation can inherit a tile after the source is repainted.
    pub(crate) fn adopt_tile(&self, generation: u64, rect: [i64; 4], scale: f32) -> Option<u64> {

        if scale != self.scale {
            return None;
        }
        self.raster_source_for_tile(rect)?;
        if generation == self.generation {
            return Some(generation);
        }
        if generation.checked_add(1) != Some(self.generation) {
            return None;
        }
        let bounds = self.previous_coverage?;
        let [left, top, right, bottom] = rect;
        // Whole tile coverage in BOTH captures, including the boundary.
        if (left as f64) < f64::from(bounds.left().0)
            || (top as f64) < f64::from(bounds.top().0)
            || right as f64 > f64::from(bounds.right().0)
            || bottom as f64 > f64::from(bounds.bottom().0)
        {
            return None;
        }
        let clean = match self.damage {
            LayerDamage::Full => false,
            LayerDamage::Clean => true,
            LayerDamage::Bounds(bounds) => f64::from(bounds.right().0) <= left as f64
                || f64::from(bounds.bottom().0) <= top as f64
                || f64::from(bounds.left().0) >= right as f64
                || f64::from(bounds.top().0) >= bottom as f64,
        };
        clean.then_some(self.generation)
    }
}

/// Prove raw painter order without relying on the provisional DrawOrder of
/// an unexpanded root. A nested scope receives a different native order
/// while its parent's suffix retains the parent's earlier order. Admit
/// such scopes only when their pixels commute with every sibling.
#[derive(Default)]
pub(in crate::window) struct StaticPaintOrder {
    scopes: smallvec::SmallVec<[ScopeOrder; 4]>,
    root_extension: bool,
    last_extension: Option<Bounds<ScaledPixels>>,
}

struct ScopeOrder {
    bounds: Bounds<ScaledPixels>,
    visual: Option<Bounds<ScaledPixels>>,
    isolated: Option<Bounds<ScaledPixels>>,
    kind: Option<u8>,
    unstable_bounds: Option<Bounds<ScaledPixels>>,
}

fn union_visual(target: &mut Option<Bounds<ScaledPixels>>, bounds: Bounds<ScaledPixels>) {
    if !bounds.is_empty() {
        *target = Some(target.map_or(bounds, |old| old.union(&bounds)));
    }
}

fn contains_visual(bounds: Bounds<ScaledPixels>, visual: Bounds<ScaledPixels>) -> bool {
    visual.is_empty() || bounds.intersect(&visual) == visual
}

impl ScopeOrder {
    fn append_isolated(&mut self, visual: Bounds<ScaledPixels>) -> bool {
        if !contains_visual(self.bounds, visual)
            || self.visual.is_some_and(|old| old.intersects(&visual))
        {
            return false;
        }
        union_visual(&mut self.visual, visual);
        union_visual(&mut self.isolated, visual);
        true
    }
}

impl StaticPaintOrder {
    pub(in crate::window) fn root() -> Self {
        Self { root_extension: true, ..Self::default() }
    }

    pub(in crate::window) fn accept(&mut self, operation: PaintOperationRef<'_>) -> bool {
        match operation {
            PaintOperationRef::StartLayer(bounds) => {
                if ![bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0].into_iter().all(f32::is_finite) {
                    return false;
                }
                self.scopes.push(ScopeOrder { bounds: *bounds, visual: None, isolated: None,
                    kind: None, unstable_bounds: None });
                true
            }
            PaintOperationRef::EndLayer => {
                let Some(scope) = self.scopes.pop() else { return false; };
                if let (Some(parent), Some(visual)) = (self.scopes.last_mut(), scope.visual) {
                    parent.append_isolated(visual)
                } else { true }
            }
            PaintOperationRef::DeferredScroll(placement) => {
                if !placement.is_visible() { return true; }
                if !placement.has_static_order() { return false; }
                if let Some(scope) = self.scopes.last_mut() {
                    // Replaying under an inherited layer can collapse the
                    // source's independent primitive orders into one order.
                    placement.has_scoped_order() && scope.append_isolated(placement.visual_bounds())
                } else { true }
            }
            PaintOperationRef::Primitive(PrimitiveRef::RendererExtension(extension)) => {
                // Native extensions are prepared in sorted DrawOrder, not
                // necessarily raw paint order. A chain of intersecting
                // clipped ordering rectangles proves strictly increasing
                // native orders even with unexpanded scroll markers.
                if !self.root_extension || !self.scopes.is_empty()
                    || extension.animation_id.is_some()
                    || ![extension.bounds, extension.content_mask.bounds].into_iter().all(|bounds|
                        [bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0]
                            .into_iter().all(|v| v.is_finite() && v.fract() == 0.))
                {
                    return false;
                }
                let bounds = extension.bounds.intersect(&extension.content_mask.bounds);
                if bounds.is_empty() { return false; }
                if self.last_extension.is_some_and(|previous| !previous.intersects(&bounds)) {
                    return false;
                }
                self.last_extension = Some(bounds);
                true
            }
            PaintOperationRef::Primitive(primitive) => {
                if static_raster_bounds(primitive).is_none() {
                    return false;
                }
                // A shadow fringe outside its native ordering bounds
                // cannot be certified by this geometric proof.
                let kind = match primitive {
                    PrimitiveRef::Shadow(shadow) if shadow.blur_radius.0 == 0. => 0,
                    PrimitiveRef::Quad(_) => 1,
                    PrimitiveRef::Underline(_) => 3,
                    PrimitiveRef::MonochromeSprite(_) => 4,
                    PrimitiveRef::PolychromeSprite(_) => 5,
                    _ => return false,
                };
                let visual = primitive.visual_bounds();
                if ![visual.left().0, visual.top().0, visual.right().0, visual.bottom().0].into_iter().all(f32::is_finite) {
                    return false;
                }
                let ordering = self.scopes.last().map_or_else(
                    || primitive.bounds().intersect(&primitive.content_mask().bounds), |scope| scope.bounds);
                if !contains_visual(ordering, visual) { return false; }
                if let Some(scope) = self.scopes.last_mut() {
                    if scope.isolated.is_some_and(|old| old.intersects(&visual))
                        || scope.kind.is_some_and(|old| old > kind)
                    {
                        return false;
                    }
                    if scope.kind != Some(kind) { scope.unstable_bounds = None; }
                    // Equal-kind unstable streams can be reordered by
                    // texture/sampling changes. Only disjoint pixels commute.
                    if kind >= 3 && !visual.is_empty() {
                        if scope.unstable_bounds.is_some_and(|old| old.intersects(&visual)) { return false; }
                        union_visual(&mut scope.unstable_bounds, visual);
                    }
                    union_visual(&mut scope.visual, visual);
                    scope.kind = Some(kind);
                }
                true
            }
            _ => false,
        }
    }

    pub(in crate::window) fn complete(&self) -> bool { self.scopes.is_empty() }
}

// A raw paint-order split is valid only if the native quad stream has
// that same order. Scope markers themselves draw nothing, but inherited
// DrawOrder can reorder nested layers. Reject inversions rather than
// guessing which overlapping primitives can safely commute.
fn root_order_eligible(scene: &Scene) -> bool {
    scene.animation_values.is_empty() && scene.blurs.is_empty() && scene.backdrop_blurs.is_empty()
}

fn ordered_static_root(scene: &Scene) -> bool {
    if !root_order_eligible(scene) {
        return false;
    }
    if scene.paint_operations.has_deferred_scroll() {
        let mut proof = StaticPaintOrder::root();
        return scene.paint_operations.views().all(|operation| proof.accept(operation)) && proof.complete();
    }
    if !scene.renderer_extensions.is_empty() { return false; }
    let mut previous = None;
    for operation in scene.paint_operations.views() {
        match operation {
            PaintOperationRef::Primitive(primitive) => {
                let order = match primitive {
                    PrimitiveRef::Shadow(_) => (primitive.order(), 0, 0, 0, 0),
                    PrimitiveRef::Quad(_) => (primitive.order(), 1, 0, 0, 0),
                    PrimitiveRef::Underline(_) => (primitive.order(), 3, 0, 0, 0),
                    PrimitiveRef::MonochromeSprite(sprite)
                        if sprite.tile.texture_id.kind == crate::AtlasTextureKind::Monochrome =>
                        (primitive.order(), 4, sprite.tile.texture_id.kind as u32, sprite.tile.texture_id.index, sprite.sampling() as u32),
                    PrimitiveRef::PolychromeSprite(sprite) =>
                        (primitive.order(), 5, sprite.tile.texture_id.kind as u32, sprite.tile.texture_id.index, 0),
                    _ => return false,
                };
                if previous.is_some_and(|last| last > order) || static_raster_bounds(primitive).is_none() {
                    return false;
                }
                previous = Some(order);
            }
            PaintOperationRef::StartLayer(_) | PaintOperationRef::EndLayer => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
fn static_root_span(scene: &Scene, range: Range<usize>) -> Option<Range<usize>> {
    (!range.is_empty() && range.end <= scene.len()).then_some(range)
}

// The order proof depends on static geometry and scope order, not on the
// scroll source's generation or its content-space translation. Shared
// immutable operation pages already prove every input in those pages.
fn root_order_inputs_match(current: &Scene, previous: &Scene, flat_root: bool) -> bool {
    root_order_eligible(current) && current.paint_operations.all().matches_views(
        previous.paint_operations.all(),
        |current, previous| match (current, previous) {
            (PaintOperationRef::DeferredScroll(a), PaintOperationRef::DeferredScroll(b)) => {
                a.is_visible() == b.is_visible()
                    && a.has_static_order() == b.has_static_order()
                    && (flat_root || (a.has_scoped_order() == b.has_scoped_order()
                        && a.visual_bounds() == b.visual_bounds()))
            }
            _ => current.same_paint(previous),
        },
    )
}

#[derive(Default)]
pub(crate) struct SceneLayers {
    pub(crate) frames: Vec<LayerFrame>,
    // A positive proof belongs only to this immutable root publication.
    ordered: bool,
    flat_root: bool,
    window_opaque: bool,
}

impl SceneLayers {
    /// Protect frozen overscan glyphs as well as the visible Scene. This
    /// visits only unique logical dependencies, never the source operations.
    pub(crate) fn collect_atlas_tile_ids_into(&self, live: &mut FxHashSet<(crate::AtlasTextureId, u32)>) {
        for frame in &self.frames {
            live.extend(frame.content.raster_atlas_tiles.iter().copied());
        }
    }

    // Native GPU tests use the production admission and coordinate mapping
    // with a frozen ScrollPaint-style operation page.
    #[cfg(test)]
    pub(crate) fn raster_fixture(
        &mut self,
        operations: Operations,
        coverage: Bounds<ScaledPixels>,
        viewport: ContentMask<Pixels>,
        translation: [i32; 2],
    ) {
        let mut operations = operations;
        operations.finish();
        assert!(operations.range_is_replayable(0..operations.len()));
        assert!(operations.views().all(|op| static_scroll_raster_operation(op, coverage)));
        let atlas_tiles: Arc<[_]> = operations.views().filter_map(|op| match op {
            PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(sprite)) => Some((sprite.tile.texture_id, sprite.tile.tile_id.0)),
            PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(sprite)) => Some((sprite.tile.texture_id, sprite.tile.tile_id.0)),
            _ => None,
        }).collect::<FxHashSet<_>>().into_iter().collect::<Vec<_>>().into();
        let operations = Arc::new(operations);
        let content = LayerContent::from_scroll(
            &operations, coverage, Point::default(), translation, viewport.scale(1.), &atlas_tiles, None,
        ).expect("fixture source coverage");
        assert!(Arc::ptr_eq(&operations, &content.operations));
        let id = self.frames.len() as u64 + 1;
        self.frames.push(LayerFrame {
            id, source_id: id, generation: 1, background: None,
            static_root_span: Some(0..0),
            damage: LayerDamage::Full, previous_coverage: None,
            translation, source_viewport: viewport.scale(1.),
            scale: 1.,
            content,
            row_raster: None,
        });
    }

    #[cfg(test)]
    pub(crate) fn certify_fixture_root(&mut self, scene: &Scene, range: Range<usize>) {
        self.frames.last_mut().expect("raster fixture").static_root_span = ordered_static_root(scene)
            .then(|| static_root_span(scene, range)).flatten();
    }

    #[cfg(test)]
    pub(crate) fn capture(scene: &Scene, previous: Option<&Self>, window_opaque: bool) -> Self {
        Self::capture_layers(scene, previous, window_opaque, ordered_static_root(scene),
            !scene.paint_operations.has_layer_scopes())
    }

    /// The caller has certified that precisely one marker's translation
    /// changed, with identical source, visual bounds and operation order.
    /// Root order and LCD underlays depend on marker bounds. With bounds
    /// and window opacity unchanged, sibling layer proofs still hold.
    #[cfg(test)]
    pub(in crate::window) fn with_scroll_placement(
        &self,
        index: usize,
        placement: &crate::window::scroll_paint::ScrollPlacement,
        window_opaque: bool,
    ) -> Option<Self> {
        self.with_scroll_placements(&[(index, placement.clone())], window_opaque)
    }

    #[cfg(test)]
    pub(in crate::window) fn with_scroll_placements(
        &self,
        placements: &[(usize, crate::window::scroll_paint::ScrollPlacement)],
        window_opaque: bool,
    ) -> Option<Self> {
        self.with_scroll_placements_checked(placements, window_opaque, false)
    }

    /// The scene has already certified unchanged marker identity and
    /// topology. Moving bounds preserve the flat-root order proof, but
    /// scoped isolation and LCD underlays need fresh geometry validation.
    pub(in crate::window) fn with_scroll_placements_checked(
        &self,
        placements: &[(usize, crate::window::scroll_paint::ScrollPlacement)],
        window_opaque: bool,
        bounds_changed: bool,
    ) -> Option<Self> {
        if placements.is_empty() || self.window_opaque != window_opaque {
            return None;
        }
        if !self.ordered && self.frames.is_empty() && placements.iter().all(|(_, placement)| !placement.has_static_order()) {
            // The same logical source still requires owner-side expansion.
            // There are no raster layers or opaque underlays to recapture.
            return Some(Self {
                frames: Vec::new(),
                ordered: false,
                flat_root: self.flat_root,
                window_opaque,
            });
        }
        if !self.ordered {
            return None;
        }
        if bounds_changed && (!self.flat_root
            || self.frames.iter().any(|layer| layer.background.is_some() || layer.root_static_span().is_none())
            || placements.iter().any(|(_, placement)| placement.has_lcd_glyphs()))
        {
            return None;
        }
        let mut frames = self.frames.clone();
        // Both marker replacements and native frames are in scene order.
        // Locate each slot once rather than restart the scan for every row.
        let mut slots = self.frames.iter().enumerate();
        for (index, placement) in placements {
            if !placement.is_visible() { continue; }
            let span = *index..*index + 1;
            let (slot, _) = slots.find(|(_, layer)| layer.root_static_span() == Some(&span))?;
            let old = &self.frames[slot];
            let geometry = placement.native_geometry();
            let viewport = placement.viewport();
            if old.source_id != geometry.id
                || old.generation != geometry.generation
                || old.source_viewport != viewport
                || old.scale != geometry.scale
                || (placement.has_lcd_glyphs() && (!window_opaque || old.background.is_none()))
            {
                return None;
            }
            let (operations, coverage, delta, atlas_tiles) = placement.raster_source();
            let translation = [
                geometry.source_origin[0].checked_add(delta.x.0 as i32)?,
                geometry.source_origin[1].checked_add(delta.y.0 as i32)?,
            ];
            let content = LayerContent::from_scroll(
                operations, coverage, delta, translation, viewport, atlas_tiles, Some(&old.content),
            )?;
            if !Arc::ptr_eq(&content, &old.content) {
                return None;
            }
            frames[slot].translation = translation;
        }
        // Match the general capture's same-generation adoption policy.
        for frame in &mut frames {
            frame.damage = LayerDamage::Full;
            frame.previous_coverage = None;
        }
        Some(Self { frames, ordered: true, flat_root: self.flat_root, window_opaque })
    }

    /// Merge the changed visible marker set into an already certified
    /// flat root. No ordinary operation is inspected; the scene variant
    /// has already verified source identity and unchanged indices.
    pub(in crate::window) fn with_row_visibility_delta(
        &self,
        placements: &[(usize, crate::window::scroll_paint::ScrollPlacement)],
        window_opaque: bool,
    ) -> Option<Self> {
        if placements.is_empty()
            || placements.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || !self.ordered || !self.flat_root || self.window_opaque != window_opaque
            || self.frames.iter().any(|layer| layer.background.is_some() || layer.root_static_span().is_none())
            || placements.iter().any(|(_, p)| p.has_lcd_glyphs()
                || (p.is_visible() && !p.has_static_order()))
        {
            return None;
        }
        let mut old = self.frames.iter().peekable();
        let mut frames = Vec::with_capacity(self.frames.len());
        // Native cache identities must stay unique, including newly
        // revealed rows. Existing layers have already proved uniqueness.
        let mut ids: FxHashSet<_> = self.frames.iter().map(|layer| layer.source_id).collect();
        for (index, placement) in placements {
            while old.peek().is_some_and(|layer| layer.root_static_span().unwrap().start < *index) {
                frames.push(old.next()?.clone());
            }
            let previous = if old.peek().is_some_and(|layer| layer.root_static_span().unwrap().start == *index) {
                let layer = old.next()?;
                if layer.root_static_span()? != &(*index..*index + 1) { return None; }
                Some(layer)
            } else { None };
            let geometry = placement.native_geometry();
            if let Some(previous) = previous {
                if previous.source_id != geometry.id || previous.generation != geometry.generation
                    || previous.source_viewport != placement.viewport() || previous.scale != geometry.scale {
                    return None;
                }
            }
            if !placement.is_visible() { continue; }
            if previous.is_none() && !ids.insert(geometry.id) { return None; }
            let viewport = placement.viewport();
            let (operations, coverage, delta, atlas_tiles) = placement.raster_source();
            let translation = [
                geometry.source_origin[0].checked_add(delta.x.0 as i32)?,
                geometry.source_origin[1].checked_add(delta.y.0 as i32)?,
            ];
            let content = LayerContent::from_scroll(
                operations, coverage, delta, translation, viewport, atlas_tiles,
                previous.map(|layer| &layer.content),
            )?;
            frames.push(LayerFrame {
                id: geometry.id, source_id: geometry.id, background: None,
                generation: geometry.generation,
                static_root_span: Some(*index..*index + 1),
                damage: LayerDamage::Full, previous_coverage: None,
                translation, source_viewport: viewport, scale: geometry.scale, content,
                row_raster: geometry.row_bounds.map(|bounds| {
                    translated_bounds(bounds, Point::default() - translation_point(geometry.source_origin))
                }),
            });
        }
        frames.extend(old.cloned());
        for frame in &mut frames {
            frame.damage = LayerDamage::Full;
            frame.previous_coverage = None;
        }
        Some(Self { frames, ordered: true, flat_root: true, window_opaque })
    }

    pub(in crate::window) fn capture_against(
        scene: &Scene,
        previous: Option<(&Scene, &Self)>,
        window_opaque: bool,
    ) -> Self {
        if !scene.paint_operations.has_deferred_scroll() {
            return Self::default();
        }
        // Reuse the preceding positive proof only when every input is
        // equivalent. On failure, run the authoritative full proof.
        let inherited = previous.is_some_and(|(old_scene, old_layers)| old_layers.ordered
            && root_order_inputs_match(scene, old_scene, old_layers.flat_root));
        let (ordered, flat_root) = if inherited {
            (true, previous.expect("inherited root proof").1.flat_root)
        } else {
            (ordered_static_root(scene), !scene.paint_operations.has_layer_scopes())
        };
        Self::capture_layers(scene, previous.map(|(_, layers)| layers), window_opaque, ordered, flat_root)
    }

    fn capture_layers(scene: &Scene, previous: Option<&Self>, window_opaque: bool,
        ordered: bool, flat_root: bool) -> Self {
        if !scene.paint_operations.has_deferred_scroll() {
            return Self::default();
        }
        // Composition eligibility is not a source-lifetime proof. A
        // dynamic root can still own immutable, reusable scroll textures.
        let mut seen = FxHashSet::default();
        let old = previous.map_or(&[][..], |layers| layers.frames.as_slice());
        // The common scroll publication preserves row order. Match that
        // prefix directly; build an identity index only at the first
        // insertion, removal or reorder that actually needs one.
        let mut old_by_id: Option<FxHashMap<u64, &LayerFrame>> = None;
        let mut frames = Vec::with_capacity(old.len());
        for index in scene.paint_operations.deferred_scroll_indices() {
            let Some(PaintOperationRef::DeferredScroll(placement)) = scene.paint_operations.view(index) else {
                return Self::default();
            };
            if !placement.has_static_order() {
                continue;
            }
            let geometry = placement.native_geometry();
            // A source may not be instanced twice in one root using the
            // same cache identity. Reject the entire native plan.
            if !seen.insert(geometry.id) { return Self::default(); }
            let source_viewport = placement.viewport();
            let (operations, coverage, delta, atlas_tiles) = placement.raster_source();
            let needs_background = placement.has_lcd_glyphs();
            let background = if needs_background && ordered && window_opaque {
                raster_background(scene, index, placement.visual_bounds())
            } else {
                None
            };
            let translation = [geometry.source_origin[0] + delta.x.0 as i32,
                geometry.source_origin[1] + delta.y.0 as i32];
            // Native source origins are relative to the first capture of
            // this identity; tile keys remain in stable content space.
            let previous = if let Some(layer) = old.get(frames.len())
                && layer.source_id == geometry.id
            {
                Some(layer)
            } else if old.is_empty() {
                None
            } else {
                old_by_id.get_or_insert_with(|| old.iter().map(|layer| (layer.source_id, layer)).collect())
                    .get(&geometry.id).copied()
            };
            let same_background = previous.filter(|layer| layer.background == background);
            let id = if needs_background {
                same_background.map_or_else(next_layer_id, |layer| layer.id)
            } else {
                geometry.id
            };
            let Some(content) = LayerContent::from_scroll(
                operations, coverage, delta, translation, source_viewport, atlas_tiles,
                previous.map(|layer| &layer.content),
            ) else { return Self::default(); };
            let (damage, previous_coverage) = if let Some(previous) = same_background
                && previous.generation.checked_add(1) == Some(geometry.generation)
                && previous.source_viewport == source_viewport
            {
                let common = content.raster.coverage.intersect(&previous.content.raster.coverage);
                if common.is_empty() {
                    (LayerDamage::Full, None)
                } else {
                    (content.native_damage_from(&previous.content), Some(common))
                }
            } else { (LayerDamage::Full, None) };
            frames.push(LayerFrame {
                id, source_id: geometry.id, background, generation: geometry.generation,
                // Native markers are self-contained even inside a proven
                // ordinary scope; the outside stream preserves its delimiters.
                static_root_span: (ordered && (!needs_background || background.is_some()))
                    .then_some(index..index + 1),
                damage, previous_coverage,
                translation, source_viewport, scale: geometry.scale, content,
                row_raster: geometry.row_bounds.map(|bounds| {
                    translated_bounds(bounds, Point::default() - translation_point(geometry.source_origin))
                }),
            });
        }
        Self { frames, ordered, flat_root, window_opaque }
    }


}

/// GPUI Fast's opaque underlay rule applied after native root-order
/// certification. The last draw touching the destination must cover it.
fn raster_background(scene: &Scene, index: usize, visible: Bounds<ScaledPixels>) -> Option<Rgba> {
    if visible.is_empty() {
        return None;
    }
    for operation in scene.paint_operations.range(0..index).views().rev() {
        match operation {
            PaintOperationRef::Primitive(primitive) if primitive.visual_bounds().intersects(&visible) => {
                let PrimitiveRef::Quad(quad) = primitive else { return None; };
                let color = quad.background.solid;
                let full_coverage = contains_visual(quad.bounds, visible)
                    && contains_visual(quad.content_mask.bounds, visible);
                let rectangular = quad.corner_radii == Default::default()
                    && quad.content_mask.corner_radii == Default::default();
                let no_border = quad.border_color.a == 0. || quad.border_widths == Default::default();
                return (quad.background.tag == crate::BackgroundTag::Solid
                    && color.a == 1. && full_coverage && rectangular && no_border)
                    .then_some(color);
            }
            PaintOperationRef::DeferredScroll(other) if other.visual_bounds().intersects(&visible) => {
                return None;
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Pixels, Quad, ScaledPixels, point, size, scene::{Painter, PaintOperation, Primitive}};

    #[test]
    fn ordered_root_retains_both_outside_spans() {
        let bounds = Bounds::new(point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(64.), ScaledPixels(64.)));
        let mut painter = Painter::default();
        for color in [0xff0000, 0x00ff00, 0x0000ff] {
            painter.insert_primitive(Quad {
                bounds, content_mask: ContentMask::new(bounds),
                background: crate::rgb(color).into(), ..Default::default()
            });
        }
        painter.finish();
        let scene = painter.into_scene();
        assert!(ordered_static_root(&scene));
        assert_eq!(static_root_span(&scene, 1..2), Some(1..2));
        assert_eq!(static_root_span(&scene, 0..3), Some(0..3));
        assert!(static_root_span(&scene, 3..4).is_none());
    }

    #[test]
    fn root_order_inversion_rejects_flattened_composition() {
        let bounds = Bounds::new(point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(64.), ScaledPixels(64.)));
        let mut scene = Scene::default();
        for order in [1, 3, 2] {
            scene.paint_operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                order, bounds, content_mask: ContentMask::new(bounds), ..Default::default()
            })));
        }
        scene.paint_operations.finish();
        assert!(!ordered_static_root(&scene));
    }

    #[test]
    fn overscan_source_maps_signed_content_tiles_without_viewport_projection() {
        let source = ContentRaster {
            source_origin: [0, -128],
            coverage: Bounds::new(point(ScaledPixels(0.), ScaledPixels(0.)),
                size(ScaledPixels(512.), ScaledPixels(1024.))),
        };
        assert_eq!(source.source_origin_for_tile([0, 0, 256, 256]), Some([0, -128]));
        assert_eq!(source.source_origin_for_tile([0, 768, 256, 1024]), Some([0, 640]));
        assert_eq!(source.source_origin_for_tile([0, 900, 256, 1156]), None);
    }

    #[test]
    fn scroll_source_shares_operations_and_keeps_capture_origin_after_scroll() {
        let viewport = ContentMask::new(Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(512.), ScaledPixels(256.)),
        ));
        let coverage = Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(-256.)),
            size(ScaledPixels(512.), ScaledPixels(768.)),
        );
        let mut operations = Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: viewport.bounds, content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        operations.finish();
        let operations = Arc::new(operations);
        let atlas = Arc::default();
        let before = LayerContent::from_scroll(&operations, coverage, Point::default(),
            [0, -128], viewport, &atlas, None).unwrap();
        let after = LayerContent::from_scroll(&operations, coverage, translation_point([0, -64]),
            [0, -192], viewport, &atlas, Some(&before)).unwrap();
        assert!(Arc::ptr_eq(&before, &after));
        assert_eq!(after.raster.source_origin_for_tile([0, 256, 128, 384]), Some([0, 128]));
        let clipped = ContentMask::new(Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(16.)),
            size(ScaledPixels(512.), ScaledPixels(192.)),
        ));
        let clipped = LayerContent::from_scroll(&operations, coverage, translation_point([0, -64]),
            [0, -192], clipped, &atlas, Some(&after)).unwrap();
        assert!(Arc::ptr_eq(&before, &clipped));
        let uncovered = ContentMask::new(Bounds::new(Point::default(),
            size(ScaledPixels(1024.), ScaledPixels(256.))));
        assert!(LayerContent::from_scroll(&operations, coverage, translation_point([0, -64]),
            [0, -192], uncovered, &atlas, Some(&after)).is_none());
        let new_origin = LayerContent::from_scroll(&operations, coverage, Point::default(),
            [0, -64], viewport, &atlas, Some(&before)).unwrap();
        assert!(!Arc::ptr_eq(&before, &new_origin));
    }

    #[test]
    fn tile_adoption_requires_adjacent_generation_and_complete_coverage() {
        let viewport = ContentMask::new(Bounds::new(
            point(crate::px(0.), crate::px(0.)),
            size(crate::px(1024.), crate::px(1024.)),
        ));
        let frame = LayerFrame {
            id: 1, source_id: 1, generation: 4, background: None,
            static_root_span: None,
            damage: LayerDamage::Bounds(Bounds::new(
                point(ScaledPixels(600.), ScaledPixels(20.)),
                size(ScaledPixels(10.), ScaledPixels(10.)),
            )),
            scale: 1.,
            source_viewport: viewport.scale(1.),
            translation: [0, 0],
            previous_coverage: Some(viewport.scale(1.).bounds),
            content: Arc::new(LayerContent {
                operations: Arc::default(),
                raster: ContentRaster { source_origin: [0, 0], coverage: viewport.scale(1.).bounds },
                raster_atlas_tiles: Arc::default(),
            }),
            row_raster: None,
        };
        assert_eq!(frame.adopt_tile(3, [0, 0, 512, 512], 1.), Some(4));
        assert_eq!(frame.adopt_tile(3, [512, 0, 1024, 512], 1.), None);
        assert_eq!(frame.adopt_tile(3, [-1, 0, 511, 512], 1.), None);
        assert_eq!(frame.adopt_tile(2, [0, 0, 512, 512], 1.), None);
    }

}

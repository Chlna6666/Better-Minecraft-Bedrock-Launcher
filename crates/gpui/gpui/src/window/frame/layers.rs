//! Immutable scroll content publication, adapted from GPUI Fast's LayerContent/LayerPart.
//!
//! Prefer an exact overscanned view's immutable source; ordinary visible
//! Scene ranges remain the fallback for incomplete or unsupported captures.
use super::{LayerRecord, LayerScrollPosition};
use crate::{Bounds, ContentMask, Pixels, Point, ScaledPixels, TransformationMatrix, point, scene::{Operations, PaintOperation, Primitive, Scene}};
use collections::{FxHashMap, FxHashSet};
use std::{ops::Range, sync::{Arc, atomic::{AtomicU64, Ordering}}};

static NEXT_LAYER_ID: AtomicU64 = AtomicU64::new(1);

/// A contiguous part of a layer. Lists retain logical row identities; gaps
/// (decorations or other content) keep their original painter order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayerPart {
    pub(crate) row: Option<usize>,
    pub(crate) operations: Range<usize>,
}

pub(crate) struct LayerContent {
    // Reuses the original immutable operation pages instead of copying a
    // Scene per row. This payload never retains the enclosing Scene itself.
    operations: Arc<Operations>,
    parts: Box<[LayerPart]>,
    raster: Option<ContentRaster>,
    // Immutable logical glyph identities collected once with ScrollPaint.
    raster_atlas_tiles: Arc<[(crate::AtlasTextureId, u32)]>,
}

impl LayerContent {
    fn capture(scene: &Scene, range: Range<usize>, rows: &[LayerPart]) -> Option<Self> {
        if range.is_empty() || !scene.can_reuse_range_bounds(range.clone()) {
            return None;
        }
        let mut parts = Vec::with_capacity(rows.len().saturating_add(1));
        let mut cursor = range.start;
        for row in rows {
            if row.operations.start < cursor || row.operations.end < row.operations.start
                || row.operations.end > range.end
            {
                return None;
            }
            if cursor < row.operations.start {
                parts.push(LayerPart {
                    row: None,
                    operations: cursor - range.start..row.operations.start - range.start,
                });
            }
            parts.push(LayerPart {
                row: row.row,
                operations: row.operations.start - range.start..row.operations.end - range.start,
            });
            cursor = row.operations.end;
        }
        if cursor < range.end {
            parts.push(LayerPart {
                row: None,
                operations: cursor - range.start..range.end - range.start,
            });
        }
        let mut operations = Operations::default();
        operations.share(&scene.paint_operations, range);
        Some(Self { operations: Arc::new(operations), parts: parts.into_boxed_slice(), raster: None, raster_atlas_tiles: Arc::default() })
    }

    fn from_scroll(
        operations: &Arc<Operations>,
        source_coverage: Bounds<ScaledPixels>,
        delta: Point<ScaledPixels>,
        translation: Option<[i32; 2]>,
        source_viewport: ContentMask<ScaledPixels>,
        quad_only: bool,
        atlas_tiles: &Arc<[(crate::AtlasTextureId, u32)]>,
    ) -> Option<Self> {
        let [x, y] = translation?;
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
        Some(Self {
            operations: Arc::clone(operations),
            parts: Box::new([LayerPart { row: None, operations: 0..operations.len() }]),
            raster: Some(ContentRaster { source_origin, coverage, quad_only }),
            raster_atlas_tiles: Arc::clone(atlas_tiles),
        })
    }

    // GPUI Fast inherits a tile only when its ordered contributing paint
    // is unchanged. Use the existing immutable operation pages for a cheap
    // equality proof; unknown effects and topology changes reject adoption.
    fn damage_from(&self, previous: &Self) -> LayerDamage {
        if self.parts != previous.parts || self.operations.len() != previous.operations.len() {
            return LayerDamage::Full;
        }
        let mut damage: Option<Bounds<ScaledPixels>> = None;
        let valid = self.operations.matches_by(&previous.operations, |current, old| {
            let (PaintOperation::Primitive(current), PaintOperation::Primitive(old)) = (current, old) else {
                return current.same_paint(old)
                    && matches!(current, PaintOperation::StartLayer(_) | PaintOperation::EndLayer);
            };
            let Some(current_bounds) = static_raster_bounds(current) else { return false; };
            let Some(old_bounds) = static_raster_bounds(old) else { return false; };
            if !current.visually_eq(old) {
                let changed = current_bounds.union(&old_bounds);
                damage = Some(damage.map_or(changed, |bounds| bounds.union(&changed)));
            }
            true
        });
        if valid {
            damage.map_or(LayerDamage::Clean, LayerDamage::Bounds)
        } else {
            LayerDamage::Full
        }
    }

    /// Compare only the pixels that both viewport captures actually own.
    /// Rows entering or leaving the viewport can change the operation count
    /// without changing a single contributing primitive in this intersection.
    fn matches_translated(
        &self,
        previous: &Self,
        old_translation: [i32; 2],
        current_translation: [i32; 2],
        common: Bounds<ScaledPixels>,
        viewport: ContentMask<ScaledPixels>,
    ) -> bool {
        let delta = translation_point([
            current_translation[0] - old_translation[0],
            current_translation[1] - old_translation[1],
        ]);
        let current_region = translated_bounds(common, translation_point(current_translation));
        let old_region = translated_bounds(common, translation_point(old_translation));
        let mut current = self.contributors(current_region, viewport);
        let mut old = previous.contributors(old_region, viewport);
        loop {
            match (current.next(), old.next()) {
                (None, None) => return true,
                (Some(Ok(current)), Some(Ok(old)))
                    if translated_primitive_matches(current, old, delta, viewport) => {}
                // Unknown effects are never skipped, even outside the
                // intersection: their dependency footprint is not proven.
                _ => return false,
            }
        }
    }

    fn contributors(
        &self,
        region: Bounds<ScaledPixels>,
        viewport: ContentMask<ScaledPixels>,
    ) -> impl Iterator<Item = Result<&Primitive, ()>> + '_ {
        self.operations.iter().filter_map(move |operation| {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    let Some(bounds) = static_raster_bounds(primitive) else {
                        return Some(Err(()));
                    };
                    if *primitive.content_mask() != viewport {
                        return Some(Err(()));
                    }
                    (!bounds.intersect(&region).is_empty()).then_some(Ok(primitive))
                }
                // These markers do not emit pixels. Their actual ordering
                // effect is preserved by comparing each primitive's exact
                // DrawOrder and keeping the contributing operation order.
                PaintOperation::StartLayer(_) | PaintOperation::EndLayer => None,
                PaintOperation::StartBlur(_) | PaintOperation::EndBlur => Some(Err(())),
            }
        })
    }

    fn matches(&self, other: &Self) -> bool {
        self.raster == other.raster && self.parts == other.parts
            && self.operations.matches_by(&other.operations, PaintOperation::same_paint)
    }
}

/// Rasterization uses the source's original coordinates; only the signed
/// RasterTile origin changes. No UI-side translation or new primitive stream.
#[derive(Clone, Copy, PartialEq)]
struct ContentRaster {
    source_origin: [i32; 2],
    coverage: Bounds<ScaledPixels>,
    quad_only: bool,
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
pub(super) fn integral_translation(
    offset: LayerScrollPosition,
    visual_scale: f32,
    device_scale: f32,
) -> Option<[i32; 2]> {
    let offset = match offset {
        LayerScrollPosition::Pixels(offset) => offset,
        LayerScrollPosition::List { pixel_offset: Some(y), .. } => point(crate::px(0.), y),
        LayerScrollPosition::List { pixel_offset: None, .. } => return None,
    };
    if visual_scale != 1. || !device_scale.is_finite() || device_scale <= 0. {
        return None;
    }
    let coordinate = |p: Pixels| {
        let value = p.0 * device_scale;
        // Keep exact f32 pixel arithmetic; no rounding of logical state.
        (value.is_finite() && value.fract() == 0. && value.abs() <= 1_048_576.)
            .then_some(value as i32)
    };
    Some([coordinate(offset.x)?, coordinate(offset.y)?])
}

fn translation_point(translation: [i32; 2]) -> Point<ScaledPixels> {
    point(ScaledPixels(translation[0] as f32), ScaledPixels(translation[1] as f32))
}

fn translated_bounds(mut bounds: Bounds<ScaledPixels>, delta: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    bounds.origin = bounds.origin + delta;
    bounds
}

// GPUI Fast translates primitive coordinates and local masks together.
// Here the only admitted mask is the exact fixed rectangular viewport.
// Its pixels agree inside the intersection, so preserve it rather than
// translating the ancestor clip. Rounded and nested masks are not admitted.
fn translated_primitive_matches(
    current: &Primitive,
    old: &Primitive,
    delta: Point<ScaledPixels>,
    viewport: ContentMask<ScaledPixels>,
) -> bool {
    if static_raster_bounds(current).is_none() || static_raster_bounds(old).is_none()
        || *current.content_mask() != viewport || *old.content_mask() != viewport
    {
        return false;
    }
    macro_rules! compare {
        ($a:ident, $b:ident) => {{
            let mut moved = *$b;
            moved.bounds = translated_bounds(moved.bounds, delta);
            *$a == moved
        }};
    }
    match (current, old) {
        (Primitive::Quad(a), Primitive::Quad(b)) => compare!(a, b),
        (Primitive::Shadow(a), Primitive::Shadow(b)) => compare!(a, b),
        (Primitive::Underline(a), Primitive::Underline(b)) => compare!(a, b),
        (Primitive::MonochromeSprite(a), Primitive::MonochromeSprite(b)) => compare!(a, b),
        (Primitive::PolychromeSprite(a), Primitive::PolychromeSprite(b)) => compare!(a, b),
        _ => false,
    }
}

// The source still covers only a viewport, not an overscan region.
#[derive(Clone, Copy)]
enum LayerDamage {
    Full,
    Clean,
    Bounds(Bounds<ScaledPixels>),
}

fn static_raster_bounds(primitive: &Primitive) -> Option<Bounds<ScaledPixels>> {
    if primitive.animation_id().is_some() {
        return None;
    }
    match primitive {
        Primitive::Quad(_) | Primitive::Shadow(_) | Primitive::Underline(_) => {}
        Primitive::PolychromeSprite(sprite) if sprite.sampling != 2 => {}
        Primitive::MonochromeSprite(sprite)
            if sprite.transformation == TransformationMatrix::unit()
                && sprite.tile.texture_id.kind != crate::AtlasTextureKind::Subpixel => {}
        _ => return None,
    }
    // Include the antialiased boundary; never claim a clean neighbor from
    // a fractional geometry edge or filter footprint.
    let bounds = primitive.visual_bounds().dilate(ScaledPixels(2.));
    [bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0]
        .into_iter().all(f32::is_finite).then_some(bounds)
}

// First production admission is deliberately narrow: only independently
// compositable static primitives with the original rectangular capture mask.
// Effects, LCD coverage and transformed glyphs must keep the ordinary path.
pub(in crate::window) fn static_scroll_raster_operation(
    operation: &PaintOperation,
    coverage: Bounds<ScaledPixels>,
) -> bool {
    match operation {
        PaintOperation::StartLayer(bounds) => [
            bounds.left().0, bounds.top().0, bounds.right().0, bounds.bottom().0,
        ].into_iter().all(f32::is_finite),
        PaintOperation::EndLayer => true,
        PaintOperation::Primitive(primitive) => {
            primitive.content_mask().bounds == coverage
                && primitive.content_mask().corner_radii == Default::default()
                && matches!(primitive, Primitive::Quad(_) | Primitive::MonochromeSprite(_))
                && static_raster_bounds(primitive).is_some()
        }
        _ => false,
    }
}

pub(crate) struct LayerFrame {
    layout_key: u64,
    /// Stable within one mounted lifetime; not a transient HitboxId.
    pub(crate) id: u64,
    pub(crate) generation: u64,
    /// Original root operation ranges before and after the promoted fragment.
    /// A verified static painter-order root can be split without reading the
    /// promoted operations on the GPU owner.
    quad_root_outside: Option<[Range<usize>; 2]>,
    damage: LayerDamage,
    /// Certified coverage shared with the immediately previous generation,
    /// in stable content-space device pixels.
    previous_coverage: Option<Bounds<ScaledPixels>>,
    translation: Option<[i32; 2]>,
    source_viewport: ContentMask<ScaledPixels>,
    viewport: ContentMask<Pixels>,
    offset: LayerScrollPosition,
    pub(crate) content: Arc<LayerContent>,
}

impl LayerFrame {
    fn coverage(viewport: ContentMask<ScaledPixels>, translation: Option<[i32; 2]>) -> Bounds<ScaledPixels> {
        let [x, y] = translation.unwrap_or([0, 0]);
        translated_bounds(viewport.bounds, translation_point([-x, -y]))
    }

    fn transition(&self, record: &LayerRecord, content: &LayerContent) -> (LayerDamage, Option<Bounds<ScaledPixels>>) {
        if self.viewport != record.viewport || self.source_viewport != record.source_viewport
            || self.source_viewport.corner_radii != Default::default()
        {
            return (LayerDamage::Full, None);
        }
        let current_coverage = Self::coverage(record.source_viewport, record.translation);
        let old_coverage = Self::coverage(self.source_viewport, self.translation);
        let common = current_coverage.intersect(&old_coverage);
        if common.is_empty() {
            return (LayerDamage::Full, None);
        }
        let damage = if self.offset == record.offset && self.translation == record.translation {
            match content.damage_from(&self.content) {
                LayerDamage::Bounds(bounds) => {
                    let [x, y] = record.translation.unwrap_or([0, 0]);
                    LayerDamage::Bounds(translated_bounds(bounds, translation_point([-x, -y])))
                }
                damage => damage,
            }
        } else if let (Some(old), Some(current)) = (self.translation, record.translation) {
            if content.matches_translated(&self.content, old, current, common, record.source_viewport) {
                LayerDamage::Clean
            } else {
                LayerDamage::Full
            }
        } else {
            LayerDamage::Full
        };
        (damage, Some(common))
    }

    /// A static quad root may contain ordinary drawing around this layer.
    /// The certified outside ranges retain their original paint order.
    pub(crate) fn root_quad_raster(&self) -> Option<(Bounds<ScaledPixels>, [i32; 2])> {
        (self.quad_root_outside.is_some() && self.content.raster?.quad_only)
            .then_some((self.source_viewport.bounds, self.translation?))
    }

    pub(crate) fn raster_atlas_tiles(&self) -> &[(crate::AtlasTextureId, u32)] {
        &self.content.raster_atlas_tiles
    }

    pub(crate) fn root_quad_outside(&self) -> Option<&[Range<usize>; 2]> {
        self.quad_root_outside.as_ref()
    }

    /// Borrow the original capture, along with the signed projection origin
    /// for this content-space tile. The cache key itself must remain in
    /// content coordinates. The renderer can compile this source on a miss;
    /// a resident hit need not inspect a single primitive.
    pub(crate) fn raster_source_for_tile(
        &self, rect: [i64; 4],
    ) -> Option<(&Operations, [i32; 2])> {
        let origin = self.content.raster?.source_origin_for_tile(rect)?;
        Some((&self.content.operations, origin))
    }

    /// Only adjacent generations can inherit a visible-source tile. Direct
    /// overscan sources remain in the same generation until content changes.
    pub(crate) fn adopt_tile(&self, generation: u64, rect: [i64; 4], scale: f32) -> Option<u64> {
        // Overscan residency is keyed in stable content space. A same-generation
        // tile is valid only within the immutable source's certified coverage.
        if self.content.raster.is_some() {
            if generation != self.generation || !scale.is_finite() || scale <= 0.
                || self.viewport.scale(scale) != self.source_viewport
            {
                return None;
            }
            self.raster_source_for_tile(rect)?;
            return Some(generation);
        }
        if generation == self.generation {
            return Some(generation);
        }
        if generation.checked_add(1) != Some(self.generation) || !scale.is_finite() || scale <= 0.
            || self.viewport.scale(scale) != self.source_viewport
        {
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

// A raw paint-order split is valid only if the native quad stream has
// that same order. Scope markers themselves draw nothing, but inherited
// DrawOrder can reorder nested layers. Reject inversions rather than
// guessing which overlapping primitives can safely commute.
fn ordered_static_quad_root(scene: &Scene) -> bool {
    if !scene.animation_values.is_empty() || !scene.renderer_extensions.is_empty()
        || !scene.blurs.is_empty() || !scene.backdrop_blurs.is_empty()
    {
        return false;
    }
    let mut previous = None;
    for operation in scene.paint_operations.iter() {
        match operation {
            PaintOperation::Primitive(primitive @ Primitive::Quad(_)) => {
                let order = primitive.order();
                if primitive.animation_id().is_some() || previous.is_some_and(|last| last > order)
                    || static_raster_bounds(primitive).is_none()
                {
                    return false;
                }
                previous = Some(order);
            }
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
            _ => return false,
        }
    }
    true
}

fn quad_outside_ranges(scene: &Scene, range: Range<usize>) -> Option<[Range<usize>; 2]> {
    if range.is_empty() || range.end > scene.len() { return None; }
    let mut outside = [0..range.start, range.end..scene.len()];
    // Empty scopes do not need a native draw or an owner-side scan.
    for span in &mut outside {
        if scene.paint_operations.range(span.clone()).iter().all(|operation|
            matches!(operation, PaintOperation::StartLayer(_) | PaintOperation::EndLayer))
        {
            *span = 0..0;
        }
    }
    Some(outside)
}

#[derive(Default)]
pub(crate) struct SceneLayers {
    pub(crate) frames: Vec<LayerFrame>,
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
        assert!(operations.iter().all(|op| static_scroll_raster_operation(op, coverage)));
        let atlas_tiles: Arc<[_]> = operations.iter().filter_map(|op| match op {
            PaintOperation::Primitive(Primitive::MonochromeSprite(sprite)) => Some((sprite.tile.texture_id, sprite.tile.tile_id.0)),
            _ => None,
        }).collect::<FxHashSet<_>>().into_iter().collect::<Vec<_>>().into();
        let operations = Arc::new(operations);
        let content = LayerContent::from_scroll(
            &operations, coverage, Point::default(), Some(translation), viewport.scale(1.), atlas_tiles.is_empty(), &atlas_tiles,
        ).expect("fixture source coverage");
        assert!(Arc::ptr_eq(&operations, &content.operations));
        self.frames.push(LayerFrame {
            layout_key: 1, id: 1, generation: 1,
            quad_root_outside: Some([0..0, 0..0]),
            damage: LayerDamage::Full, previous_coverage: None,
            translation: Some(translation), source_viewport: viewport.scale(1.),
            viewport, offset: LayerScrollPosition::Pixels(point(crate::px(translation[0] as f32), crate::px(translation[1] as f32))),
            content: Arc::new(content),
        });
    }

    #[cfg(test)]
    pub(crate) fn certify_fixture_root(&mut self, scene: &Scene, range: Range<usize>) {
        self.frames[0].quad_root_outside = ordered_static_quad_root(scene)
            .then(|| quad_outside_ranges(scene, range)).flatten();
    }

    pub(super) fn capture(
        scene: &Scene,
        records: &[LayerRecord],
        scroll_views: &crate::window::scroll_views::ScrollViews,
        view_records: &[crate::window::view_retention::ViewRecordSlot],
        previous: Option<&Self>,
    ) -> Self {
        if records.is_empty() {
            return Self::default();
        }
        let mut unique = FxHashSet::default();
        let mut duplicates = FxHashSet::default();
        for record in records {
            if let Some(key) = record.layout_key {
                if !unique.insert(key) {
                    duplicates.insert(key);
                }
            }
        }
        let old: FxHashMap<_, _> = previous.into_iter()
            .flat_map(|layers| &layers.frames)
            .map(|layer| (layer.layout_key, layer))
            .collect();
        let mut frames = Vec::new();
        // Evaluate at most once per published root, regardless of candidate count.
        let mut ordered_quad_root = None;
        for record in records {
            let (Some(key), Some(paint)) = (record.layout_key, record.paint.as_ref()) else { continue; };
            if duplicates.contains(&key) { continue; }
            let range = paint.start.scene_index..paint.end.scene_index;
            // Derive the physical scale from the owning retained view, not the
            // current native drawable (which can already have resized).
            let direct = if record.rows.is_empty() && !range.is_empty() {
                scroll_views.raster_source(view_records, &range, record.viewport, record.source_viewport)
                     .and_then(|(operations, coverage, delta, quad_only, atlas_tiles)| LayerContent::from_scroll(
                        operations, coverage, delta, record.translation, record.source_viewport, quad_only, atlas_tiles,
                    ))
            } else { None };
            let quad_root_outside = if direct.as_ref().is_some_and(|content| content.raster.is_some_and(|raster| raster.quad_only))
                && *ordered_quad_root.get_or_insert_with(|| ordered_static_quad_root(scene))
            {
                quad_outside_ranges(scene, range.clone())
            } else { None };
            let Some(content) = direct.or_else(|| LayerContent::capture(scene, range, &record.rows)) else { continue; };
            let previous = old.get(&key).copied();
            let (id, generation, content, damage, previous_coverage) = if let Some(previous) = previous {
                if content.raster.is_some() && previous.content.raster == content.raster
                    && Arc::ptr_eq(&previous.content.operations, &content.operations)
                    && previous.source_viewport == record.source_viewport
                {
                    // Scroll changes the composite placement, not the source.
                    (previous.id, previous.generation, Arc::clone(&previous.content), LayerDamage::Full, None)
                } else if previous.viewport == record.viewport && previous.offset == record.offset
                    && previous.source_viewport == record.source_viewport
                    && previous.translation == record.translation
                    && previous.content.matches(&content)
                {
                    // Equal pixels need not share their backing pages. Do
                    // not pin unrelated historical window operations.
                    let shared = previous.content.operations.matches_by(&content.operations, |_, _| false);
                    let content = if shared { Arc::clone(&previous.content) } else { Arc::new(content) };
                    // Preserve the last transition proof if the GPU owner
                    // has not yet consumed it.
                    (previous.id, previous.generation, content, previous.damage, previous.previous_coverage)
                } else if let Some(next) = previous.generation.checked_add(1) {
                    let (damage, coverage) = if content.raster.is_some() || previous.content.raster.is_some() {
                        (LayerDamage::Full, None)
                    } else { previous.transition(record, &content) };
                    (previous.id, next, Arc::new(content), damage, coverage)
                } else {
                    (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full, None)
                }
            } else {
                (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full, None)
            };
            frames.push(LayerFrame {
                layout_key: key, id, generation, quad_root_outside, damage, previous_coverage,
                translation: record.translation, source_viewport: record.source_viewport,
                viewport: record.viewport, offset: record.offset, content,
            });
        }
        Self { frames }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Quad, ScaledPixels, point, size, scene::Painter};

    #[test]
    fn ordered_quad_root_retains_both_outside_spans() {
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
        assert!(ordered_static_quad_root(&scene));
        assert_eq!(quad_outside_ranges(&scene, 1..2), Some([0..1, 2..3]));
        assert_eq!(quad_outside_ranges(&scene, 0..3), Some([0..0, 0..0]));
        assert!(quad_outside_ranges(&scene, 3..4).is_none());
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
        assert!(!ordered_static_quad_root(&scene));
    }

    #[test]
    fn overscan_source_maps_signed_content_tiles_without_viewport_projection() {
        let source = ContentRaster {
            source_origin: [0, -128],
            quad_only: true,
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
        let before = LayerContent::from_scroll(&operations, coverage, Point::default(),
            Some([0, -128]), viewport, true, &Arc::default()).unwrap();
        let after = LayerContent::from_scroll(&operations, coverage, translation_point([0, -64]),
            Some([0, -192]), viewport, true, &Arc::default()).unwrap();
        assert!(Arc::ptr_eq(&before.operations, &after.operations));
        assert!(before.raster == after.raster);
        assert_eq!(after.raster.unwrap().source_origin_for_tile([0, 256, 128, 384]), Some([0, 128]));
    }

    #[test]
    fn tile_adoption_requires_adjacent_generation_and_complete_coverage() {
        let viewport = ContentMask::new(Bounds::new(
            point(crate::px(0.), crate::px(0.)),
            size(crate::px(1024.), crate::px(1024.)),
        ));
        let frame = LayerFrame {
            layout_key: 1, id: 1, generation: 4,
            quad_root_outside: None,
            damage: LayerDamage::Bounds(Bounds::new(
                point(ScaledPixels(600.), ScaledPixels(20.)),
                size(ScaledPixels(10.), ScaledPixels(10.)),
            )),
            viewport,
            source_viewport: viewport.scale(1.),
            translation: Some([0, 0]),
            previous_coverage: Some(viewport.scale(1.).bounds),
            offset: LayerScrollPosition::Pixels(point(crate::px(0.), crate::px(0.))),
            content: Arc::new(LayerContent { operations: Arc::new(Operations::default()), parts: Box::new([]), raster: None, raster_atlas_tiles: Arc::default() }),
        };
        assert_eq!(frame.adopt_tile(3, [0, 0, 512, 512], 1.), Some(4));
        assert_eq!(frame.adopt_tile(3, [512, 0, 1024, 512], 1.), None);
        assert_eq!(frame.adopt_tile(3, [-1, 0, 511, 512], 1.), None);
        assert_eq!(frame.adopt_tile(2, [0, 0, 512, 512], 1.), None);
    }

    #[test]
    fn integral_scroll_translation_keeps_fixed_clip_and_glyph_phase() {
        let viewport = ContentMask::new(Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(1024.), ScaledPixels(1024.)),
        ));
        let delta = translation_point([0, -128]);
        let old = Quad {
            bounds: Bounds::new(point(ScaledPixels(20.), ScaledPixels(300.)),
                size(ScaledPixels(100.), ScaledPixels(80.))),
            content_mask: viewport,
            ..Default::default()
        };
        let current = Quad { bounds: translated_bounds(old.bounds, delta), ..old };
        assert!(translated_primitive_matches(&Primitive::Quad(current), &Primitive::Quad(old), delta, viewport));
        let wrong_clip = Quad { content_mask: ContentMask::new(current.bounds), ..current };
        assert!(!translated_primitive_matches(&Primitive::Quad(wrong_clip), &Primitive::Quad(old), delta, viewport));
        let common = LayerFrame::coverage(viewport, Some([0, -128]))
            .intersect(&LayerFrame::coverage(viewport, Some([0, 0])));
        assert_eq!(common.top(), ScaledPixels(128.));
        assert_eq!(common.bottom(), ScaledPixels(1024.));
        assert_eq!(integral_translation(LayerScrollPosition::Pixels(point(crate::px(0.), crate::px(-64.))), 1., 2.), Some([0, -128]));
        assert_eq!(integral_translation(LayerScrollPosition::Pixels(point(crate::px(0.), crate::px(-64.25))), 1., 2.), None);
    }

    #[test]
    fn list_translation_requires_a_measured_pixel_offset() {
        let logical = crate::ListOffset { item_ix: 12, offset_in_item: crate::px(4.) };
        let position = |pixel_offset| LayerScrollPosition::List { logical, pixel_offset };
        assert_eq!(integral_translation(position(None), 1., 2.), None);
        assert_eq!(integral_translation(position(Some(crate::px(-124.))), 1., 2.), Some([0, -248]));
        assert_eq!(integral_translation(position(Some(crate::px(-124.25))), 1., 2.), None);
    }

    #[test]
    fn entering_and_leaving_rows_do_not_invalidate_common_coverage() {
        let viewport = ContentMask::new(Bounds::new(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(1024.), ScaledPixels(1024.)),
        ));
        let common = LayerFrame::coverage(viewport, Some([0, -128]))
            .intersect(&LayerFrame::coverage(viewport, Some([0, 0])));
        let quad = |y| Quad {
            bounds: Bounds::new(point(ScaledPixels(20.), ScaledPixels(y)),
                size(ScaledPixels(100.), ScaledPixels(32.))),
            content_mask: viewport,
            ..Default::default()
        };
        let content = |quads: &[Quad]| {
            let mut operations = Operations::default();
            for quad in quads {
                operations.push(PaintOperation::Primitive(Primitive::Quad(*quad)));
            }
            LayerContent { operations: Arc::new(operations), parts: Box::new([]), raster: None, raster_atlas_tiles: Arc::default() }
        };
        let old = content(&[quad(20.), quad(300.)]);
        // The first old row is no longer visible; two new rows are outside
        // the shared region. Neither should affect the shared tile pixels.
        let current = content(&[quad(172.), quad(920.), quad(970.)]);
        assert!(current.matches_translated(&old, [0, 0], [0, -128], common, viewport));
        let changed = content(&[quad(173.), quad(920.), quad(970.)]);
        assert!(!changed.matches_translated(&old, [0, 0], [0, -128], common, viewport));
        let entering = content(&[quad(172.), quad(895.)]);
        // This row touches the common edge, including the AA guard.
        assert!(!entering.matches_translated(&old, [0, 0], [0, -128], common, viewport));
        let reordered = content(&[Quad { order: 1, ..quad(172.) }]);
        assert!(!reordered.matches_translated(&old, [0, 0], [0, -128], common, viewport));
    }

    #[test]
    fn row_parts_share_finalized_operations_in_painter_order() {
        let mut painter = Painter::default();
        for x in [0., 10., 20., 30.] {
            let bounds = Bounds::new(point(ScaledPixels(x), ScaledPixels(0.)),
                size(ScaledPixels(8.), ScaledPixels(8.)));
            painter.insert_primitive(Quad { bounds, content_mask: ContentMask::new(bounds), ..Default::default() });
        }
        painter.finish();
        let scene = painter.into_scene();
        let content = LayerContent::capture(&scene, 1..4, &[
            LayerPart { row: Some(9), operations: 1..2 },
            LayerPart { row: Some(8), operations: 2..3 },
        ]).unwrap();
        assert_eq!(content.parts.as_ref(), &[
            LayerPart { row: Some(9), operations: 0..1 },
            LayerPart { row: Some(8), operations: 1..2 },
            LayerPart { row: None, operations: 2..3 },
        ]);
        assert!(std::ptr::eq(&content.operations[0], &scene.paint_operations[1]));
        assert!(LayerContent::capture(&scene, 1..4, &[
            LayerPart { row: Some(0), operations: 0..2 },
        ]).is_none());
    }
}

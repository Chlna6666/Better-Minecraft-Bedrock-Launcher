//! Immutable scroll content publication, adapted from GPUI Fast's LayerContent/LayerPart.
//!
//! The painter still culls to the viewport. Only the intersection of two
//! certified content-space captures can inherit translated pixels; newly
//! exposed regions and rounded clips must use the ordinary drawing stream.
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
    operations: Operations,
    parts: Box<[LayerPart]>,
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
        Some(Self { operations, parts: parts.into_boxed_slice() })
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

    /// Fast's content-space translation, but do not mutate or copy the
    /// authoritative scene. Compare the two captures in one coordinate
    /// system. Any difference declines translation reuse for this version.
    fn matches_translated(
        &self,
        previous: &Self,
        delta: Point<ScaledPixels>,
        viewport: ContentMask<ScaledPixels>,
    ) -> bool {
        if self.parts != previous.parts || self.operations.len() != previous.operations.len() {
            return false;
        }
        // Pointer equality alone cannot prove equality after translation.
        self.operations.iter().zip(previous.operations.iter()).all(|(current, old)| {
            match (current, old) {
                (PaintOperation::Primitive(current), PaintOperation::Primitive(old)) => {
                    translated_primitive_matches(current, old, delta, viewport)
                }
                (PaintOperation::StartLayer(current), PaintOperation::StartLayer(old)) => {
                    // A layer is ordering metadata. Require matching translated
                    // geometry or the same fixed viewport on both sides.
                    *current == translated_bounds(*old, delta)
                        || (*current == viewport.bounds && *old == viewport.bounds)
                }
                (PaintOperation::EndLayer, PaintOperation::EndLayer) => true,
                _ => false,
            }
        })
    }

    fn matches(&self, other: &Self) -> bool {
        self.parts == other.parts
            && self.operations.matches_by(&other.operations, PaintOperation::same_paint)
    }
}

// GPU raster coordinates are stable content pixels, with an integral
// device-pixel translation into the original captured window coordinates.
// Fractional/variable-height logical positions have no translation proof.
pub(super) fn integral_translation(
    offset: LayerScrollPosition,
    visual_scale: f32,
    device_scale: f32,
) -> Option<[i32; 2]> {
    let LayerScrollPosition::Pixels(offset) = offset else { return None; };
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

pub(crate) struct LayerFrame {
    layout_key: u64,
    /// Stable within one mounted lifetime; not a transient HitboxId.
    pub(crate) id: u64,
    pub(crate) generation: u64,
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
            let delta = translation_point([current[0] - old[0], current[1] - old[1]]);
            if content.matches_translated(&self.content, delta, record.source_viewport) {
                LayerDamage::Clean
            } else {
                LayerDamage::Full
            }
        } else {
            LayerDamage::Full
        };
        (damage, Some(common))
    }

    /// Only adjacent generations can inherit a tile. Its coordinates are
    /// stable content pixels, NOT the current viewport placement.
    pub(crate) fn adopt_tile(&self, generation: u64, rect: [i64; 4], scale: f32) -> Option<u64> {
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

#[derive(Default)]
pub(crate) struct SceneLayers {
    pub(crate) frames: Vec<LayerFrame>,
}

impl SceneLayers {
    pub(super) fn capture(scene: &Scene, records: &[LayerRecord], previous: Option<&Self>) -> Self {
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
        for record in records {
            let (Some(key), Some(paint)) = (record.layout_key, record.paint.as_ref()) else { continue; };
            if duplicates.contains(&key) { continue; }
            let range = paint.start.scene_index..paint.end.scene_index;
            let Some(content) = LayerContent::capture(scene, range, &record.rows) else { continue; };
            let previous = old.get(&key).copied();
            let (id, generation, content, damage, previous_coverage) = if let Some(previous) = previous {
                if previous.viewport == record.viewport && previous.offset == record.offset
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
                    let (damage, coverage) = previous.transition(record, &content);
                    (previous.id, next, Arc::new(content), damage, coverage)
                } else {
                    (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full, None)
                }
            } else {
                (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full, None)
            };
            frames.push(LayerFrame {
                layout_key: key, id, generation, damage, previous_coverage,
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
    fn tile_adoption_requires_adjacent_generation_and_complete_coverage() {
        let viewport = ContentMask::new(Bounds::new(
            point(crate::px(0.), crate::px(0.)),
            size(crate::px(1024.), crate::px(1024.)),
        ));
        let frame = LayerFrame {
            layout_key: 1, id: 1, generation: 4,
            damage: LayerDamage::Bounds(Bounds::new(
                point(ScaledPixels(600.), ScaledPixels(20.)),
                size(ScaledPixels(10.), ScaledPixels(10.)),
            )),
            viewport,
            source_viewport: viewport.scale(1.),
            translation: Some([0, 0]),
            previous_coverage: Some(viewport.scale(1.).bounds),
            offset: LayerScrollPosition::Pixels(point(crate::px(0.), crate::px(0.))),
            content: Arc::new(LayerContent { operations: Operations::default(), parts: Box::new([]) }),
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

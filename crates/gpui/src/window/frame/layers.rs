//! Immutable scroll content publication, adapted from GPUI Fast's LayerContent/LayerPart.
//!
//! The current painter still culls to the viewport. These fragments therefore
//! certify only their original placement, not overscan or scroll translation.
//! Keep the ordinary drawing stream until the renderer can consume a complete
//! content-space capture.
use super::{LayerRecord, LayerScrollPosition};
use crate::{Bounds, ContentMask, Pixels, ScaledPixels, TransformationMatrix, scene::{Operations, PaintOperation, Primitive, Scene}};
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

    fn matches(&self, other: &Self) -> bool {
        self.parts == other.parts
            && self.operations.matches_by(&other.operations, PaintOperation::same_paint)
    }
}

// This proof currently applies only to window-space source pixels at the
// same placement. Scroll/viewport changes cannot inherit clipped edge tiles.
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
        Primitive::Quad(_) | Primitive::Shadow(_) | Primitive::Underline(_)
        | Primitive::PolychromeSprite(_) => {}
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
    /// Source clip/placement at capture. No offscreen coverage is implied.
    viewport: ContentMask<Pixels>,
    offset: LayerScrollPosition,
    pub(crate) content: Arc<LayerContent>,
}

impl LayerFrame {
    /// Only adjacent generations can inherit an exact unchanged tile.
    /// Rectangles are signed device pixels in the original source space.
    pub(crate) fn adopt_tile(&self, generation: u64, rect: [i64; 4], scale: f32) -> Option<u64> {
        if generation == self.generation {
            return Some(generation);
        }
        if generation.checked_add(1) != Some(self.generation) || !scale.is_finite() || scale <= 0. {
            return None;
        }
        let viewport = self.viewport.scale(scale);
        let bounds = viewport.bounds;
        let [left, top, right, bottom] = rect;
        // Partial tiles at a viewport edge do not prove unpainted pixels.
        if viewport.corner_radii != Default::default()
            || (left as f64) < f64::from(bounds.left().0)
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
            let (id, generation, content, damage) = if let Some(previous) = previous {
                if previous.viewport == record.viewport && previous.offset == record.offset
                    && previous.content.matches(&content)
                {
                    // Equal pixels need not share their backing pages. Do
                    // not pin unrelated historical window operations.
                    let shared = previous.content.operations.matches_by(&content.operations, |_, _| false);
                    let content = if shared { Arc::clone(&previous.content) } else { Arc::new(content) };
                    // Preserve the last transition proof if the GPU owner
                    // has not yet consumed it.
                    (previous.id, previous.generation, content, previous.damage)
                } else if let Some(next) = previous.generation.checked_add(1) {
                    let damage = if previous.viewport == record.viewport && previous.offset == record.offset {
                        content.damage_from(&previous.content)
                    } else {
                        LayerDamage::Full
                    };
                    (previous.id, next, Arc::new(content), damage)
                } else {
                    (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full)
                }
            } else {
                (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content), LayerDamage::Full)
            };
            frames.push(LayerFrame {
                layout_key: key, id, generation, damage, viewport: record.viewport,
                offset: record.offset, content,
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
            offset: LayerScrollPosition::Pixels(point(crate::px(0.), crate::px(0.))),
            content: Arc::new(LayerContent { operations: Operations::default(), parts: Box::new([]) }),
        };
        assert_eq!(frame.adopt_tile(3, [0, 0, 512, 512], 1.), Some(4));
        assert_eq!(frame.adopt_tile(3, [512, 0, 1024, 512], 1.), None);
        assert_eq!(frame.adopt_tile(3, [-1, 0, 511, 512], 1.), None);
        assert_eq!(frame.adopt_tile(2, [0, 0, 512, 512], 1.), None);
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

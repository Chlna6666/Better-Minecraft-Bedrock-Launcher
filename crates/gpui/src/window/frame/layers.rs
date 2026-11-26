//! Immutable scroll content publication, adapted from GPUI Fast's LayerContent/LayerPart.
//!
//! The current painter still culls to the viewport. These fragments therefore
//! certify only their original placement, not overscan or scroll translation.
//! Keep the ordinary drawing stream until the renderer can consume a complete
//! content-space capture.
use super::{LayerRecord, LayerScrollPosition};
use crate::{ContentMask, Pixels, scene::{Operations, PaintOperation, Scene}};
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

    fn matches(&self, other: &Self) -> bool {
        self.parts == other.parts
            && self.operations.matches_by(&other.operations, PaintOperation::same_paint)
    }
}

pub(crate) struct LayerFrame {
    layout_key: u64,
    /// Stable within one mounted lifetime; not a transient HitboxId.
    pub(crate) id: u64,
    pub(crate) generation: u64,
    /// Source clip/placement at capture. No offscreen coverage is implied.
    viewport: ContentMask<Pixels>,
    offset: LayerScrollPosition,
    pub(crate) content: Arc<LayerContent>,
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
            let (id, generation, content) = if let Some(previous) = previous {
                if previous.viewport == record.viewport && previous.offset == record.offset
                    && previous.content.matches(&content)
                {
                    (previous.id, previous.generation, Arc::clone(&previous.content))
                } else if let Some(next) = previous.generation.checked_add(1) {
                    (previous.id, next, Arc::new(content))
                } else {
                    (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content))
                }
            } else {
                (NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed), 1, Arc::new(content))
            };
            frames.push(LayerFrame {
                layout_key: key, id, generation, viewport: record.viewport,
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

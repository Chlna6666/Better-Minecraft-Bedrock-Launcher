//! Physical ordered operation pages: compact tags plus typed payloads.
//! Indices are local to their immutable owner, not to the Scene or frame.
use super::*;
use crate::{Bounds, ScaledPixels};
use super::primitive_stream::Ordered;
use std::{mem::size_of, ops::{Index, Range}, sync::Arc};

/// Empty typed pages need only one nullable pointer. A nonempty page can
/// publish its actual allocation to the sorted draw stream without retaining
/// the operation page or allocating another primitive vector.
#[derive(Clone)]
struct TypedPayload<T>(Option<Arc<Vec<T>>>);

impl<T> Default for TypedPayload<T> {
    fn default() -> Self { Self(None) }
}

impl<T> TypedPayload<T> {
    fn len(&self) -> usize { self.0.as_ref().map_or(0, |v| v.len()) }
    fn capacity(&self) -> usize { self.0.as_ref().map_or(0, |v| v.capacity()) }
    fn push(&mut self, value: T) {
        Arc::get_mut(self.0.get_or_insert_with(|| Arc::new(Vec::new())))
            .expect("mutable operation payload must be unique")
            .push(value);
    }
    fn clear(&mut self) {
        if let Some(owner) = &mut self.0
            && let Some(values) = Arc::get_mut(owner)
        {
            values.clear();
        } else {
            self.0 = None;
        }
    }
    fn shrink_to(&mut self, floor: usize) {
        if let Some(owner) = &mut self.0
            && let Some(values) = Arc::get_mut(owner)
        {
            values.shrink_to(floor);
        }
    }
}

impl<T> Index<usize> for TypedPayload<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        &self.0.as_ref().expect("nonempty typed payload")[index]
    }
}

impl<T: Ordered + PartialEq> TypedPayload<T> {
    /// Directly share sorted physical ranges, including a logical permutation
    /// when the sorted slots can be represented by a bounded number of runs.
    fn share_sorted_run(
        &self,
        indices: &[OperationIndex],
        kind: u32,
        stream: &mut PrimitiveStream<T>,
    ) -> bool {
        let Some(owner) = &self.0 else { return false };
        let Some(first) = indices.first() else { return false };
        let start = first.slot();
        if indices.iter().enumerate().all(|(i, index)| {
            index.kind() == kind && index.slot() == start + i
        }) {
            let range = start..start + indices.len();
            if owner[range.clone()].is_sorted_by_key(T::sort_key) {
                stream.share_sorted_page(owner, range);
                return true;
            }
        }

        // The logical sequence may be permuted relative to the already
        // sorted physical page. Sort narrow operation slots, not primitives.
        // Preserve logical insertion order for equal keys.
        if indices.iter().any(|index| index.kind() != kind) {
            return false;
        }
        let mut slots: Vec<_> = indices.iter().map(|index| index.slot()).collect();
        slots.sort_by_key(|&slot| owner[slot].sort_key());
        let mut ranges = smallvec::SmallVec::<[Range<usize>; 8]>::new();
        // Bound descriptor and encoder fragmentation, not just payload bytes.
        // A rejected candidate has not mutated the destination stream.
        let max_runs = (slots.len() / 4).max(8).min(
            (slots.len().saturating_mul(size_of::<T>()) / (4 * size_of::<usize>())).max(1),
        );
        for slot in slots {
            if let Some(last) = ranges.last_mut()
                && last.end == slot
            {
                last.end += 1;
            } else {
                if ranges.len() >= max_runs { return false; }
                ranges.push(slot..slot + 1);
            }
        }
        for range in ranges {
            stream.share_sorted_page(owner, range);
        }
        true
    }

    /// Only a fresh, exclusively owned page whose insertion sequence agrees
    /// exactly with the draw builder can be unified. Root blur capture mirrors
    /// and synthetic draw streams are deliberately not inferred from counts.
    fn publish_sorted(
        &mut self,
        indices: &mut [OperationIndex],
        kind: u32,
        stream: &mut PrimitiveStream<T>,
    ) {
        let Some(owner) = &mut self.0 else { return };
        let Some(values) = Arc::get_mut(owner) else { return };
        if values.is_empty() || !stream.matches_fresh_pending(values) {
            return;
        }

        if !values.is_sorted_by_key(T::sort_key) {
            // Sort narrow slot indices rather than moving large primitives
            // through the sort. Equal keys keep their insertion order.
            let mut sorted: Vec<usize> = (0..values.len()).collect();
            sorted.sort_unstable_by_key(|&slot| (values[slot].sort_key(), slot));
            let mut target = vec![0usize; values.len()];
            for (new, old) in sorted.into_iter().enumerate() {
                target[old] = new;
            }
            for index in indices {
                if index.kind() == kind {
                    *index = OperationIndex::new(kind, target[index.slot()]);
                }
            }
            // Apply the old-to-new permutation in place. Operation indices
            // were remapped above, before the scratch permutation is consumed.
            for old in 0..target.len() {
                while target[old] != old {
                    let new = target[old];
                    values.swap(old, new);
                    target.swap(old, new);
                }
            }
        }
        stream.adopt_fresh_sorted(Arc::clone(owner));
    }
}

#[repr(transparent)]
#[derive(Clone, Copy)]
struct OperationIndex(u32);

impl OperationIndex {
    const SLOT_MASK: u32 = (1 << 28) - 1;
    fn new(kind: u32, slot: usize) -> Self {
        let slot = u32::try_from(slot).expect("paint page slot overflow");
        assert!(slot <= Self::SLOT_MASK, "paint page slot overflow");
        Self((kind << 28) | slot)
    }

    fn kind(self) -> u32 { self.0 >> 28 }
    fn slot(self) -> usize { (self.0 & Self::SLOT_MASK) as usize }
}

/// All payload vectors are contiguous. Empty kinds allocate no payload.
/// Sharing a logical range retains its page owner and its stable local slots.
#[derive(Clone, Default)]
pub(super) struct OperationPage {
    indices: Vec<OperationIndex>,
    // A compacted logical page may reference only some slots of an original
    // typed allocation. Dense builders need no per-operation coverage walk.
    sparse_typed: u8,
    shadows: TypedPayload<Shadow>,
    quads: TypedPayload<Quad>,
    paths: Vec<Path<ScaledPixels>>,
    underlines: Vec<Underline>,
    mono: TypedPayload<MonochromeSprite>,
    poly: Vec<PolychromeSprite>,
    surfaces: Vec<PaintSurface>,
    backdrop: Vec<PaintBackdropBlur>,
    blurs: Vec<PaintBlur>,
    extensions: Vec<PaintRendererExtension>,
    scrolls: Vec<crate::window::scroll_paint::ScrollPlacement>,
    layers: Vec<Bounds<ScaledPixels>>,
    captures: Vec<BlurCapture>,
}

impl OperationPage {
    pub(super) fn len(&self) -> usize { self.indices.len() }
    pub(super) fn is_empty(&self) -> bool { self.indices.is_empty() }
    pub(super) fn capacity(&self) -> usize { self.indices.capacity() }

    /// Publication preserves logical operation order while draw streams borrow
    /// the very same typed allocations in physical sorted order.
    pub(super) fn publish_fresh_draw_payloads(
        &mut self,
        shadows: &mut PrimitiveStream<Shadow>,
        quads: &mut PrimitiveStream<Quad>,
        mono: &mut PrimitiveStream<MonochromeSprite>,
    ) {
        self.shadows.publish_sorted(&mut self.indices, 0, shadows);
        self.quads.publish_sorted(&mut self.indices, 1, quads);
        self.mono.publish_sorted(&mut self.indices, 4, mono);
    }

    pub(super) fn share_quad_run(
        &self, range: Range<usize>, stream: &mut PrimitiveStream<Quad>,
    ) -> bool {
        self.quads.share_sorted_run(&self.indices[range], 1, stream)
    }

    pub(super) fn share_mono_run(
        &self, range: Range<usize>, stream: &mut PrimitiveStream<MonochromeSprite>,
    ) -> bool {
        self.mono.share_sorted_run(&self.indices[range], 4, stream)
    }

    pub(super) fn capacity_bytes(&self) -> usize {
        self.indices.capacity().saturating_mul(size_of::<OperationIndex>())
            .saturating_add(self.shadows.capacity().saturating_mul(size_of::<Shadow>()))
            .saturating_add(self.quads.capacity().saturating_mul(size_of::<Quad>()))
            .saturating_add(self.paths.capacity().saturating_mul(size_of::<Path<ScaledPixels>>()))
            .saturating_add(self.underlines.capacity().saturating_mul(size_of::<Underline>()))
            .saturating_add(self.mono.capacity().saturating_mul(size_of::<MonochromeSprite>()))
            .saturating_add(self.poly.capacity().saturating_mul(size_of::<PolychromeSprite>()))
            .saturating_add(self.surfaces.capacity().saturating_mul(size_of::<PaintSurface>()))
            .saturating_add(self.backdrop.capacity().saturating_mul(size_of::<PaintBackdropBlur>()))
            .saturating_add(self.blurs.capacity().saturating_mul(size_of::<PaintBlur>()))
            .saturating_add(self.extensions.capacity().saturating_mul(size_of::<PaintRendererExtension>()))
            .saturating_add(self.scrolls.capacity().saturating_mul(size_of::<crate::window::scroll_paint::ScrollPlacement>()))
            .saturating_add(self.layers.capacity().saturating_mul(size_of::<Bounds<ScaledPixels>>()))
            .saturating_add(self.captures.capacity().saturating_mul(size_of::<BlurCapture>()))
    }

    pub(super) fn clear(&mut self) {
        self.indices.clear();
        self.sparse_typed = 0;
        self.shadows.clear();
        self.quads.clear();
        self.paths.clear();
        self.underlines.clear();
        self.mono.clear();
        self.poly.clear();
        self.surfaces.clear();
        self.backdrop.clear();
        self.blurs.clear();
        self.extensions.clear();
        self.scrolls.clear();
        self.layers.clear();
        self.captures.clear();
    }

    pub(super) fn shrink_to(&mut self, floor: usize) {
        self.indices.shrink_to(floor);
        self.shadows.shrink_to(floor.min(self.shadows.capacity()));
        self.quads.shrink_to(floor.min(self.quads.capacity()));
        self.paths.shrink_to(floor.min(self.paths.capacity()));
        self.underlines.shrink_to(floor.min(self.underlines.capacity()));
        self.mono.shrink_to(floor.min(self.mono.capacity()));
        self.poly.shrink_to(floor.min(self.poly.capacity()));
        self.surfaces.shrink_to(floor.min(self.surfaces.capacity()));
        self.backdrop.shrink_to(floor.min(self.backdrop.capacity()));
        self.blurs.shrink_to(floor.min(self.blurs.capacity()));
        self.extensions.shrink_to(floor.min(self.extensions.capacity()));
        self.scrolls.shrink_to(floor.min(self.scrolls.capacity()));
        self.layers.shrink_to(floor.min(self.layers.capacity()));
        self.captures.shrink_to(floor.min(self.captures.capacity()));
    }

    pub(super) fn push(&mut self, operation: PaintOperation) {
        macro_rules! store {
            ($kind:expr, $field:ident, $value:expr) => {{
                let index = OperationIndex::new($kind, self.$field.len());
                self.$field.push($value);
                index
            }};
        }
        let index = match operation {
            PaintOperation::Primitive(Primitive::Shadow(value)) => store!(0, shadows, value),
            PaintOperation::Primitive(Primitive::Quad(value)) => store!(1, quads, value),
            PaintOperation::Primitive(Primitive::Path(value)) => store!(2, paths, value),
            PaintOperation::Primitive(Primitive::Underline(value)) => store!(3, underlines, value),
            PaintOperation::Primitive(Primitive::MonochromeSprite(value)) => store!(4, mono, value),
            PaintOperation::Primitive(Primitive::PolychromeSprite(value)) => store!(5, poly, value),
            PaintOperation::Primitive(Primitive::Surface(value)) => store!(6, surfaces, value),
            PaintOperation::Primitive(Primitive::BackdropBlur(value)) => store!(7, backdrop, value),
            PaintOperation::Primitive(Primitive::Blur(value)) => store!(8, blurs, value),
            PaintOperation::Primitive(Primitive::RendererExtension(value)) => store!(9, extensions, value),
            PaintOperation::DeferredScroll(value) => store!(10, scrolls, value),
            PaintOperation::StartLayer(value) => store!(11, layers, value),
            PaintOperation::StartBlur(value) => store!(12, captures, value),
            PaintOperation::EndLayer => OperationIndex::new(13, 0),
            PaintOperation::EndBlur => OperationIndex::new(14, 0),
        };
        self.indices.push(index);
    }

    pub(super) fn view(&self, index: usize) -> PaintOperationRef<'_> {
        let index = self.indices[index];
        let slot = index.slot();
        match index.kind() {
            0 => PaintOperationRef::Primitive(PrimitiveRef::Shadow(&self.shadows[slot])),
            1 => PaintOperationRef::Primitive(PrimitiveRef::Quad(&self.quads[slot])),
            2 => PaintOperationRef::Primitive(PrimitiveRef::Path(&self.paths[slot])),
            3 => PaintOperationRef::Primitive(PrimitiveRef::Underline(&self.underlines[slot])),
            4 => PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(&self.mono[slot])),
            5 => PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(&self.poly[slot])),
            6 => PaintOperationRef::Primitive(PrimitiveRef::Surface(&self.surfaces[slot])),
            7 => PaintOperationRef::Primitive(PrimitiveRef::BackdropBlur(&self.backdrop[slot])),
            8 => PaintOperationRef::Primitive(PrimitiveRef::Blur(&self.blurs[slot])),
            9 => PaintOperationRef::Primitive(PrimitiveRef::RendererExtension(&self.extensions[slot])),
            10 => PaintOperationRef::DeferredScroll(&self.scrolls[slot]),
            11 => PaintOperationRef::StartLayer(&self.layers[slot]),
            12 => PaintOperationRef::StartBlur(&self.captures[slot]),
            13 => PaintOperationRef::EndLayer,
            14 => PaintOperationRef::EndBlur,
            _ => unreachable!("valid operation tag"),
        }
    }

    #[cfg(test)]
    pub(super) fn range_identity(&self, range: Range<usize>) -> *const () {
        self.indices[range].as_ptr().cast()
    }
    
    /// Construct only the requested sparse source range, preserving order.
    pub(super) fn append_range(&mut self, source: &Self, range: Range<usize>) {
        self.indices.reserve(range.len());
        for i in range {
            self.push(source.view(i).to_operation());
        }
    }

    /// Compact logical indices without independently copying draw payload.
    /// All ranges appended to this destination belong to one source page.
    pub(super) fn append_shared_range(&mut self, source: &Self, range: Range<usize>) {
        self.indices.reserve(range.len());
        for i in range {
            let index = source.indices[i];
            macro_rules! retain {
                ($field:ident) => {{
                    if self.$field.0.is_none() {
                        self.$field.0 = source.$field.0.clone();
                    }
                    self.sparse_typed |= 1 << index.kind();
                    self.indices.push(index);
                }};
            }
            match index.kind() {
                0 => retain!(shadows),
                1 => retain!(quads),
                4 => retain!(mono),
                _ => self.push(source.view(i).to_operation()),
            }
        }
    }

    /// Fast unique-page scope mutation. The index and typed payload address
    /// are unchanged; no other primitive is visited or cloned.
    pub(super) fn clip_layer(&mut self, index: usize, tight: Bounds<ScaledPixels>) {
        let index = self.indices[index];
        debug_assert_eq!(index.kind(), 11);
        let bounds = &mut self.layers[index.slot()];
        *bounds = bounds.intersect(&tight);
    }

    pub(super) fn set_blur_bounds(&mut self, index: usize, tight: Bounds<ScaledPixels>) {
        let index = self.indices[index];
        debug_assert_eq!(index.kind(), 12);
        self.captures[index.slot()].bounds = tight;
    }
}

/// Typed coverage is shared by logical Paint and sorted draw ranges.
pub(super) trait DrawPayload<T: Ordered> {
    fn draw_owner(&self) -> Option<&Arc<Vec<T>>>;
    fn has_sparse_draw_coverage(&self) -> bool;
    fn draw_coverage<'a>(&'a self, out: &mut Vec<(&'a Arc<Vec<T>>, Range<usize>)>);
    fn remap_draw(&mut self, page: &super::page_compaction::CompactedPage<Vec<T>>);
}

macro_rules! draw_payload {
    ($ty:ty, $field:ident, $tag:expr) => {
        impl DrawPayload<$ty> for OperationPage {
            fn draw_owner(&self) -> Option<&Arc<Vec<$ty>>> {
                self.$field.0.as_ref()
            }

            fn has_sparse_draw_coverage(&self) -> bool {
                self.sparse_typed & (1 << $tag) != 0
            }

            fn draw_coverage<'a>(&'a self, out: &mut Vec<(&'a Arc<Vec<$ty>>, Range<usize>)>) {
                let Some(owner) = self.$field.0.as_ref() else { return };
                if self.sparse_typed & (1 << $tag) == 0 {
                    if !owner.is_empty() {
                        out.push((owner, 0..owner.len()));
                    }
                    return;
                }
                for index in &self.indices {
                    if index.kind() != $tag { continue; }
                    let slot = index.slot();
                    if let Some((previous, range)) = out.last_mut()
                        && Arc::ptr_eq(previous, owner)
                        && range.end == slot
                    {
                        range.end += 1;
                    } else {
                        out.push((owner, slot..slot + 1));
                    }
                }
            }

            fn remap_draw(&mut self, page: &super::page_compaction::CompactedPage<Vec<$ty>>) {
                let mut count = 0;
                for index in &mut self.indices {
                    if index.kind() == $tag {
                        let slot = index.slot();
                        *index = OperationIndex::new($tag, page.remap(slot..slot + 1).start);
                        count += 1;
                    }
                }
                self.$field.0 = Some(Arc::clone(&page.owner));
                // Every source slot appears at most once per operation page.
                if count == page.owner.len() {
                    self.sparse_typed &= !(1 << $tag);
                }
            }
        }
    };
}

draw_payload!(Shadow, shadows, 0);
draw_payload!(Quad, quads, 1);
draw_payload!(MonochromeSprite, mono, 4);

impl super::page_compaction::PagePayload for OperationPage {
    fn len(&self) -> usize { self.len() }
    fn with_capacity(len: usize) -> Self {
        Self { indices: Vec::with_capacity(len), ..Self::default() }
    }
    fn append_range(&mut self, source: &Self, range: Range<usize>) {
        self.append_range(source, range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorted_draw_payload_is_the_same_allocation_as_logical_operations() {
        let mut page = OperationPage::default();
        let mut quads = PrimitiveStream::default();
        for (order, x) in [(3, 10.), (1, 20.), (1, 30.), (2, 40.)] {
            let mut quad = Quad { order, ..Quad::default() };
            quad.bounds.origin.x = ScaledPixels(x);
            quads.push(quad);
            page.push(PaintOperation::Primitive(Primitive::Quad(quad)));
            page.push(PaintOperation::EndLayer);
        }
        page.publish_fresh_draw_payloads(
            &mut PrimitiveStream::default(), &mut quads, &mut PrimitiveStream::default(),
        );
        quads.finish();
        let logical_to_sorted = [3, 0, 1, 2];
        for (logical, sorted) in logical_to_sorted.into_iter().enumerate() {
            let PaintOperationRef::Primitive(PrimitiveRef::Quad(quad)) = page.view(logical * 2) else {
                panic!("expected quad");
            };
            assert!(std::ptr::eq(quad, &quads[sorted]));
            assert_eq!(quad.bounds.origin.x, ScaledPixels(10. * (logical + 1) as f32));
            assert!(matches!(page.view(logical * 2 + 1), PaintOperationRef::EndLayer));
        }
        assert_eq!(quads.slices(0..4).count(), 1);
        page.clear();
        assert_eq!(quads[0].bounds.origin.x, ScaledPixels(20.));
    }

    #[test]
    fn replay_merges_sorted_views_without_joining_a_descending_boundary() {
        let mut page = OperationPage::default();
        let mut quads = PrimitiveStream::default();
        for order in [2, 3, 1, 4] {
            page.push(PaintOperation::Primitive(Primitive::Quad(Quad { order, ..Quad::default() })));
        }
        let mut permuted = PrimitiveStream::default();
        assert!(page.share_quad_run(0..4, &mut permuted));
        permuted.finish();
        assert_eq!(permuted.iter().map(|quad| quad.order).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(page.share_quad_run(0..2, &mut quads));
        assert!(page.share_quad_run(2..4, &mut quads));
        quads.finish();
        assert_eq!(quads.iter().map(|quad| quad.order).collect::<Vec<_>>(), [1, 2, 3, 4]);
        for (logical, sorted) in [1, 2, 0, 3].into_iter().enumerate() {
            let PaintOperationRef::Primitive(PrimitiveRef::Quad(quad)) = page.view(logical) else {
                panic!("expected quad");
            };
            assert!(std::ptr::eq(quad, &quads[sorted]));
            assert!(std::ptr::eq(quad, &permuted[sorted]));
        }
    }

    #[test]
    fn different_draw_payload_does_not_adopt_operation_allocation() {
        let mut page = OperationPage::default();
        let mut quads = PrimitiveStream::default();
        page.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
        quads.push(Quad { order: 1, ..Quad::default() });
        page.publish_fresh_draw_payloads(
            &mut PrimitiveStream::default(), &mut quads, &mut PrimitiveStream::default(),
        );
        quads.finish();
        let PaintOperationRef::Primitive(PrimitiveRef::Quad(quad)) = page.view(0) else {
            panic!("expected quad");
        };
        assert!(!std::ptr::eq(quad, &quads[0]));
        assert_eq!(quads[0].order, 1);
    }

    #[test]
    fn index_is_four_bytes_and_markers_own_no_primitive_slots() {
        assert_eq!(size_of::<OperationIndex>(), 4);
        let mut page = OperationPage::default();
        page.push(PaintOperation::EndLayer);
        page.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
        page.push(PaintOperation::EndBlur);
        assert_eq!(page.indices.len(), 3);
        assert_eq!(page.quads.len(), 1);
        assert_eq!(page.shadows.capacity(), 0);
        assert!(matches!(page.view(0), PaintOperationRef::EndLayer));
        assert!(matches!(page.view(1), PaintOperationRef::Primitive(PrimitiveRef::Quad(_))));
        assert!(matches!(page.view(2), PaintOperationRef::EndBlur));
        assert_eq!(OperationIndex::new(14, OperationIndex::SLOT_MASK as usize).slot(),
            OperationIndex::SLOT_MASK as usize);
    }
}

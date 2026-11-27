//! Physical ordered operation pages: compact tags plus typed payloads.
//! Indices are local to their immutable owner, not to the Scene or frame.
use super::*;
use crate::{Bounds, ScaledPixels};
use std::{mem::size_of, ops::Range};

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
#[derive(Default)]
pub(super) struct OperationPage {
    indices: Vec<OperationIndex>,
    shadows: Vec<Shadow>,
    quads: Vec<Quad>,
    paths: Vec<Path<ScaledPixels>>,
    underlines: Vec<Underline>,
    mono: Vec<MonochromeSprite>,
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

use super::*;
use smallvec::SmallVec;
use std::{ops::Range, sync::Arc};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ChunkUploadPlan {
    None,
    Full,
    // Common one-to-four dirty gaps stay inline; many disjoint chunks may
    // spill without losing a single byte or reordering backend writes.
    Ranges(SmallVec<[Range<usize>; 4]>),
}

impl ChunkUploadPlan {
    pub(super) fn uploaded_bytes(&self, full_len: usize) -> usize {
        match self {
            Self::None => 0,
            Self::Full => full_len,
            Self::Ranges(ranges) => ranges.iter().map(Range::len).sum(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct RetainedChunkLayout(Arc<[RetainedResidentSpan]>);

impl RetainedChunkLayout {
    pub(super) fn from_spans(spans: &[RetainedResidentSpan]) -> Self {
        // Construct the immutable resident descriptor directly from the slice. The
        // old Vec::clone() -> Arc conversion allocated a temporary Vec and
        // cloned every chunk identity a second time on layout changes.
        Self(Arc::from(spans))
    }

    /// A new Scene revision does not imply a new resident chunk layout. Reuse
    /// the immutable descriptor allocation whenever its exact ordered spans
    /// are unchanged, including their chunk generation and content hash.
    pub(super) fn from_spans_reusing(previous: &Self, spans: &[RetainedResidentSpan]) -> Self {
        if previous.0.as_ref() == spans {
            return previous.clone();
        }
        Self::from_spans(spans)
    }

    pub(super) fn upload_plan(
        &self,
        previous: Option<&Self>,
        byte_len: usize,
        stream_dirty: bool,
    ) -> ChunkUploadPlan {
        if !stream_dirty || byte_len == 0 {
            return ChunkUploadPlan::None;
        }
        let Some(previous) = previous else {
            return ChunkUploadPlan::Full;
        };
        // The GPU owner intentionally shares one Arc when the ordered resident
        // layout has not changed. In that common case the old and new layout
        // are the *same* allocation: dirty_complement performs all necessary
        // ordering/bounds validation and finds gaps in one pass. Do not scan
        // and compare both copies of identical metadata on every slot update.
        if Arc::ptr_eq(&self.0, &previous.0) {
            return dirty_complement(self.0.iter(), byte_len).unwrap_or(ChunkUploadPlan::Full);
        }
        // Retained ranges are sorted by GPU byte offset, never by Chunk ID.
        // Validate BOTH generations before omitting any resident bytes:
        // a malformed current layout could otherwise preserve a coincidentally
        // matching old span while hiding overlapping/out-of-bounds new bytes.
        // The old buffer may be larger than the new one after a resize, so its
        // ranges are checked for ordering but not against the new byte length.
        if !resident_spans_are_ordered(&previous.0, None)
            || !resident_spans_are_ordered(&self.0, Some(byte_len))
        {
            return ChunkUploadPlan::Full;
        }
        let mut old = previous.0.iter().peekable();
        let clean = self.0.iter().filter(|span| {
            while old
                .peek()
                .is_some_and(|candidate| candidate.range.start < span.range.start)
            {
                old.next();
            }
            old.peek().is_some_and(|candidate| *candidate == *span)
        });
        dirty_complement(clean, byte_len).unwrap_or(ChunkUploadPlan::Full)
    }
}

#[derive(Clone, Default)]
pub(super) struct RetainedChunkLayouts {
    pub(super) quads: RetainedChunkLayout,
    pub(super) monochrome_sprites: RetainedChunkLayout,
}

impl RetainedChunkLayouts {
    pub(super) fn from_upload_reusing(previous: &Self, upload: &FrameUpload) -> Self {
        Self {
            quads: RetainedChunkLayout::from_spans_reusing(
                &previous.quads,
                &upload.resident_quad_spans,
            ),
            monochrome_sprites: RetainedChunkLayout::from_spans_reusing(
                &previous.monochrome_sprites,
                &upload.resident_monochrome_sprite_spans,
            ),
        }
    }
}

/// Reject malformed retained metadata before a partial GPU buffer update.
/// Validation stays allocation-free; full upload is the conservative fallback.
fn resident_spans_are_ordered(spans: &[RetainedResidentSpan], byte_len: Option<usize>) -> bool {
    let mut end = 0usize;
    for span in spans {
        if span.range.is_empty()
            || span.range.start < end
            || byte_len.is_some_and(|len| span.range.end > len)
        {
            return false;
        }
        end = span.range.end;
    }
    true
}

fn dirty_complement<'a>(
    clean: impl Iterator<Item = &'a RetainedResidentSpan>,
    byte_len: usize,
) -> Option<ChunkUploadPlan> {
    let mut ranges = SmallVec::<[Range<usize>; 4]>::new();
    let mut cursor = 0usize;
    let mut retained_any = false;
    for span in clean {
        if span.range.start < cursor || span.range.end > byte_len || span.range.is_empty() {
            return None;
        }
        retained_any = true;
        if cursor < span.range.start {
            ranges.push(cursor..span.range.start);
        }
        cursor = span.range.end;
    }
    if !retained_any {
        return None;
    }
    if cursor < byte_len {
        ranges.push(cursor..byte_len);
    }
    if ranges.is_empty() {
        // A changed upload token may describe only animation topology while
        // every physical Quad byte remains resident. Avoid a zero-write plan.
        Some(ChunkUploadPlan::None)
    } else {
        Some(ChunkUploadPlan::Ranges(ranges))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(name: &'static str, generation: u64, range: Range<usize>) -> RetainedResidentSpan {
        RetainedResidentSpan {
            id: RetainedChunkId::new(
                crate::GlobalElementId::from_path(&[name.into()]),
                generation,
            ),
            range,
            byte_hash: generation,
        }
    }

    #[test]
    fn unchanged_chunk_skips_its_resident_range() {
        let current = RetainedChunkLayout(Arc::from([span("chunk", 1, 16..48)]));
        let previous = current.clone();

        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Ranges(smallvec::smallvec![0..16, 48..64])
        );
    }

    #[test]
    fn identical_arc_layout_uses_single_validated_gap_walk() {
        let original = RetainedChunkLayout(Arc::from([
            span("left", 1, 16..32),
            span("right", 1, 48..64),
        ]));
        let reused = original.clone();
        assert!(Arc::ptr_eq(&original.0, &reused.0));
        assert_eq!(
            reused.upload_plan(Some(&original), 80, true),
            ChunkUploadPlan::Ranges(smallvec::smallvec![0..16, 32..48, 64..80])
        );
        // A deep-copied but equivalent descriptor must have the same plan.
        let independent = RetainedChunkLayout(Arc::from(original.0.as_ref()));
        assert!(!Arc::ptr_eq(&original.0, &independent.0));
        assert_eq!(
            independent.upload_plan(Some(&original), 80, true),
            reused.upload_plan(Some(&original), 80, true)
        );
    }

    #[test]
    fn identical_arc_with_complete_residency_needs_no_quad_upload() {
        let layout = RetainedChunkLayout(Arc::from([span("whole", 2, 0..64)]));
        assert_eq!(
            layout.upload_plan(Some(&layout), 64, true),
            ChunkUploadPlan::None
        );
        assert_eq!(
            layout.upload_plan(Some(&layout), 64, false),
            ChunkUploadPlan::None
        );
    }

    #[test]
    fn malformed_shared_layout_still_forces_full_upload() {
        let invalid_layouts = [
            RetainedChunkLayout(Arc::from([
                span("right", 1, 32..64),
                span("left", 1, 0..32),
            ])),
            RetainedChunkLayout(Arc::from([
                span("left", 1, 0..32),
                span("overlap", 1, 16..48),
            ])),
            RetainedChunkLayout(Arc::from([span("overflow", 1, 32..96)])),
            RetainedChunkLayout(Arc::from([span("empty", 1, 16..16)])),
        ];
        for layout in &invalid_layouts {
            let shared = layout.clone();
            assert!(Arc::ptr_eq(&layout.0, &shared.0));
            assert_eq!(
                shared.upload_plan(Some(layout), 64, true),
                ChunkUploadPlan::Full,
                "same-Arc reuse must not bypass range validation"
            );
        }
    }

    #[test]
    fn generation_or_position_change_falls_back_to_full_upload() {
        let previous = RetainedChunkLayout(Arc::from([span("chunk", 1, 16..48)]));
        let changed_generation = RetainedChunkLayout(Arc::from([span("chunk", 2, 16..48)]));
        let changed_position = RetainedChunkLayout(Arc::from([span("chunk", 1, 8..40)]));
        let mut changed_bytes = span("chunk", 1, 16..48);
        changed_bytes.byte_hash = 99;
        let changed_bytes = RetainedChunkLayout(Arc::from([changed_bytes]));

        assert_eq!(
            changed_generation.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Full
        );
        assert_eq!(
            changed_position.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Full
        );
        assert_eq!(
            changed_bytes.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Full
        );
    }

    #[test]
    fn uninitialized_slot_requires_full_upload() {
        let current = RetainedChunkLayout(Arc::from([span("chunk", 1, 16..48)]));

        assert_eq!(current.upload_plan(None, 64, true), ChunkUploadPlan::Full);
    }

    #[test]
    fn identical_layout_reuses_arc_and_changed_generation_creates_new_storage() {
        let mut upload = FrameUpload::default();
        upload.resident_quad_spans.push(span("chunk", 1, 0..32));
        let original = RetainedChunkLayout::from_spans(&upload.resident_quad_spans);
        let reused =
            RetainedChunkLayout::from_spans_reusing(&original, &upload.resident_quad_spans);
        assert!(Arc::ptr_eq(&original.0, &reused.0));

        upload.resident_quad_spans[0].byte_hash = 2;
        let changed =
            RetainedChunkLayout::from_spans_reusing(&original, &upload.resident_quad_spans);
        assert!(!Arc::ptr_eq(&original.0, &changed.0));
        assert_eq!(
            changed.upload_plan(Some(&original), 32, true),
            ChunkUploadPlan::Full
        );
    }

    #[test]
    fn unordered_previous_spans_fall_back_to_safe_full_upload() {
        let previous = RetainedChunkLayout(Arc::from([
            span("right", 1, 32..64),
            span("left", 1, 0..32),
        ]));
        let current = RetainedChunkLayout(Arc::from([
            span("left", 1, 0..32),
            span("right", 2, 32..64),
        ]));
        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Full
        );
    }

    #[test]
    fn malformed_current_ranges_never_skip_upload_using_matching_previous_span() {
        let previous = RetainedChunkLayout(Arc::from([
            span("first", 1, 0..32),
            span("second", 1, 32..64),
        ]));
        let current_reordered = RetainedChunkLayout(Arc::from([
            span("second", 1, 32..64),
            span("first", 1, 0..32),
        ]));
        let current_overlap = RetainedChunkLayout(Arc::from([
            span("first", 1, 0..32),
            span("overlap", 1, 16..48),
        ]));
        let current_out_of_bounds = RetainedChunkLayout(Arc::from([
            span("first", 1, 0..32),
            span("overflow", 1, 64..96),
        ]));
        for current in [&current_reordered, &current_overlap, &current_out_of_bounds] {
            assert_eq!(
                current.upload_plan(Some(&previous), 64, true),
                ChunkUploadPlan::Full
            );
        }
    }

    #[test]
    fn valid_previous_layout_can_exceed_shrinking_current_buffer_size() {
        let previous = RetainedChunkLayout(Arc::from([
            span("first", 1, 0..32),
            span("second", 1, 32..96),
        ]));
        let current = RetainedChunkLayout(Arc::from([span("first", 1, 0..32)]));
        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Ranges(smallvec::smallvec![32..64])
        );
    }

    #[test]
    fn common_dirty_ranges_do_not_allocate_a_heap_vector() {
        let current = RetainedChunkLayout(Arc::from([
            span("left", 1, 16..32),
            span("right", 1, 48..64),
        ]));
        let plan = current.upload_plan(Some(&current), 80, true);
        match plan {
            ChunkUploadPlan::Ranges(ranges) => {
                assert!(!ranges.spilled(), "three dirty gaps fit the inline storage");
                assert_eq!(ranges.as_slice(), &[0..16, 32..48, 64..80]);
                assert_eq!(ranges.iter().map(Range::len).sum::<usize>(), 48);
            }
            other => panic!("expected partial upload: {other:?}"),
        }
    }

    #[test]
    fn more_than_four_dirty_ranges_preserve_order_and_uploaded_bytes() {
        let current = RetainedChunkLayout(Arc::from([
            span("a", 1, 4..8),
            span("b", 1, 20..24),
            span("c", 1, 36..40),
            span("d", 1, 52..56),
            span("e", 1, 68..72),
            span("f", 1, 84..88),
        ]));
        let plan = current.upload_plan(Some(&current), 96, true);
        assert_eq!(plan.uploaded_bytes(96), 72);
        match plan {
            ChunkUploadPlan::Ranges(ranges) => {
                assert!(ranges.spilled(), "uncommon fragmented layout may grow");
                assert_eq!(
                    ranges.as_slice(),
                    &[0..4, 8..20, 24..36, 40..52, 56..68, 72..84, 88..96]
                );
            }
            other => panic!("expected partial upload: {other:?}"),
        }
    }

    #[test]
    fn dirty_chunk_does_not_upload_clean_sibling() {
        let previous = RetainedChunkLayout(Arc::from([
            span("left", 1, 0..32),
            span("right", 1, 32..64),
        ]));
        let current = RetainedChunkLayout(Arc::from([
            span("left", 2, 0..32),
            span("right", 1, 32..64),
        ]));

        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            ChunkUploadPlan::Ranges(smallvec::smallvec![0..32])
        );
    }
}

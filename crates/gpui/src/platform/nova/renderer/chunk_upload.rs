use super::*;
use std::{ops::Range, sync::Arc};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum QuadUploadPlan {
    None,
    Full,
    Ranges(Vec<Range<usize>>),
}

impl QuadUploadPlan {
    pub(super) fn uploaded_bytes(&self, full_len: usize) -> usize {
        match self {
            Self::None => 0,
            Self::Full => full_len,
            Self::Ranges(ranges) => ranges.iter().map(Range::len).sum(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct QuadResidentLayout(Arc<[RetainedResidentSpan]>);

impl QuadResidentLayout {
    pub(super) fn from_upload(upload: &FrameUpload) -> Self {
        Self(Arc::from(upload.resident_quad_spans.clone()))
    }

    /// A new Scene revision does not imply a new resident quad layout. Reuse
    /// the immutable descriptor allocation whenever its exact ordered spans
    /// are unchanged, including their chunk generation and content hash.
    pub(super) fn from_upload_reusing(previous: &Self, upload: &FrameUpload) -> Self {
        if previous.0.as_ref() == upload.resident_quad_spans.as_slice() {
            return previous.clone();
        }
        Self::from_upload(upload)
    }

    pub(super) fn upload_plan(
        &self,
        previous: Option<&Self>,
        byte_len: usize,
        stream_dirty: bool,
    ) -> QuadUploadPlan {
        if !stream_dirty || byte_len == 0 {
            return QuadUploadPlan::None;
        }
        let Some(previous) = previous else {
            return QuadUploadPlan::Full;
        };
        // Span ranges are emitted in GPU byte-offset order. A malformed old
        // layout must never make us skip an upload, so reject it before the
        // allocation-free ordered merge.
        if previous.0.iter().any(|span| span.range.is_empty())
            || previous
                .0
                .windows(2)
                .any(|pair| pair[0].range.end > pair[1].range.start)
        {
            return QuadUploadPlan::Full;
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
        dirty_complement(clean, byte_len).unwrap_or(QuadUploadPlan::Full)
    }
}

fn dirty_complement<'a>(
    clean: impl Iterator<Item = &'a RetainedResidentSpan>,
    byte_len: usize,
) -> Option<QuadUploadPlan> {
    let mut ranges = Vec::new();
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
    Some(QuadUploadPlan::Ranges(ranges))
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
        let current = QuadResidentLayout(Arc::from([span("chunk", 1, 16..48)]));
        let previous = current.clone();

        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            QuadUploadPlan::Ranges(vec![0..16, 48..64])
        );
    }

    #[test]
    fn generation_or_position_change_falls_back_to_full_upload() {
        let previous = QuadResidentLayout(Arc::from([span("chunk", 1, 16..48)]));
        let changed_generation = QuadResidentLayout(Arc::from([span("chunk", 2, 16..48)]));
        let changed_position = QuadResidentLayout(Arc::from([span("chunk", 1, 8..40)]));
        let mut changed_bytes = span("chunk", 1, 16..48);
        changed_bytes.byte_hash = 99;
        let changed_bytes = QuadResidentLayout(Arc::from([changed_bytes]));

        assert_eq!(
            changed_generation.upload_plan(Some(&previous), 64, true),
            QuadUploadPlan::Full
        );
        assert_eq!(
            changed_position.upload_plan(Some(&previous), 64, true),
            QuadUploadPlan::Full
        );
        assert_eq!(
            changed_bytes.upload_plan(Some(&previous), 64, true),
            QuadUploadPlan::Full
        );
    }

    #[test]
    fn uninitialized_slot_requires_full_upload() {
        let current = QuadResidentLayout(Arc::from([span("chunk", 1, 16..48)]));

        assert_eq!(current.upload_plan(None, 64, true), QuadUploadPlan::Full);
    }

    #[test]
    fn identical_layout_reuses_arc_and_changed_generation_creates_new_storage() {
        let mut upload = FrameUpload::default();
        upload.resident_quad_spans.push(span("chunk", 1, 0..32));
        let original = QuadResidentLayout::from_upload(&upload);
        let reused = QuadResidentLayout::from_upload_reusing(&original, &upload);
        assert!(Arc::ptr_eq(&original.0, &reused.0));

        upload.resident_quad_spans[0].byte_hash = 2;
        let changed = QuadResidentLayout::from_upload_reusing(&original, &upload);
        assert!(!Arc::ptr_eq(&original.0, &changed.0));
        assert_eq!(changed.upload_plan(Some(&original), 32, true), QuadUploadPlan::Full);
    }

    #[test]
    fn unordered_previous_spans_fall_back_to_safe_full_upload() {
        let previous = QuadResidentLayout(Arc::from([
            span("right", 1, 32..64),
            span("left", 1, 0..32),
        ]));
        let current = QuadResidentLayout(Arc::from([
            span("left", 1, 0..32),
            span("right", 2, 32..64),
        ]));
        assert_eq!(current.upload_plan(Some(&previous), 64, true), QuadUploadPlan::Full);
    }

    #[test]
    fn dirty_chunk_does_not_upload_clean_sibling() {
        let previous = QuadResidentLayout(Arc::from([
            span("left", 1, 0..32),
            span("right", 1, 32..64),
        ]));
        let current = QuadResidentLayout(Arc::from([
            span("left", 2, 0..32),
            span("right", 1, 32..64),
        ]));

        assert_eq!(
            current.upload_plan(Some(&previous), 64, true),
            QuadUploadPlan::Ranges(vec![0..32])
        );
    }
}

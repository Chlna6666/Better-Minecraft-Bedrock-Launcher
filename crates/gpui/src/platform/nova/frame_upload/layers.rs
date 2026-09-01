use super::*;
use smallvec::SmallVec;

/// One isolated element-blur content range in the flattened upload stream.
///
/// The range excludes both marker batches. Ranges are stored deepest first so a child blur can be
/// rendered and retained in its target before its parent source samples the child composite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::nova) struct BlurContentRange {
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) depth: usize,
    pub(in crate::platform::nova) content_start: usize,
    pub(in crate::platform::nova) content_end: usize,
}

impl FrameUpload {
    /// Rebuild the parsed blur marker topology after the static batch stream changes.
    ///
    /// Present, target planning and draw-step construction all query this topology in the same
    /// frame. Keeping it beside the retained batch stream prevents each consumer from reparsing the
    /// BeginBlur/EndBlur stack and allocating another temporary Vec.
    pub(in crate::platform::nova) fn refresh_blur_content_ranges(&mut self) {
        let ranges = &mut self.blur_content_ranges_cache;
        ranges.clear();
        let mut stack: SmallVec<[(u32, usize, usize); 4]> = SmallVec::new();

        for (batch_index, batch) in self.batches.iter().enumerate() {
            match *batch {
                UploadedBatch::BeginBlur { index } => {
                    stack.push((index, batch_index, stack.len()));
                }
                UploadedBatch::EndBlur { index } => {
                    let Some(&(open_index, begin, depth)) = stack.last() else {
                        continue;
                    };
                    if open_index != index {
                        continue;
                    }
                    stack.pop();
                    ranges.push(BlurContentRange {
                        index,
                        depth,
                        content_start: begin.saturating_add(1),
                        content_end: batch_index,
                    });
                }
                UploadedBatch::SolidQuads { .. }
                | UploadedBatch::Quads { .. }
                | UploadedBatch::Shadows { .. }
                | UploadedBatch::PathRasterization { .. }
                | UploadedBatch::Paths { .. }
                | UploadedBatch::MonoSprites { .. }
                | UploadedBatch::PolySprites { .. }
                | UploadedBatch::Underlines { .. }
                | UploadedBatch::BackdropBlurs { .. }
                | UploadedBatch::CompositeBlur { .. }
                | UploadedBatch::CustomMesh3d { .. } => {}
            }
        }

        ranges.sort_unstable_by_key(|range| (std::cmp::Reverse(range.depth), range.content_start));
    }

    #[inline]
    pub(in crate::platform::nova) fn blur_content_ranges(&self) -> &[BlurContentRange] {
        &self.blur_content_ranges_cache
    }

    #[inline]
    pub(in crate::platform::nova) fn has_element_blurs(&self) -> bool {
        !self.blur_content_ranges_cache.is_empty()
    }
}

use super::*;

/// One isolated element-blur content range in the flattened upload stream.
///
/// The range excludes both marker batches. Ranges are returned deepest first so a child blur can
/// be rendered and retained in its target before its parent source samples the child composite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::nova) struct BlurContentRange {
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) depth: usize,
    pub(in crate::platform::nova) content_start: usize,
    pub(in crate::platform::nova) content_end: usize,
}

impl FrameUpload {
    pub(in crate::platform::nova) fn blur_content_ranges(&self) -> Vec<BlurContentRange> {
        let mut stack = Vec::<(u32, usize, usize)>::new();
        let mut ranges = Vec::new();

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
        ranges
    }

    pub(in crate::platform::nova) fn has_element_blurs(&self) -> bool {
        self.batches
            .iter()
            .any(|batch| matches!(batch, UploadedBatch::BeginBlur { .. }))
    }
}

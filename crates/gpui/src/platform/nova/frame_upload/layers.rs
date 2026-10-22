use super::*;
use smallvec::SmallVec;

/// Window color work, separate from Element/Backdrop Filter topology.
/// `index` addresses the shared compositor shader ABI; it is not a Filter target identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::nova) struct RootColorPass {
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) clear_quad: u32,
    pub(in crate::platform::nova) content_end: usize,
}

impl RootColorPass {
    pub(in crate::platform::nova) fn content_range(self) -> ColorContentRange {
        ColorContentRange {
            index: self.index,
            depth: 0,
            content_start: 0,
            content_end: self.content_end,
        }
    }
}

/// One color-source range in the flattened upload stream.
///
/// Filter ranges exclude their marker batches and are sorted deepest first.
/// Root owns a separate explicit range; it executes after its child Filter outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::nova) struct ColorContentRange {
    pub(in crate::platform::nova) index: u32,
    pub(in crate::platform::nova) depth: usize,
    pub(in crate::platform::nova) content_start: usize,
    pub(in crate::platform::nova) content_end: usize,
}

impl FrameUpload {
    /// Rebuild static blur-derived topology/dependency caches after the flattened batch stream
    /// changes. Retained frames reuse these results without reparsing the same batches.
    pub(in crate::platform::nova) fn refresh_blur_content_ranges(&mut self) {
        // The encoder pushes a captured source immediately after allocating
        // its packed Blur index; recursive child indices and the synthetic
        // AutoStatic index are strictly increasing. Prove this once when the
        // flattened Scene changes, not on every presentation frame.
        debug_assert!(
            self.element_blur_inputs
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0)
        );
        {
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
                        ranges.push(ColorContentRange {
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
                    | UploadedBatch::RendererExtensions { .. } => {}
                }
            }

            ranges.sort_unstable_by_key(|range| {
                (std::cmp::Reverse(range.depth), range.content_start)
            });
        }

        // GPU target identity needs a stable order independent of nesting/depth order. Build it once
        // with the static topology instead of collect + sort + dedup in every compatibility check.
        self.isolated_blur_source_indices_cache.clear();
        self.isolated_blur_source_indices_cache.extend(
            self.blur_content_ranges_cache
                .iter()
                .map(|range| range.index),
        );
        self.isolated_blur_source_indices_cache.sort_unstable();
        self.isolated_blur_source_indices_cache.dedup();

        // Atlas dependency membership is also a pure function of the static flattened batch stream.
        // Refresh it beside the marker topology so retained frames never rescan the batch prefix.
        self.refresh_backdrop_textures();
    }

    #[inline]
    pub(in crate::platform::nova) fn blur_content_ranges(&self) -> &[ColorContentRange] {
        &self.blur_content_ranges_cache
    }

    #[inline]
    pub(in crate::platform::nova) fn isolated_blur_source_indices(&self) -> &[u32] {
        &self.isolated_blur_source_indices_cache
    }

    /// The packed GPU blur index is monotonic even when nested child ranges
    /// are ordered deepest-first for compositing. Look up the original source
    /// without rescanning every preceding filter for each retained layer.
    #[inline]
    pub(in crate::platform::nova) fn element_blur_source(
        &self,
        index: u32,
    ) -> Option<&FilterSource> {
        let position = self
            .element_blur_inputs
            .binary_search_by_key(&index, |(source_index, _)| *source_index)
            .ok()?;
        Some(&self.element_blur_inputs[position].1)
    }

    #[inline]
    pub(in crate::platform::nova) fn has_element_blurs(&self) -> bool {
        !self.blur_content_ranges_cache.is_empty()
    }
}

#[cfg(test)]
mod source_lookup_tests {
    use super::*;

    #[test]
    fn packed_blur_index_finds_sparse_sources_in_any_layer_depth_order() {
        let mut upload = FrameUpload::default();
        let rect = crate::bounds(
            crate::point(crate::ScaledPixels(2.0), crate::ScaledPixels(3.0)),
            crate::size(crate::ScaledPixels(16.0), crate::ScaledPixels(18.0)),
        );
        for index in [1u32, 4, 7, 13, 21] {
            upload.element_blur_inputs.push((
                index,
                FilterSource::AutoStatic {
                    bounds: rect,
                    content_mask: crate::ContentMask::new(rect),
                    content_hash: u64::from(index),
                    content_bytes: 24,
                },
            ));
        }
        for index in [21u32, 1, 13, 7, 4] {
            assert!(matches!(
                upload.element_blur_source(index),
                Some(FilterSource::AutoStatic { content_hash, .. })
                    if *content_hash == u64::from(index)
            ));
        }
        for missing in [0, 2, 5, 8, 14, 22, u32::MAX] {
            assert!(upload.element_blur_source(missing).is_none());
        }
    }

    #[test]
    fn empty_packed_blur_source_lookup_returns_none() {
        let upload = FrameUpload::default();
        assert!(upload.element_blur_source(0).is_none());
    }
}

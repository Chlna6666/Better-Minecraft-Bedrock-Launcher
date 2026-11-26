mod builders;
mod convert;
mod engine;
mod fingerprint;
mod key;
mod metrics;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fractional_tests;

#[cfg(test)]
mod placement_tests;

pub use builders::{absolute_fill, center, h_stack, relative_fill, v_stack};
pub(crate) use convert::snap_logical_to_device_pixel;
pub use engine::TaffyLayoutEngine;
pub(crate) use engine::{MeasurementKey, NativePlacementDelta, NativePlacementProof, SizeMeasurement};
use std::ops::Range;

/// References into the double-buffered native claim log, not copies of nodes.
/// Each lifecycle phase can own any number of disjoint blocks. The common
/// request/prepaint pair stays inline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LayoutClaimBlocks {
    blocks: smallvec::SmallVec<[Range<usize>; 2]>,
    request_count: usize,
}

impl LayoutClaimBlocks {
    pub(crate) fn request(range: Range<usize>) -> Self {
        let mut result = Self::default();
        result.push_request(range);
        result
    }

    pub(crate) fn prepaint(range: Range<usize>) -> Self {
        let mut result = Self::default();
        result.push_prepaint(range);
        result
    }

    pub(crate) fn request_blocks(&self) -> &[Range<usize>] {
        &self.blocks[..self.request_count]
    }

    pub(crate) fn prepaint_blocks(&self) -> &[Range<usize>] {
        &self.blocks[self.request_count..]
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Range<usize>> {
        self.blocks.iter()
    }

    pub(crate) fn push_request(&mut self, range: Range<usize>) {
        if range.is_empty() { return; }
        // New request blocks can follow an already adopted prepaint block
        // in physical log order. Keep the logical phase partition intact.
        self.blocks.insert(self.request_count, range);
        self.request_count += 1;
    }

    pub(crate) fn push_prepaint(&mut self, range: Range<usize>) {
        if range.is_empty() { return; }
        self.blocks.push(range);
    }

    /// Committed blocks are phase-local, sorted and disjoint. Validate
    /// containment before an intermediate ancestor inherits a rebuilt gap.
    pub(crate) fn covers(&self, child: &Self) -> bool {
        [(self.request_blocks(), child.request_blocks()),
         (self.prepaint_blocks(), child.prepaint_blocks())]
            .into_iter().all(|(parent, child)| child.iter().all(|span| {
                let index = parent.partition_point(|block| block.end <= span.start);
                parent.get(index).is_some_and(|block| block.start <= span.start && block.end >= span.end)
            }))
    }

    pub(crate) fn normalize(&mut self) {
        if self.request_blocks().len() <= 1 && self.prepaint_blocks().len() <= 1 {
            return;
        }
        let mut result = Self::default();
        for (prepaint, spans) in [(false, self.request_blocks()), (true, self.prepaint_blocks())] {
            let mut ordered: smallvec::SmallVec<[Range<usize>; 4]> = spans.iter().cloned().collect();
            ordered.sort_unstable_by_key(|span| span.start);
            let mut pending: Option<Range<usize>> = None;
            for span in ordered {
                if let Some(last) = pending.as_mut()
                    && last.end == span.start
                {
                    last.end = span.end;
                    continue;
                }
                if let Some(last) = pending.replace(span) {
                    if prepaint { result.push_prepaint(last); } else { result.push_request(last); }
                }
            }
            if let Some(last) = pending {
                if prepaint { result.push_prepaint(last); } else { result.push_request(last); }
            }
        }
        *self = result;
    }

    pub(crate) fn resolve(&self, moves: &SpanRelocations) -> Self {
        let mut result = Self::default();
        for span in self.request_blocks() {
            result.push_request(moves.resolve(span));
        }
        for span in self.prepaint_blocks() {
            result.push_prepaint(moves.resolve(span));
        }
        result
    }
}

/// An affine mapping per copied native block. A nested record can cross any
/// number of source blocks, and its target need not be contiguous.
#[derive(Default)]
pub(crate) struct ClaimBlockMap {
    segments: smallvec::SmallVec<[(Range<usize>, usize); 4]>,
}

impl ClaimBlockMap {
    pub(crate) fn record(&mut self, source: Range<usize>, target: usize) {
        if !source.is_empty() {
            self.segments.push((source, target));
        }
    }

    pub(crate) fn sort_by_source(&mut self) {
        self.segments.sort_unstable_by_key(|(source, _)| source.start);
    }

    pub(crate) fn between(source: &LayoutClaimBlocks, target: &LayoutClaimBlocks) -> Option<Self> {
        let mut result = Self::default();
        for (sources, targets) in [
            (source.request_blocks(), target.request_blocks()),
            (source.prepaint_blocks(), target.prepaint_blocks()),
        ] {
            let (mut i, mut j, mut a, mut b) = (0, 0, 0, 0);
            while let (Some(s), Some(t)) = (sources.get(i), targets.get(j)) {
                let len = (s.len() - a).min(t.len() - b);
                result.record(s.start + a..s.start + a + len, t.start + b);
                a += len;
                b += len;
                if a == s.len() { i += 1; a = 0; }
                if b == t.len() { j += 1; b = 0; }
            }
            if i != sources.len() || j != targets.len() {
                return None;
            }
        }
        result.sort_by_source();
        Some(result)
    }

    pub(crate) fn project_range(
        &self, span: &Range<usize>, complete: bool, mut emit: impl FnMut(Range<usize>),
    ) -> Option<()> {
        let mut copied = 0;
        let index = self.segments.partition_point(|(source, _)| source.end <= span.start);
        for (source, target) in &self.segments[index..] {
            if source.start >= span.end { break; }
            let start = source.start.max(span.start);
            let end = source.end.min(span.end);
            if start >= end { continue; }
            copied += end - start;
            emit(target + start - source.start..target + end - source.start);
        }
        (!complete || copied == span.len()).then_some(())
    }

    pub(crate) fn project(&self, claims: &LayoutClaimBlocks, complete: bool) -> Option<LayoutClaimBlocks> {
        let mut result = LayoutClaimBlocks::default();
        for (prepaint, spans) in [(false, claims.request_blocks()), (true, claims.prepaint_blocks())] {
            for span in spans {
                self.project_range(span, complete, |range| {
                    if prepaint { result.push_prepaint(range); }
                    else { result.push_request(range); }
                })?;
            }
        }
        Some(result)
    }
}

/// Candidate-only offsets for adopted prebuilt subtrees. Nested lifecycle
/// states resolve old spans at consumption instead of walking the arena.
#[derive(Default)]
pub(crate) struct SpanRelocations {
    // Ordered by source start; prefix ends also cover nested source intervals.
    moves: smallvec::SmallVec<[SpanMove; 2]>,
}

struct SpanMove {
    source: std::ops::Range<usize>,
    target: usize,
    sequence: usize,
    prefix_end: usize,
}

impl SpanRelocations {
    pub(crate) fn record(&mut self, source: std::ops::Range<usize>, target: usize) {
        if source.is_empty() || source.start == target {
            return;
        }
        let index = self.moves.partition_point(|entry| entry.source.start <= source.start);
        let sequence = self.moves.len();
        let previous_end = index.checked_sub(1).map_or(0, |i| self.moves[i].prefix_end);
        let prefix_end = previous_end.max(source.end);
        self.moves.insert(index, SpanMove { source, target, sequence, prefix_end });
        let mut end = prefix_end;
        for entry in &mut self.moves[index + 1..] {
            end = end.max(entry.source.end);
            if entry.prefix_end == end {
                break;
            }
            entry.prefix_end = end;
        }
    }

    pub(crate) fn resolve(&self, span: &std::ops::Range<usize>) -> std::ops::Range<usize> {
        let mut span = span.clone();
        if span.is_empty() {
            return span;
        }
        let mut next_sequence = 0;
        loop {
            let index = self.moves.partition_point(|entry| entry.source.start <= span.start);
            let mut selected: Option<&SpanMove> = None;
            for entry in self.moves[..index].iter().rev() {
                if entry.prefix_end < span.end {
                    break;
                }
                if entry.source.end >= span.end && entry.sequence >= next_sequence
                    && selected.is_none_or(|previous| entry.sequence < previous.sequence)
                {
                    selected = Some(entry);
                }
            }
            let Some(entry) = selected else { break; };
            // Preserve chronological adoption for overlapping spans and chains.
            span = entry.target + (span.start - entry.source.start)
                ..entry.target + (span.end - entry.source.start);
            next_sequence = entry.sequence + 1;
        }
        span
    }

    pub(crate) fn clear(&mut self) {
        self.moves.clear();
    }
}
pub use key::NativeLayoutKey;
pub(crate) use key::{is_list_row_segment, layout_node_key};
pub use metrics::{AvailableSpace, LayoutId};

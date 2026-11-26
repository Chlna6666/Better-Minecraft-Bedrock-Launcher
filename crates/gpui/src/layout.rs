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
pub(crate) use engine::{MeasurementKey, NativePlacementProof, SizeMeasurement};
/// Request-layout and prepaint claims in the double-buffered native log.
pub(crate) type LayoutClaimSpans = [std::ops::Range<usize>; 2];

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
pub(crate) use key::layout_node_key;
pub use metrics::{AvailableSpace, LayoutId};

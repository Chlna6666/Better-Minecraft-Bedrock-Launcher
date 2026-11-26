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
    moves: smallvec::SmallVec<[(std::ops::Range<usize>, usize); 2]>,
}

impl SpanRelocations {
    pub(crate) fn record(&mut self, source: std::ops::Range<usize>, target: usize) {
        if !source.is_empty() && source.start != target {
            self.moves.push((source, target));
        }
    }

    pub(crate) fn resolve(&self, span: &std::ops::Range<usize>) -> std::ops::Range<usize> {
        let mut span = span.clone();
        if !span.is_empty() {
            for (source, target) in &self.moves {
                if span.start >= source.start && span.end <= source.end {
                    span = target + (span.start - source.start)..target + (span.end - source.start);
                }
            }
        }
        span
    }

    pub(crate) fn clear(&mut self) {
        self.moves.clear();
    }
}
pub(crate) use key::layout_node_key;
pub use metrics::{AvailableSpace, LayoutId};

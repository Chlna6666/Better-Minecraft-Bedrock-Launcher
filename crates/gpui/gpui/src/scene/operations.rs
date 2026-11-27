//! Ordered Paint storage; shared payloads do not retain earlier Scenes.
use super::PaintOperation;
use std::{
    ops::{Index, Range},
    sync::Arc,
};

#[cfg(test)]
mod tests;

struct Segment {
    owner: Arc<Vec<PaintOperation>>,
    range: Range<usize>,
    end: usize,
}
impl Segment {
    fn start(&self) -> usize {
        self.end - self.range.len()
    }
    fn slice(&self) -> &[PaintOperation] {
        &self.owner[self.range.clone()]
    }
}

/// The only ordered operation payload, with immutable retained ranges and new append storage.
#[derive(Default)]
pub(crate) struct Operations {
    segments: Vec<Segment>,
    pending: Vec<PaintOperation>,
    len: usize,
    // Sparse prefix states belong to the authoritative operation sequence.
    boundaries: Vec<ReplayBoundary>,
}

/// A bounded logical view that can cross allocation boundaries.
#[derive(Clone, Copy)]
pub(crate) struct OperationRange<'a> {
    operations: &'a Operations,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Default)]
struct ReplayBoundary {
    after: usize,
    blur_depth: usize,
    layer_depth: usize,
    deferred_scrolls: usize,
    invalid: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeChange {
    None,
    DeferredScroll,
    StartBlur,
    EndBlur,
    StartLayer,
    EndLayer,
}

impl ScopeChange {
    fn of(operation: &PaintOperation) -> Self {
        match operation {
            PaintOperation::Primitive(_) => Self::None,
            PaintOperation::DeferredScroll(_) => Self::DeferredScroll,
            PaintOperation::StartBlur(_) => Self::StartBlur,
            PaintOperation::EndBlur => Self::EndBlur,
            PaintOperation::StartLayer(_) => Self::StartLayer,
            PaintOperation::EndLayer => Self::EndLayer,
        }
    }
}

impl ReplayBoundary {
    fn advance(mut self, after: usize, change: ScopeChange) -> Self {
        self.after = after;
        match change {
            ScopeChange::StartBlur => self.blur_depth += 1,
            ScopeChange::StartLayer => self.layer_depth += 1,
            ScopeChange::EndBlur => match self.blur_depth.checked_sub(1) {
                Some(depth) => self.blur_depth = depth,
                None => self.invalid = true,
            },
            ScopeChange::EndLayer => match self.layer_depth.checked_sub(1) {
                Some(depth) => self.layer_depth = depth,
                None => self.invalid = true,
            },
            ScopeChange::DeferredScroll => self.deferred_scrolls += 1,
            ScopeChange::None => {}
        }
        self
    }

    fn independent(self) -> bool {
        !self.invalid && self.blur_depth == 0 && self.layer_depth == 0
    }
}

impl Operations {
    fn record_boundary(&mut self, after: usize, change: ScopeChange) {
        if change != ScopeChange::None {
            let state = self.boundaries.last().copied().unwrap_or_default();
            self.boundaries.push(state.advance(after, change));
        }
    }

    fn boundary_at(&self, index: usize) -> ReplayBoundary {
        let end = self.boundaries.partition_point(|boundary| boundary.after <= index);
        end.checked_sub(1).map_or(ReplayBoundary::default(), |i| self.boundaries[i])
    }

    /// A fragment is replayable when neither endpoint inherits an external
    /// layer or capture, and no unmatched close has poisoned its prefix.
    pub(crate) fn range_is_replayable(&self, range: Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
            && (range.is_empty()
                || (self.boundary_at(range.start).independent()
                    && self.boundary_at(range.end).independent()))
    }

    fn rebuild_boundaries(&mut self) {
        let mut boundaries = std::mem::take(&mut self.boundaries);
        boundaries.clear();
        let mut state = ReplayBoundary::default();
        for (index, operation) in self.iter().enumerate() {
            let change = ScopeChange::of(operation);
            if change != ScopeChange::None {
                state = state.advance(index + 1, change);
                boundaries.push(state);
            }
        }
        self.boundaries = boundaries;
    }

    /// Deferred operations participate in the existing sparse prefix
    /// metadata. Sharing, editing and clearing ranges preserve the proof.
    pub(crate) fn has_deferred_scroll(&self) -> bool {
        self.boundaries.last().is_some_and(|boundary| boundary.deferred_scrolls != 0)
    }

    pub(crate) fn deferred_scroll_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.boundaries.iter().scan(0, |previous, boundary| {
            let changed = boundary.deferred_scrolls != *previous;
            *previous = boundary.deferred_scrolls;
            Some(changed.then_some(boundary.after - 1))
        }).flatten()
    }

    pub(crate) fn deferred_scrolls(&self) -> impl Iterator<Item = &crate::window::scroll_paint::ScrollPlacement> {
        self.deferred_scroll_indices().filter_map(|index| match self.get(index) {
            Some(PaintOperation::DeferredScroll(placement)) => Some(placement),
            _ => None,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn capacity(&self) -> usize {
        self.len - self.pending.len() + self.pending.capacity()
    }
    pub(crate) fn shrink_to(&mut self, floor: usize) {
        self.pending.shrink_to(floor);
        self.segments.shrink_to(self.segments.len());
        self.boundaries.shrink_to_fit();
    }
    pub(crate) fn push(&mut self, operation: PaintOperation) {
        self.len += 1;
        self.record_boundary(self.len, ScopeChange::of(&operation));
        self.pending.push(operation);
    }
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.boundaries.clear();
        for segment in self.segments.drain(..) {
            if let Ok(mut owner) = Arc::try_unwrap(segment.owner)
                && owner.capacity() > self.pending.capacity()
            {
                owner.clear();
                self.pending = owner;
            }
        }
        self.len = 0;
    }
    pub(crate) fn get(&self, index: usize) -> Option<&PaintOperation> {
        if index >= self.len {
            return None;
        }
        let segment = self
            .segments
            .partition_point(|segment| segment.end <= index);
        Some(if let Some(segment) = self.segments.get(segment) {
            &segment.slice()[index - segment.start()]
        } else {
            &self.pending[index - (self.len - self.pending.len())]
        })
    }
    pub(crate) fn first(&self) -> Option<&PaintOperation> {
        self.get(0)
    }
    pub(crate) fn get_range(&self, range: Range<usize>) -> Option<OperationRange<'_>> {
        (range.start <= range.end && range.end <= self.len).then_some(OperationRange {
            operations: self,
            start: range.start,
            end: range.end,
        })
    }
    pub(crate) fn range(&self, range: Range<usize>) -> OperationRange<'_> {
        self.get_range(range)
            .expect("Paint range must belong to this generation")
    }
    pub(crate) fn all(&self) -> OperationRange<'_> {
        self.range(0..self.len)
    }
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &PaintOperation> + Clone {
        self.all().iter()
    }

    /// Compares ordered Paint operations while skipping ranges that both generations share.
    ///
    /// A retained replay can rebuild a Scene from unchanged immutable operation segments plus a
    /// small set of newly painted segments. Comparing the operation values one by one defeats that
    /// sharing at Scene commit time, so identical storage ranges are treated as an exact proof and
    /// only unshared spans are passed to `matches`.
    pub(crate) fn matches_by(
        &self,
        other: &Self,
        matches: impl FnMut(&PaintOperation, &PaintOperation) -> bool,
    ) -> bool {
        self.for_each_unshared_pair(other, matches)
    }

    /// Compares Paint values and, after the first visual change, checks whether the remaining
    /// operations still address the same prepared batch ranges.
    ///
    /// Immutable ranges shared by both Scenes prove both comparisons without visiting their
    /// operations. When batch topology is not needed, the comparison stops at the first visual
    /// change instead of adding another full Scene walk.
    pub(crate) fn scene_reuse_matches(
        &self,
        other: &Self,
        compare_batch_topology: bool,
    ) -> (bool, bool) {
        if self.len != other.len {
            return (false, false);
        }

        let mut paint_matches = true;
        let mut batch_topology_matches = true;
        let operations_match = self.for_each_unshared_pair(other, |left, right| {
            if paint_matches && left.same_paint(right) {
                return true;
            }

            paint_matches = false;
            if compare_batch_topology && left.has_same_batch_topology(right) {
                true
            } else {
                batch_topology_matches = false;
                false
            }
        });
        (
            paint_matches && operations_match,
            operations_match
                && (paint_matches || (compare_batch_topology && batch_topology_matches)),
        )
    }

    /// Visits unshared operation pairs, skipping immutable storage ranges shared by both Scenes.
    fn for_each_unshared_pair(
        &self,
        other: &Self,
        compare: impl FnMut(&PaintOperation, &PaintOperation) -> bool,
    ) -> bool {
        self.all().matches_by(other.all(), compare)
    }

    /// Freeze newly appended operations without rewriting any existing immutable ranges.
    pub(crate) fn finish(&mut self) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let count = pending.len();
            self.len -= count;
            self.append(Arc::new(pending), 0..count);
        }
    }
    fn append(&mut self, owner: Arc<Vec<PaintOperation>>, range: Range<usize>) {
        self.len += range.len();
        if let Some(previous) = self.segments.last_mut()
            && Arc::ptr_eq(&previous.owner, &owner)
            && previous.range.end == range.start
        {
            previous.range.end = range.end;
            previous.end = self.len;
            return;
        }
        self.segments.push(Segment {
            owner,
            range,
            end: self.len,
        });
    }
    /// Release unrelated old page payload when the combined live views are sparse.
    pub(crate) fn compact(&mut self) {
        assert!(self.pending.is_empty(), "compact only finalized Paint");
        if self
            .segments
            .iter()
            .all(|segment| segment.range.len() == segment.owner.len())
        {
            return;
        }
        let mut referenced = collections::FxHashMap::<_, usize>::default();
        for segment in &self.segments {
            *referenced.entry(Arc::as_ptr(&segment.owner)).or_default() += segment.range.len();
        }
        let segments = std::mem::take(&mut self.segments);
        self.len = 0;
        for segment in segments {
            if referenced[&Arc::as_ptr(&segment.owner)] < segment.owner.len() / 2 {
                let payload = segment.slice().to_vec();
                let count = payload.len();
                self.append(Arc::new(payload), 0..count);
            } else {
                self.append(segment.owner, segment.range);
            }
        }
    }
    /// Share a finished range without copying operations or retaining its source Scene.
    pub(crate) fn share(&mut self, previous: &Self, range: Range<usize>) {
        assert!(previous.pending.is_empty(), "shared Paint must be finished");
        assert!(range.start <= range.end && range.end <= previous.len);
        self.finish();
        let destination = self.len;
        let first_boundary = previous.boundaries.partition_point(|b| b.after <= range.start);
        for boundary in previous.boundaries[first_boundary..].iter().take_while(|b| b.after <= range.end) {
            self.record_boundary(destination + boundary.after - range.start, ScopeChange::of(&previous[boundary.after - 1]));
        }
        let first = previous
            .segments
            .partition_point(|segment| segment.end <= range.start);
        for segment in previous.segments[first..]
            .iter()
            .take_while(|segment| segment.start() < range.end)
        {
            let start = range.start.max(segment.start());
            let end = range.end.min(segment.end);
            if start < end {
                let local = segment.range.start + start - segment.start();
                self.append(Arc::clone(&segment.owner), local..local + end - start);
            }
        }
    }
    /// Edit only requested operations; shared neighbours keep their original backing.
    /// Capture construction uses this before publishing a Paint generation.
    pub(crate) fn edit(&mut self, range: Range<usize>, mut edit: impl FnMut(&mut PaintOperation)) {
        assert!(range.start <= range.end && range.end <= self.len);
        if range.is_empty() {
            return;
        }
        let mut structure_changed = false;
        let mut edit = |operation: &mut PaintOperation| {
            let before = ScopeChange::of(operation);
            edit(operation);
            structure_changed |= before != ScopeChange::of(operation);
        };
        let pending_start = self.len - self.pending.len();
        if range.start < pending_start {
            let segments = std::mem::take(&mut self.segments);
            let pending_len = self.pending.len();
            self.len = 0;
            for segment in segments {
                self.edit_segment(segment, &range, &mut edit);
            }
            self.len += pending_len;
        }
        if range.end > pending_start {
            for operation in &mut self.pending
                [range.start.saturating_sub(pending_start)..range.end - pending_start]
            {
                edit(operation);
            }
        }
        if structure_changed {
            self.rebuild_boundaries();
        }
    }

    fn edit_segment(
        &mut self,
        mut segment: Segment,
        range: &Range<usize>,
        edit: &mut impl FnMut(&mut PaintOperation),
    ) {
        let start = range.start.max(segment.start());
        let end = range.end.min(segment.end);
        if start >= end {
            self.append(segment.owner, segment.range);
            return;
        }
        let local = segment.range.start + start - segment.start();
        let local_end = local + end - start;
        if let Some(owner) = Arc::get_mut(&mut segment.owner) {
            for operation in &mut owner[local..local_end] {
                edit(operation);
            }
            self.append(segment.owner, segment.range);
            return;
        }
        if segment.range.start < local {
            self.append(Arc::clone(&segment.owner), segment.range.start..local);
        }
        let mut changed = segment.owner[local..local_end].to_vec();
        for operation in &mut changed {
            edit(operation);
        }
        self.append(Arc::new(changed), 0..end - start);
        if local_end < segment.range.end {
            self.append(segment.owner, local_end..segment.range.end);
        }
    }
}

impl<'a> OperationRange<'a> {
    pub(crate) fn len(self) -> usize {
        self.end - self.start
    }
    pub(crate) fn get(self, index: usize) -> Option<&'a PaintOperation> {
        (index < self.len()).then(|| &self.operations[self.start + index])
    }
    pub(crate) fn range(self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len());
        Self {
            operations: self.operations,
            start: self.start + range.start,
            end: self.start + range.end,
        }
    }
    /// Compare logical ranges using their immutable backing slices as an
    /// exact proof. Pending tails are compared without freezing the builder.
    pub(crate) fn matches_by(
        self,
        other: OperationRange<'_>,
        mut compare: impl FnMut(&PaintOperation, &PaintOperation) -> bool,
    ) -> bool {
        if self.len() != other.len() {
            return false;
        }
        let mut left_runs = self.slices();
        let mut right_runs = other.slices();
        let mut left = left_runs.next().unwrap_or(&[]);
        let mut right = right_runs.next().unwrap_or(&[]);
        while !left.is_empty() && !right.is_empty() {
            let count = left.len().min(right.len());
            let (a, left_tail) = left.split_at(count);
            let (b, right_tail) = right.split_at(count);
            // Both slices are live and nonempty, so equal addresses prove
            // identical storage, including the source offset.
            if !std::ptr::eq(a, b) && !a.iter().zip(b).all(|(a, b)| compare(a, b)) {
                return false;
            }
            left = if left_tail.is_empty() { left_runs.next().unwrap_or(&[]) } else { left_tail };
            right = if right_tail.is_empty() { right_runs.next().unwrap_or(&[]) } else { right_tail };
        }
        left.is_empty() && right.is_empty()
    }

    /// Contiguous portions in logical order, including an unfinished tail.
    /// Resolve the segment boundaries once for the whole range.
    pub(crate) fn slices(self) -> impl DoubleEndedIterator<Item = &'a [PaintOperation]> + Clone {
        let first = self
            .operations
            .segments
            .partition_point(|segment| segment.end <= self.start);
        let end = self
            .operations
            .segments
            .partition_point(|segment| segment.start() < self.end);
        let pending_start = self.operations.len - self.operations.pending.len();
        let pending = if self.end > pending_start && self.start < self.end {
            &self.operations.pending
                [self.start.saturating_sub(pending_start)..self.end - pending_start]
        } else {
            &[]
        };
        self.operations.segments[first..end]
            .iter()
            .map(move |segment| {
                let start = self.start.saturating_sub(segment.start());
                let end = self.end.min(segment.end) - segment.start();
                &segment.slice()[start..end]
            })
            .chain(std::iter::once(pending))
            .filter(|slice| !slice.is_empty())
    }

    pub(crate) fn iter(self) -> impl DoubleEndedIterator<Item = &'a PaintOperation> + Clone {
        self.slices().flat_map(|slice| slice.iter())
    }
}
impl Index<usize> for Operations {
    type Output = PaintOperation;
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("Paint index must be in bounds")
    }
}
impl Extend<PaintOperation> for Operations {
    fn extend<T: IntoIterator<Item = PaintOperation>>(&mut self, operations: T) {
        for operation in operations {
            self.push(operation);
        }
    }
}

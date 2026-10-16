//! Immutable ordered primitive payload segments. Window retention chooses which ranges may be shared.
#[cfg(test)]
mod tests;
use super::{DrawOrder, Quad, Shadow};

pub(crate) trait Ordered: Copy {
    fn order(&self) -> DrawOrder;
}

impl Ordered for Quad {
    fn order(&self) -> DrawOrder {
        self.order
    }
}

impl Ordered for Shadow {
    fn order(&self) -> DrawOrder {
        self.order
    }
}
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    ops::{Index, Range},
    sync::{Arc, Weak},
};

/// A caller-proven static Quad span in one source Paint generation.
/// Scene still validates its current ordering context before sharing any payload.
pub(crate) struct QuadReplay {
    pub(crate) paint_range: Range<usize>,
    pub(crate) quad_range: Range<usize>,
}

struct Segment<T> {
    owner: Arc<Vec<T>>,
    range: Range<usize>,
    end: usize,
}

impl<T> Segment<T> {
    fn slice(&self) -> &[T] {
        &self.owner[self.range.clone()]
    }
    fn start(&self) -> usize {
        self.end - self.range.len()
    }
}

/// Sorted logical primitive storage backed by immutable payload segments and newly painted runs.
/// Segment boundaries do not change primitive indices or GPU ownership.
pub(crate) struct PrimitiveStream<T: Ordered> {
    segments: Vec<Segment<T>>,
    pending: Vec<T>,
    len: usize,
}

/// Identity of immutable payload portions, without retaining their drawing data.
/// Weak control blocks prevent allocation-address reuse from matching a retired source.
pub(crate) struct PayloadIdentity<T> {
    segments: Vec<(Weak<Vec<T>>, Range<usize>)>,
}

impl<T: Ordered> PayloadIdentity<T> {
    pub(crate) fn matches(&self, stream: &PrimitiveStream<T>) -> bool {
        stream.pending.is_empty()
            && self.segments.len() == stream.segments.len()
            && self
                .segments
                .iter()
                .zip(&stream.segments)
                .all(|((owner, range), segment)| {
                    owner.as_ptr() == Arc::as_ptr(&segment.owner) && *range == segment.range
                })
    }
}

impl<T: Ordered> Default for PrimitiveStream<T> {
    fn default() -> Self {
        Self {
            segments: Vec::new(),
            pending: Vec::new(),
            len: 0,
        }
    }
}

impl<T: Ordered> PrimitiveStream<T> {
    /// Unfinished mutable runs cannot serve as an immutable content identity.
    pub(crate) fn identity(&self) -> Option<PayloadIdentity<T>> {
        self.pending.is_empty().then(|| PayloadIdentity {
            segments: self
                .segments
                .iter()
                .map(|segment| (Arc::downgrade(&segment.owner), segment.range.clone()))
                .collect(),
        })
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn push(&mut self, quad: T) {
        self.pending.push(quad);
        self.len += 1;
    }
    pub(crate) fn capacity(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| segment.range.len())
            .sum::<usize>()
            + self.pending.capacity()
    }
    pub(crate) fn shrink_to(&mut self, floor: usize) {
        self.pending.shrink_to(floor);
        self.segments.shrink_to(self.segments.len());
    }
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
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
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.segments
            .iter()
            .flat_map(|segment| segment.slice().iter())
            .chain(self.pending.iter())
    }
    pub(crate) fn last(&self) -> Option<&T> {
        self.pending
            .last()
            .or_else(|| self.segments.last()?.slice().last())
    }
    pub(crate) fn partition_point(&self, mut predicate: impl FnMut(&T) -> bool) -> usize {
        let (mut first, mut end) = (0, self.len);
        while first < end {
            let middle = first + (end - first) / 2;
            if predicate(&self[middle]) {
                first = middle + 1;
            } else {
                end = middle;
            }
        }
        first
    }
    /// Borrows every contiguous payload portion in a logical sorted range.
    pub(crate) fn slices(&self, range: Range<usize>) -> impl Iterator<Item = &[T]> {
        assert!(range.start <= range.end && range.end <= self.len);
        let first = self
            .segments
            .partition_point(|segment| segment.end <= range.start);
        let pending_start = self.len - self.pending.len();
        let pending = (range.end > pending_start && range.start < range.end).then(|| {
            &self.pending[range.start.saturating_sub(pending_start)..range.end - pending_start]
        });
        self.segments[first..]
            .iter()
            .take_while(move |segment| segment.start() < range.end)
            .filter(move |_| range.start < range.end)
            .map(move |segment| {
                let start = range.start.saturating_sub(segment.start());
                let end = range.end.min(segment.end) - segment.start();
                &segment.slice()[start..end]
            })
            .chain(pending)
    }
    /// Append proven immutable source ranges; never clone or mutate their primitive payload.
    pub(crate) fn share(&mut self, previous: &Self, range: Range<usize>) {
        assert!(previous.pending.is_empty(), "shared Paint must be finished");
        assert!(range.start <= range.end && range.end <= previous.len);
        // Preserve insertion order for equal DrawOrder when new Paint lies
        // between two replayed ranges in the same layer.
        self.freeze_pending();
        let first = previous
            .segments
            .partition_point(|segment| segment.end <= range.start);
        for segment in previous.segments[first..]
            .iter()
            .take_while(|segment| segment.start() < range.end)
        {
            let first = range.start.max(segment.start());
            let end = range.end.min(segment.end);
            if first < end {
                let local = segment.range.start + first - segment.start();
                self.append(Arc::clone(&segment.owner), local..local + end - first);
            }
        }
    }
    fn append(&mut self, owner: Arc<Vec<T>>, range: Range<usize>) {
        let end = self.segments.last().map_or(0, |segment| segment.end) + range.len();
        self.len += range.len();
        if let Some(previous) = self.segments.last_mut()
            && Arc::ptr_eq(&previous.owner, &owner)
            && previous.range.end == range.start
        {
            previous.range.end = range.end;
            previous.end = end;
            return;
        }
        self.segments.push(Segment { owner, range, end });
    }
    fn freeze_pending(&mut self) {
        if !self.pending.is_empty() {
            self.pending.sort_unstable_by_key(|quad| quad.order());
            let pending = std::mem::take(&mut self.pending);
            let count = pending.len();
            // These primitives were already counted by push().
            self.len -= count;
            self.append(Arc::new(pending), 0..count);
        }
    }
    /// Sort only newly painted primitives, then merge immutable runs without rewriting old payload.
    pub(crate) fn finish(&mut self) {
        self.freeze_pending();
        if self.segments.windows(2).all(|pair| {
            pair[0]
                .slice()
                .last()
                .expect("nonempty Paint segment")
                .order()
                <= pair[1].slice()[0].order()
        }) {
            return;
        }
        let mut sources = std::mem::take(&mut self.segments);
        let mut heads = BinaryHeap::new();
        for (index, segment) in sources.iter().enumerate() {
            heads.push(Reverse((segment.slice()[0].order(), index)));
        }
        self.len = 0;
        while let Some(Reverse((_, index))) = heads.pop() {
            let segment = &mut sources[index];
            let count = heads
                .peek()
                .map_or(segment.range.len(), |Reverse((order, _))| {
                    segment
                        .slice()
                        .partition_point(|quad| quad.order() <= *order)
                        .max(1)
                });
            let first = segment.range.start;
            self.append(Arc::clone(&segment.owner), first..first + count);
            segment.range.start += count;
            if !segment.range.is_empty() {
                heads.push(Reverse((segment.slice()[0].order(), index)));
            }
        }
    }
    /// Compact sparse borrowed allocations so a small fragment cannot pin an unrelated old page.
    /// New Paint and fully referenced shared owners keep their original payload without copying.
    pub(crate) fn compact(&mut self) {
        if self
            .segments
            .iter()
            .all(|segment| segment.range.len() == segment.owner.len())
        {
            return;
        }
        // Merging interleaved orders can split one allocation into many views.
        // Judge sparsity across all live views, otherwise every small view would
        // copy a fully retained payload again on every UI commit.
        let mut referenced = collections::FxHashMap::<_, usize>::default();
        for segment in &self.segments {
            *referenced.entry(Arc::as_ptr(&segment.owner)).or_default() += segment.range.len();
        }
        let segments = std::mem::take(&mut self.segments);
        self.len = 0;
        for segment in segments {
            let sparse = referenced[&Arc::as_ptr(&segment.owner)] < segment.owner.len() / 2;
            if sparse {
                let payload = segment.slice().to_vec();
                let count = payload.len();
                self.append(Arc::new(payload), 0..count);
            } else {
                self.append(segment.owner, segment.range);
            }
        }
    }
    pub(super) fn cursor(&self) -> Cursor<'_, T> {
        Cursor {
            stream: self,
            segment: 0,
            offset: 0,
        }
    }
}

impl<T: Ordered> Index<usize> for PrimitiveStream<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        let segment = self
            .segments
            .partition_point(|segment| segment.end <= index);
        if let Some(segment) = self.segments.get(segment) {
            &segment.slice()[index - segment.start()]
        } else {
            &self.pending[index - (self.len - self.pending.len())]
        }
    }
}

pub(super) struct Cursor<'a, T: Ordered> {
    stream: &'a PrimitiveStream<T>,
    segment: usize,
    offset: usize,
}

impl<'a, T: Ordered> Cursor<'a, T> {
    pub(super) fn position(&self) -> usize {
        self.stream
            .segments
            .get(self.segment)
            .map_or(self.stream.len - self.stream.pending.len(), Segment::start)
            + self.offset
    }
    pub(super) fn remaining(&self) -> &'a [T] {
        self.stream.segments.get(self.segment).map_or_else(
            || &self.stream.pending[self.offset..],
            |segment| &segment.slice()[self.offset..],
        )
    }
    pub(super) fn peek(&self) -> Option<&'a T> {
        self.remaining().first()
    }
    pub(super) fn advance(&mut self, count: usize) {
        assert!(count <= self.remaining().len());
        self.offset += count;
        if self.segment < self.stream.segments.len()
            && self.offset == self.stream.segments[self.segment].range.len()
        {
            self.segment += 1;
            self.offset = 0;
        }
    }
}

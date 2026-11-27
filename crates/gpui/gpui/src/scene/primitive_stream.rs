//! Immutable ordered primitive payload segments. Window retention chooses which ranges may be shared.
mod sort;
pub(super) use sort::DrawPermutation;
#[cfg(test)]
mod tests;
use super::{DrawOrder, MonochromeSprite, PolychromeSprite, Quad, Shadow};

pub(crate) trait Ordered: Copy {
    type SortKey: Ord + Copy;
    const ORDER_ONLY: bool = true;
    fn order(&self) -> DrawOrder;

    fn packed_sort_key(&self, slot: u32) -> u128 {
        (u128::from(self.order()) << 32) | u128::from(slot)
    }
    fn sort_key(&self) -> Self::SortKey;

    fn sort_pending(items: &mut [Self]) {
        sort::pending(items);
    }
}

impl Ordered for Quad {
    type SortKey = DrawOrder;
    fn sort_key(&self) -> Self::SortKey { self.order }

    fn order(&self) -> DrawOrder {
        self.order
    }
}

impl Ordered for Shadow {
    type SortKey = DrawOrder;
    fn sort_key(&self) -> Self::SortKey { self.order }

    fn order(&self) -> DrawOrder {
        self.order
    }
}
impl Ordered for MonochromeSprite {
    type SortKey = (DrawOrder, u32, u32, u32);
    const ORDER_ONLY: bool = false;

    fn packed_sort_key(&self, slot: u32) -> u128 {
        let (order, kind, texture, sampling) = self.sort_key();
        debug_assert!(slot < 1 << 28 && sampling < 16);
        (u128::from(order) << 96)
            | (u128::from(kind) << 64)
            | (u128::from(texture) << 32)
            | (u128::from(sampling) << 28)
            | u128::from(slot)
    }

    fn order(&self) -> DrawOrder {
        self.order
    }

    fn sort_key(&self) -> Self::SortKey {
        (self.order(), self.tile.texture_id.kind as u32,
            self.tile.texture_id.index, self.sampling() as u32)
    }

    fn sort_pending(items: &mut [Self]) {
        if !items.is_sorted_by_key(Self::sort_key) {
            items.sort_by_key(Self::sort_key);
        }
    }
}

impl Ordered for PolychromeSprite {
    type SortKey = (DrawOrder, u32, u32);
    const ORDER_ONLY: bool = false;

    fn order(&self) -> DrawOrder { self.order }

    fn sort_key(&self) -> Self::SortKey {
        (self.order, self.tile.texture_id.kind as u32, self.tile.texture_id.index)
    }

    fn packed_sort_key(&self, slot: u32) -> u128 {
        let (order, kind, texture) = self.sort_key();
        (u128::from(order) << 96) | (u128::from(kind) << 64)
            | (u128::from(texture) << 32) | u128::from(slot)
    }

    fn sort_pending(items: &mut [Self]) {
        if !items.is_sorted_by_key(Self::sort_key) {
            items.sort_by_key(Self::sort_key);
        }
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
    pub(crate) source_revision: u64,
    pub(crate) paint_range: Range<usize>,
    pub(crate) quad_range: Range<usize>,
}

#[derive(Clone)]
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

// A completed stream can share its entire descriptor table, not just
// the primitive allocations referenced by its entries. Empty streams do
// not allocate a control block; mutation detaches only the descriptors.
struct SegmentTable<T>(Option<Arc<Vec<Segment<T>>>>);

impl<T> Default for SegmentTable<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> std::ops::Deref for SegmentTable<T> {
    type Target = [Segment<T>];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl<T: Clone> SegmentTable<T> {
    fn make_mut(&mut self) -> &mut Vec<Segment<T>> {
        Arc::make_mut(self.0.get_or_insert_with(Arc::default))
    }

    fn share_from(&mut self, previous: &Self) {
        self.0 = previous.0.clone();
    }

    fn shares_storage(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    fn take_for_merge(&mut self) -> Vec<Segment<T>> {
        self.0.take().map_or_else(Vec::new, Arc::unwrap_or_clone)
    }

    fn clear(&mut self) {
        if let Some(segments) = self.0.as_mut().and_then(Arc::get_mut) {
            segments.clear();
        } else {
            self.0 = None;
        }
    }

    fn shrink_to_fit(&mut self) {
        if let Some(segments) = self.0.as_mut().and_then(Arc::get_mut) {
            segments.shrink_to_fit();
        }
    }
}

/// Sorted logical primitive storage backed by immutable payload segments and newly painted runs.
/// Segment boundaries do not change primitive indices or GPU ownership.
pub(crate) struct PrimitiveStream<T: Ordered> {
    segments: SegmentTable<T>,
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
                .zip(stream.segments.iter())
                .all(|((owner, range), segment)| {
                    owner.as_ptr() == Arc::as_ptr(&segment.owner) && *range == segment.range
                })
    }
}

impl<T: Ordered> Default for PrimitiveStream<T> {
    fn default() -> Self {
        Self {
            segments: SegmentTable::default(),
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
    pub(crate) fn is_empty(&self) -> bool { self.len == 0 }

    pub(crate) fn push(&mut self, quad: T) {
        self.pending.push(quad);
        self.len += 1;
    }
    pub(super) fn account_storage(&self, accounting: &mut crate::diagnostics::frame_storage::Accounting) {
        use crate::diagnostics::frame_storage::Kind;
        if let Some(segments) = &self.segments.0 {
            accounting.add(segments, Kind::Descriptor);
        }
        for segment in self.segments.iter() {
            accounting.add(&segment.owner, Kind::Primitive);
        }
        accounting.add(&self.pending, Kind::Primitive);
    }

    pub(crate) fn capacity(&self) -> usize {
        self.len - self.pending.len() + self.pending.capacity()
    }
    pub(crate) fn shrink_to(&mut self, floor: usize) {
        self.pending.shrink_to(floor);
        self.segments.shrink_to_fit();
    }
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        // Never detach a shared table just to discard it. Only a unique
        // descriptor owner can contribute its primitive allocations to scratch.
        if let Some(segments) = self.segments.0.as_mut().and_then(Arc::get_mut) {
            for segment in segments.drain(..) {
                if let Ok(mut owner) = Arc::try_unwrap(segment.owner)
                    && owner.capacity() > self.pending.capacity()
                {
                    owner.clear();
                    self.pending = owner;
                }
            }
        } else {
            self.segments.0 = None;
        }
        self.len = 0;
    }
    /// A frame operation builder reclaims its authoritative typed page.
    /// Drop draw aliases first, rather than stealing the same allocation
    /// into an otherwise unused independent draw builder.
    pub(super) fn release_draw_aliases(&mut self) {
        self.segments.clear();
        self.pending.clear();
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
        // Partition the immutable runs first, then the one candidate run.
        // Indexing the logical stream at every binary-search step would
        // repeat a second binary search over the same segment descriptors.
        let slot = self.segments.partition_point(|segment| {
            predicate(segment.slice().last().expect("nonempty Paint segment"))
        });
        if let Some(segment) = self.segments.get(slot) {
            segment.start() + segment.slice().partition_point(predicate)
        } else {
            self.len - self.pending.len() + self.pending.partition_point(predicate)
        }
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
        // A complete immutable stream already has the correct segment
        // boundaries, ordering and cumulative offsets. In an empty target
        // adopt its descriptors directly without re-running append checks.
        if self.len == 0 && range.start == 0 && range.end == previous.len {
            self.segments.share_from(&previous.segments);
            self.len = previous.len;
            return;
        }
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
    /// The original insertion sequence must agree exactly before replacing
    /// a fresh draw builder with an operation-owned sorted allocation.
    pub(super) fn matches_fresh_pending(&self, values: &[T]) -> bool
    where
        T: PartialEq,
    {
        self.segments.is_empty() && self.pending.as_slice() == values
    }

    pub(super) fn adopt_fresh_sorted(&mut self, owner: Arc<Vec<T>>) {
        debug_assert!(self.segments.is_empty() && self.pending.len() == owner.len());
        let count = owner.len();
        self.pending = Vec::new();
        self.len = 0;
        self.append(owner, 0..count);
    }

    /// The operation owner has verified this physical range is sorted.
    pub(super) fn share_sorted_page(&mut self, owner: &Arc<Vec<T>>, range: Range<usize>) {
        debug_assert!(!range.is_empty() && range.end <= owner.len());
        self.freeze_pending();
        self.append(Arc::clone(owner), range);
    }

    fn append(&mut self, owner: Arc<Vec<T>>, range: Range<usize>) {
        let end = self.segments.last().map_or(0, |segment| segment.end) + range.len();
        self.len += range.len();
        let segments = self.segments.make_mut();
        if let Some(previous) = segments.last_mut()
            && Arc::ptr_eq(&previous.owner, &owner)
            && previous.range.end == range.start
            // Operation-owned ranges can be individually sorted while their
            // common backing allocation contains a descending boundary.
            && previous.slice().last().expect("nonempty segment").sort_key()
                <= owner[range.start].sort_key()
        {
            previous.range.end = range.end;
            previous.end = end;
            return;
        }
        segments.push(Segment { owner, range, end });
    }
    fn freeze_pending(&mut self) {
        if !self.pending.is_empty() {
            T::sort_pending(&mut self.pending);
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
                .sort_key()
                <= pair[1].slice()[0].sort_key()
        }) {
            return;
        }
        let mut sources = self.segments.take_for_merge();
        let mut heads = BinaryHeap::new();
        for (index, segment) in sources.iter().enumerate() {
            heads.push(Reverse((segment.slice()[0].sort_key(), index)));
        }
        self.len = 0;
        while let Some(Reverse((_, index))) = heads.pop() {
            let segment = &mut sources[index];
            let count = heads
                .peek()
                .map_or(segment.range.len(), |Reverse((order, next_index))| {
                    // Equal orders belong to the earlier source first. A later
                    // source can start below the next head but must stop before
                    // reaching its equal-order primitives.
                    segment
                        .slice()
                        .partition_point(|primitive| {
                            primitive.sort_key() < *order
                                || (primitive.sort_key() == *order && index < *next_index)
                        })
                        .max(1)
                });
            let first = segment.range.start;
            self.append(Arc::clone(&segment.owner), first..first + count);
            segment.range.start += count;
            if !segment.range.is_empty() {
                heads.push(Reverse((segment.slice()[0].sort_key(), index)));
            }
        }
    }
    /// Compact sparse borrowed allocations so a small fragment cannot pin an unrelated old page.
    /// New Paint and fully referenced shared owners keep their original payload without copying.
    #[cfg(test)]
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
        let compacted = super::page_compaction::compact_sparse_pages(
            self.segments.iter().map(|segment| (&segment.owner, &segment.range)),
        );
        if compacted.is_empty() {
            return;
        }
        self.remap_pages(&compacted);
    }

    /// Identical immutable physical views need no coverage recomputation.
    /// No Weak identity table, payload comparison or allocation is involved.
    pub(super) fn same_physical_ranges(&self, previous: &Self) -> bool {
        self.pending.is_empty() && previous.pending.is_empty()
            && self.len == previous.len
            && (self.segments.shares_storage(&previous.segments)
                || (self.segments.len() == previous.segments.len()
                    && self.segments.iter().zip(previous.segments.iter()).all(|(a, b)| {
                        Arc::ptr_eq(&a.owner, &b.owner) && a.range == b.range
                    })))
    }

    pub(super) fn has_full_physical_ranges(&self) -> bool {
        debug_assert!(self.pending.is_empty());
        self.segments.iter().all(|segment| segment.range.len() == segment.owner.len())
    }

    pub(super) fn physical_ranges(&self) -> impl Iterator<Item = (&Arc<Vec<T>>, &Range<usize>)> {
        debug_assert!(self.pending.is_empty());
        self.segments.iter().map(|segment| (&segment.owner, &segment.range))
    }

    pub(super) fn remap_pages(
        &mut self,
        compacted: &collections::FxHashMap<*const Vec<T>, super::page_compaction::CompactedPage<Vec<T>>>,
    ) {
        // No matching owner means no mutation, including on a shared table.
        if compacted.is_empty() || !self.segments.iter().any(|segment| {
            compacted.contains_key(&Arc::as_ptr(&segment.owner))
        }) {
            return;
        }
        // Logical offsets and existing descriptor capacity stay intact.
        for segment in self.segments.make_mut() {
            if let Some(page) = compacted.get(&Arc::as_ptr(&segment.owner)) {
                segment.range = page.remap(segment.range.clone());
                segment.owner = Arc::clone(&page.owner);
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

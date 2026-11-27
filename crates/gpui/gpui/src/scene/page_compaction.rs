//! Compact immutable payload owners using the union of live physical
//! ranges, not the number of logical references. A repeated or overlapping
//! view never pins unrelated payload or receives its own cloned allocation.
use collections::FxHashMap;
use smallvec::SmallVec;
use std::{ops::Range, sync::Arc};

struct Coverage<'a, T> {
    owner: &'a Arc<Vec<T>>,
    ranges: SmallVec<[Range<usize>; 4]>,
    full: bool,
}

pub(super) struct CompactedPage<T> {
    pub(super) owner: Arc<Vec<T>>,
    ranges: SmallVec<[(Range<usize>, usize); 4]>,
}

impl<T> CompactedPage<T> {
    pub(super) fn remap(&self, range: Range<usize>) -> Range<usize> {
        let index = self.ranges.partition_point(|(source, _)| source.end <= range.start);
        let (source, target) = &self.ranges[index];
        debug_assert!(source.start <= range.start && range.end <= source.end);
        target + range.start - source.start..target + range.end - source.start
    }
}

pub(super) fn compact_sparse_pages<'a, T: Clone + 'a>(
    views: impl Iterator<Item = (&'a Arc<Vec<T>>, &'a Range<usize>)>,
) -> FxHashMap<*const Vec<T>, CompactedPage<T>> {
    let mut coverage = FxHashMap::<_, Coverage<'a, T>>::default();
    for (owner, range) in views {
        let entry = coverage.entry(Arc::as_ptr(owner)).or_insert_with(|| Coverage {
            owner,
            ranges: SmallVec::new(),
            full: false,
        });
        if entry.full {
            continue;
        }
        if range.len() == owner.len() {
            entry.full = true;
            entry.ranges.clear();
            continue;
        }
        // Append-only views of one owner are commonly already ordered.
        // Coalesce them before considering a sort, and ignore duplicates.
        if let Some(last) = entry.ranges.last_mut()
            && last.start <= range.start && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            entry.ranges.push(range.clone());
        }
    }
    let mut compacted = FxHashMap::default();
    for (identity, mut entry) in coverage {
        if entry.full {
            continue;
        }
        entry.ranges.sort_unstable_by_key(|range| range.start);
        let mut count = 0;
        for index in 0..entry.ranges.len() {
            let range = entry.ranges[index].clone();
            if count > 0 && range.start <= entry.ranges[count - 1].end {
                entry.ranges[count - 1].end = entry.ranges[count - 1].end.max(range.end);
            } else {
                entry.ranges[count] = range;
                count += 1;
            }
        }
        entry.ranges.truncate(count);
        let live: usize = entry.ranges.iter().map(Range::len).sum();
        if live >= entry.owner.len() / 2 {
            continue;
        }
        let mut payload = Vec::with_capacity(live);
        let mut ranges = SmallVec::new();
        for range in entry.ranges {
            let target = payload.len();
            payload.extend_from_slice(&entry.owner[range.clone()]);
            ranges.push((range, target));
        }
        compacted.insert(identity, CompactedPage { owner: Arc::new(payload), ranges });
    }
    compacted
}

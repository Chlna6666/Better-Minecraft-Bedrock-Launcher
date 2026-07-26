use crate::tasks::task_manager::ByteRangeVisualization;

#[derive(Debug, Default)]
pub(super) struct DownloadedRangeSet {
    ranges: Vec<ByteRangeVisualization>,
}

impl DownloadedRangeSet {
    pub(super) fn insert(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }

        let first_merged = self.ranges.partition_point(|range| range.end < start);
        let mut merged_start = start;
        let mut merged_end = end;
        let mut after_merged = first_merged;

        while let Some(range) = self.ranges.get(after_merged) {
            if range.start > merged_end {
                break;
            }
            merged_start = merged_start.min(range.start);
            merged_end = merged_end.max(range.end);
            after_merged += 1;
        }

        let merged = ByteRangeVisualization {
            start: merged_start,
            end: merged_end,
        };
        self.ranges
            .splice(first_merged..after_merged, std::iter::once(merged));
    }

    pub(super) fn to_vec(&self) -> Vec<ByteRangeVisualization> {
        self.ranges.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(set: &DownloadedRangeSet) -> Vec<(u64, u64)> {
        set.ranges
            .iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    #[test]
    fn insert_merges_overlapping_and_adjacent_ranges() {
        let mut set = DownloadedRangeSet::default();
        set.insert(0, 10);
        set.insert(20, 30);
        set.insert(10, 20);

        assert_eq!(ranges(&set), vec![(0, 30)]);
    }

    #[test]
    fn insert_keeps_disjoint_ranges_sorted() {
        let mut set = DownloadedRangeSet::default();
        set.insert(40, 50);
        set.insert(0, 10);
        set.insert(20, 30);

        assert_eq!(ranges(&set), vec![(0, 10), (20, 30), (40, 50)]);
    }

    #[test]
    fn insert_is_idempotent_for_repeated_progress_snapshots() {
        let mut set = DownloadedRangeSet::default();
        set.insert(100, 200);
        set.insert(100, 150);
        set.insert(100, 200);

        assert_eq!(ranges(&set), vec![(100, 200)]);
    }

    #[test]
    fn insert_ignores_empty_or_reversed_ranges() {
        let mut set = DownloadedRangeSet::default();
        set.insert(10, 10);
        set.insert(20, 10);

        assert!(set.ranges.is_empty());
    }
}

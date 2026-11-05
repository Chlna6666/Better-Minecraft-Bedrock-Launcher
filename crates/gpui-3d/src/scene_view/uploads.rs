//! Record-aligned updates preserve unchanged resident draw and culling table entries.
use std::ops::Range;

#[cfg(test)]
pub(super) fn changed_range(previous: &[u8], next: &[u8], stride: usize) -> Option<Range<usize>> {
    let start = previous
        .chunks_exact(stride)
        .zip(next.chunks_exact(stride))
        .take_while(|(a, b)| a == b)
        .count()
        * stride;
    let mut end = next.len();
    while end > start && previous.get(end - stride..end) == Some(&next[end - stride..end]) {
        end -= stride;
    }
    (start < end).then_some(start..end)
}


/// Returns record-aligned dirty ranges for a batched CPU-to-GPU upload.
///
/// A single changed_range() may upload megabytes of unchanged records between
/// two distant dirty sections. Keep separate islands with a bounded write count,
/// coalescing short clean gaps to limit per-write driver overhead. When the
/// scatter limit is exceeded, fall back to one contiguous span rather than
/// producing an unbounded number of GPU write commands.
///
/// A truncated tail needs no upload: the draw/instance count already limits GPU
/// consumers to the live prefix.
pub(super) fn changed_ranges(
    previous: &[u8],
    next: &[u8],
    stride: usize,
    max_ranges: usize,
    coalesce_gap_records: usize,
) -> Vec<Range<usize>> {
    debug_assert!(stride > 0 && next.len().is_multiple_of(stride));
    let mut ranges = Vec::<Range<usize>>::new();
    let mut first_dirty = None;
    let mut final_dirty_end = 0;
    let mut fragmented = false;
    let max_gap_bytes = coalesce_gap_records.saturating_mul(stride);
    for (index, record) in next.chunks_exact(stride).enumerate() {
        let offset = index * stride;
        let end = offset + stride;
        if previous.get(offset..end) == Some(record) {
            continue;
        }
        first_dirty.get_or_insert(offset);
        final_dirty_end = end;
        if fragmented {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if offset - last.end <= max_gap_bytes => {
                last.end = end;
            }
            _ => {
                ranges.push(offset..end);
                if ranges.len() > max_ranges.max(1) {
                    ranges.clear();
                    fragmented = true;
                }
            }
        }
    }
    if fragmented {
        vec![first_dirty.expect("fragmentation requires a dirty record")..final_dirty_end]
    } else {
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_updates_cover_only_changed_records_and_new_tails() {
        let old = [1, 2, 3, 4, 5, 6];
        assert_eq!(changed_range(&old, &[1, 2, 8, 4, 5, 6], 2), Some(2..4));
        assert_eq!(
            changed_range(&old, &[1, 2, 3, 4, 5, 6, 7, 8], 2),
            Some(6..8)
        );
        assert_eq!(changed_range(&old, &old[..4], 2), None);
        assert_eq!(changed_range(&old, &old, 2), None);
        assert_eq!(changed_range(&[], &old, 2), Some(0..6));
    }

    #[test]
    fn sparse_uploads_only_dirty_records_and_keeps_batch_count_bounded() {
        let old = vec![0u8; 64];
        let mut next = old.clone();
        next[8..12].fill(1);
        next[28..32].fill(2);
        next[52..56].fill(3);
        assert_eq!(
            changed_ranges(&old, &next, 4, 8, 0),
            vec![8..12, 28..32, 52..56],
        );
        assert_eq!(
            changed_ranges(&old, &next, 4, 2, 0),
            vec![8..56],
            "too many islands fall back to a single safe span",
        );
        assert_eq!(
            changed_ranges(&old, &next, 4, 8, 4),
            vec![8..32, 52..56],
            "short unmodified gaps are coalesced",
        );
        let mut patched = old.clone();
        for range in changed_ranges(&old, &next, 4, 8, 0) {
            patched[range.clone()].copy_from_slice(&next[range]);
        }
        assert_eq!(patched, next);
    }

    #[test]
    fn range_uploads_support_growth_shrink_and_unchanged_data() {
        let old = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let next = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        assert_eq!(changed_ranges(&old, &next, 4, 8, 0), vec![8..12]);
        assert!(changed_ranges(&next, &old, 4, 8, 0).is_empty());
        assert!(changed_ranges(&old, &old, 4, 8, 0).is_empty());
        assert_eq!(changed_ranges(&[], &old, 4, 8, 0), vec![0..8]);
    }
}

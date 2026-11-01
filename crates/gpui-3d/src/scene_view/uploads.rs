//! Record-aligned updates preserve unchanged resident draw and culling table entries.
use std::ops::Range;

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
}

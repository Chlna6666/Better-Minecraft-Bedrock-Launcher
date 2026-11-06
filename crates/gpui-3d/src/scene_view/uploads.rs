//! Record-aligned updates preserve unchanged resident draw and culling table entries.
use std::{collections::HashMap, ops::Range};

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


/// Move-safe CPU record cache shared by Scene instance rows and GPU culling
/// bounds. Hashes are only a lookup hint: full key equality always determines
/// whether an existing encoded row can be reused.
pub(super) fn encode_cached_records<K: PartialEq, E>(
    current: &[K],
    previous: &[K],
    previous_bytes: &[u8],
    stride: usize,
    output: &mut Vec<u8>,
    fingerprint: impl Fn(&K) -> u64,
    mut encode_range: impl FnMut(Range<usize>, &mut Vec<u8>) -> Result<(), E>,
) -> Result<usize, E> {
    debug_assert!(stride > 0);
    output.clear();
    output.reserve(current.len().max(1).saturating_mul(stride));

    // Fast path: most section updates preserve the ordinal of every draw.
    // Do not allocate a relocation table until a real insertion, removal or
    // nearby reorder indicates that existing rows have shifted positions.
    let mut relocated: Option<(HashMap<u64, usize>, Vec<usize>)> = None;
    let mut dirty_start = None;
    let mut encoded = 0usize;
    for (index, key) in current.iter().enumerate() {
        let start = index * stride;
        let end = start + stride;
        let same_slot = previous.get(index) == Some(key)
            && previous_bytes.get(start..end).is_some();
        let source = if same_slot {
            Some(index)
        } else {
            // Adjacent section insertion/removal shifts records by only a few
            // slots. Check nearby previous locations first, without allocating
            // a map or hashing an entire large scene.
            let nearby = [1isize, -1, 2, -2, 3, -3, 4, -4]
                .into_iter()
                .filter_map(|delta| index.checked_add_signed(delta))
                .find(|&old| {
                    previous.get(old) == Some(key)
                        && previous_bytes
                            .get(old * stride..(old + 1) * stride)
                            .is_some()
                });
            if nearby.is_some() {
                nearby
            } else {
                let shift_hint = current.len() != previous.len()
                    || (index > 0 && previous.get(index - 1) == Some(key))
                    || previous.get(index + 1) == Some(key);
                if shift_hint && relocated.is_none() && !previous.is_empty() {
                    // One allocation for the hash heads and one for the
                    // collision chains; not a Vec allocation for every one of
                    // the thousands of distinct drawn sections.
                    let mut heads = HashMap::<u64, usize>::with_capacity(previous.len());
                    let mut chains = Vec::with_capacity(previous.len());
                    for (old_index, old_key) in previous.iter().enumerate() {
                        let old_start = old_index * stride;
                        let next = if previous_bytes
                            .get(old_start..old_start + stride)
                            .is_some()
                        {
                            heads.insert(fingerprint(old_key), old_index)
                                .unwrap_or(usize::MAX)
                        } else {
                            usize::MAX
                        };
                        chains.push(next);
                    }
                    relocated = Some((heads, chains));
                }
                relocated.as_ref().and_then(|(heads, chains)| {
                    let mut old = *heads.get(&fingerprint(key))?;
                    while old != usize::MAX {
                        if previous.get(old) == Some(key) {
                            return Some(old);
                        }
                        old = chains[old];
                    }
                    None
                })
            }
        };
        if let Some(source) = source {
            if let Some(first) = dirty_start.take() {
                encode_range(first..index, output)?;
                encoded += index - first;
            }
            let old_start = source * stride;
            output.extend_from_slice(&previous_bytes[old_start..old_start + stride]);
        } else if dirty_start.is_none() {
            dirty_start = Some(index);
        }
    }
    if let Some(first) = dirty_start {
        encode_range(first..current.len(), output)?;
        encoded += current.len() - first;
    }
    if current.is_empty() {
        output.resize(stride, 0);
    }
    debug_assert_eq!(output.len(), current.len().max(1) * stride);
    Ok(encoded)
}


/// Returns record-aligned dirty ranges for a batched CPU-to-GPU upload.
///
/// A single changed_range() may upload megabytes of unchanged records between
/// two distant dirty sections. Keep separate islands with a bounded write count,
/// coalescing short clean gaps to limit per-write driver overhead. When the
/// scatter limit is exceeded, preserve the widest clean gaps and merge the
/// remaining islands. This minimizes transfer bytes under the write limit.
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
    assert!(stride > 0);
    debug_assert!(next.len().is_multiple_of(stride));
    dirty_record_ranges(
        next.len() / stride,
        stride,
        max_ranges,
        coalesce_gap_records,
        |index| {
            let start = index * stride;
            let end = start + stride;
            previous.get(start..end) != Some(&next[start..end])
        },
    )
}

/// The encoding cache has already identified the exact GPU-visible row identity.
/// Compare those keys (rather than re-reading 144 or 48 bytes per record) to
/// compute upload spans. A relocated row has the same identity but a different
/// ordinal, so it must still be uploaded at the new offset.
///
/// The key must compare every GPU-visible bit, and must have been committed
/// only after the prior write succeeded. The existing bytewise diff remains
/// the fallback for tables without such committed identities.
pub(super) fn changed_ranges_for_keys<K: PartialEq>(
    previous_keys: &[K],
    next_keys: &[K],
    uploaded_bytes: usize,
    stride: usize,
    max_ranges: usize,
    coalesce_gap_records: usize,
) -> Vec<Range<usize>> {
    assert!(stride > 0);
    dirty_record_ranges(
        next_keys.len(),
        stride,
        max_ranges,
        coalesce_gap_records,
        |index| {
            let end = (index + 1) * stride;
            uploaded_bytes < end || previous_keys.get(index) != Some(&next_keys[index])
        },
    )
}

/// Compute byte upload ranges directly from fixed-size u32 record slices.
///
/// This is used for GPU-generated draw mappings (four u32 words per mapping).
/// It avoids allocating two intermediate vectors of comparison keys just to
/// identify dirty records. Returns byte offsets suitable for BufferWrite.
pub(super) fn changed_ranges_for_u32_records(
    previous: &[u32],
    next: &[u32],
    words_per_record: usize,
    max_ranges: usize,
    coalesce_gap_records: usize,
) -> Vec<Range<usize>> {
    assert!(words_per_record > 0);
    assert!(next.len().is_multiple_of(words_per_record));
    let stride = words_per_record
        .checked_mul(std::mem::size_of::<u32>())
        .expect("mapping record stride overflow");
    dirty_record_ranges(
        next.len() / words_per_record,
        stride,
        max_ranges,
        coalesce_gap_records,
        |index| {
            let start = index * words_per_record;
            let end = start + words_per_record;
            previous.get(start..end) != Some(&next[start..end])
        },
    )
}

fn dirty_record_ranges(
    count: usize,
    stride: usize,
    max_ranges: usize,
    coalesce_gap_records: usize,
    mut is_dirty: impl FnMut(usize) -> bool,
) -> Vec<Range<usize>> {
    let mut islands = Vec::<Range<usize>>::new();
    let max_gap_bytes = coalesce_gap_records.saturating_mul(stride);
    for index in 0..count {
        if !is_dirty(index) {
            continue;
        }
        let offset = index * stride;
        let end = offset + stride;
        match islands.last_mut() {
            Some(last) if offset - last.end <= max_gap_bytes => last.end = end,
            _ => islands.push(offset..end),
        }
    }
    let limit = max_ranges.max(1);
    if islands.len() <= limit {
        return islands;
    }

    // Preserve the widest clean gaps instead of collapsing every scattered
    // update into one nearly full-table transfer. Keeping at most 'limit'
    // spans bounds native driver calls while minimizing the bytes copied.
    // A gap is identified by the island immediately preceding it.
    let mut gaps = (0..islands.len() - 1)
        .map(|index| (islands[index + 1].start - islands[index].end, index))
        .collect::<Vec<_>>();
    gaps.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut splits = gaps
        .into_iter()
        .take(limit - 1)
        .map(|(_, index)| index)
        .collect::<Vec<_>>();
    splits.sort_unstable();

    let mut merged = Vec::with_capacity(limit);
    let mut first = islands[0].start;
    for split in splits {
        merged.push(first..islands[split].end);
        first = islands[split + 1].start;
    }
    merged.push(first..islands.last().expect("nonempty dirty islands").end);
    merged
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
            vec![8..12, 28..56],
            "the widest clean gap is preserved under the write limit",
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
    fn fragmented_updates_keep_widest_gaps_and_preserve_gpu_contents() {
        let old = vec![0u8; 400];
        let mut next = old.clone();
        for index in [1usize, 3, 5, 50, 52, 90] {
            next[index * 4..index * 4 + 4].fill(7);
        }
        let ranges = changed_ranges(&old, &next, 4, 3, 0);
        assert_eq!(ranges, vec![4..24, 200..212, 360..364]);
        let mut patched = old;
        for range in &ranges {
            patched[range.clone()].copy_from_slice(&next[range.clone()]);
        }
        assert_eq!(patched, next);
    }

    #[test]
    fn identity_diff_matches_byte_diff_for_encoded_records() {
        let previous = [1u32, 2, 3, 4, 5, 6, 7, 8];
        let next = [1u32, 2, 20, 4, 5, 60, 7, 8, 9];
        let encode = |values: &[u32]| {
            values.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>()
        };
        let old_bytes = encode(&previous);
        let new_bytes = encode(&next);
        let by_bytes = changed_ranges(&old_bytes, &new_bytes, 4, 16, 0);
        let by_keys = changed_ranges_for_keys(&previous, &next, old_bytes.len(), 4, 16, 0);
        assert_eq!(by_keys, by_bytes);
        let mut patched = old_bytes;
        patched.resize(new_bytes.len(), 0);
        for range in &by_keys {
            patched[range.clone()].copy_from_slice(&new_bytes[range.clone()]);
        }
        assert_eq!(patched, new_bytes);

        let relocated = [3u32, 4, 5, 6, 7, 8];
        let relocated_bytes = encode(&relocated);
        let key_ranges = changed_ranges_for_keys(&next, &relocated, patched.len(), 4, 16, 0);
        let byte_ranges = changed_ranges(&patched, &relocated_bytes, 4, 16, 0);
        assert_eq!(key_ranges, byte_ranges);
        assert!(changed_ranges_for_keys(&relocated, &relocated, relocated_bytes.len(), 4, 16, 0).is_empty());
        assert_eq!(changed_ranges_for_keys(&[], &relocated, 0, 4, 16, 0), vec![0..24]);
    }

    #[test]
    fn identity_diff_bounds_driver_writes_and_keeps_truncated_tail_idle() {
        let previous = (0..48u32).collect::<Vec<_>>();
        let mut next = previous.clone();
        next[3] = 100;
        next[18] = 200;
        next[37] = 300;
        assert_eq!(
            changed_ranges_for_keys(&previous, &next, previous.len() * 144, 144, 16, 0),
            vec![432..576, 2592..2736, 5328..5472]
        );
        assert_eq!(
            changed_ranges_for_keys(&previous, &next, previous.len() * 144, 144, 2, 0),
            vec![432..5472]
        );
        assert!(changed_ranges_for_keys(&previous, &next[..12], previous.len() * 144, 144, 16, 0)
            == vec![432..576]);
        assert_eq!(
            changed_ranges_for_keys(&previous, &next, 0, 144, 16, 0),
            vec![0..next.len()*144]
        );
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
    #[test]
    fn direct_u32_dirty_record_ranges_match_grouped_key_comparisons() {
        let old = (0..512u32).collect::<Vec<_>>();
        let mut next = old.clone();
        for index in [0, 8, 155, 408, 511] {
            next[index] ^= 0x100;
        }
        let old_keys = old.chunks_exact(4)
            .map(|x| [x[0], x[1], x[2], x[3]]).collect::<Vec<_>>();
        let next_keys = next.chunks_exact(4)
            .map(|x| [x[0], x[1], x[2], x[3]]).collect::<Vec<_>>();
        assert_eq!(
            changed_ranges_for_u32_records(&old, &next, 4, 12, 2),
            changed_ranges_for_keys(&old_keys, &next_keys, old.len() * 4, 16, 12, 2),
        );
        assert!(changed_ranges_for_u32_records(&old, &old, 4, 12, 2).is_empty());
        assert!(changed_ranges_for_u32_records(&old, &old[..28], 4, 12, 2).is_empty());
    }

}

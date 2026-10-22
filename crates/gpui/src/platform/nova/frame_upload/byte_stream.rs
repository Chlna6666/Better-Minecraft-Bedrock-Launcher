//! Logical GPU order with writable gaps and shared immutable retained payloads.
use super::*;
use std::ops::Range;

enum Source {
    Owned(Range<usize>),
    Shared { bytes: Arc<Vec<u8>>, byte_hash: u64 },
}

struct Segment {
    offset: usize,
    source: Source,
}

#[derive(Default)]
pub(in crate::platform::nova) struct PackedByteStream {
    owned: Vec<u8>,
    segments: Vec<Segment>,
    len: usize,
}

impl PackedByteStream {
    pub(in crate::platform::nova) fn len(&self) -> usize {
        self.len
    }

    pub(in crate::platform::nova) fn owned_len(&self) -> usize {
        self.owned.len()
    }

    pub(in crate::platform::nova) fn capacity(&self) -> usize {
        self.owned.capacity()
    }

    pub(super) fn metadata_capacity(&self) -> usize {
        self.segments.capacity() * std::mem::size_of::<Segment>()
    }

    pub(in crate::platform::nova) fn clear(&mut self) {
        self.segments.clear();
        self.owned.clear();
        self.len = 0;
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn resize(&mut self, new_len: usize, value: u8) {
        if new_len >= self.len {
            let additional = new_len - self.len;
            self.write(|bytes| bytes.resize(additional, value));
        } else {
            self.truncate(new_len);
        }
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn truncate(&mut self, new_len: usize) {
        assert!(new_len <= self.len);
        if new_len == self.len {
            return;
        }
        while self
            .segments
            .last()
            .is_some_and(|segment| segment.offset >= new_len)
        {
            let Some(segment) = self.segments.pop() else {
                break;
            };
            if let Source::Owned(range) = segment.source {
                self.owned.truncate(range.start);
            } else {
                panic!("test truncation must end before a shared stream segment");
            }
        }
        if let Some(Segment {
            offset,
            source: Source::Owned(range),
        }) = self.segments.last_mut()
        {
            if *offset < new_len && new_len < *offset + range.len() {
                let retained_len = new_len - *offset;
                range.end = range.start + retained_len;
                self.owned.truncate(range.end);
            }
        }
        self.len = new_len;
    }

    pub(in crate::platform::nova) fn write(&mut self, encode: impl FnOnce(&mut Vec<u8>)) {
        let start = self.owned.len();
        encode(&mut self.owned);
        let end = self.owned.len();
        if start == end {
            return;
        }
        if let Some(Segment {
            source: Source::Owned(range),
            ..
        }) = self.segments.last_mut()
        {
            range.end = end;
        } else {
            self.segments.push(Segment {
                offset: self.len,
                source: Source::Owned(start..end),
            });
        }
        self.len += end - start;
    }

    pub(in crate::platform::nova) fn append_shared(&mut self, bytes: Arc<Vec<u8>>, byte_hash: u64) {
        if bytes.is_empty() {
            return;
        }
        let len = bytes.len();
        self.segments.push(Segment {
            offset: self.len,
            source: Source::Shared { bytes, byte_hash },
        });
        self.len += len;
    }

    /// Replace a newly encoded tail with its retained immutable backing.
    pub(in crate::platform::nova) fn share_tail(
        &mut self,
        start: usize,
        bytes: Arc<Vec<u8>>,
        byte_hash: u64,
    ) {
        let tail_len = self.len.saturating_sub(start);
        debug_assert_eq!(tail_len, bytes.len());
        if tail_len == 0 {
            return;
        }
        if let Some(Segment {
            offset,
            source: Source::Owned(range),
        }) = self.segments.last_mut()
        {
            let retained_len = start.saturating_sub(*offset);
            debug_assert!(retained_len <= range.len());
            let retained_end = range.start + retained_len;
            self.owned.truncate(retained_end);
            if retained_len == 0 {
                self.segments.pop();
            } else {
                range.end = retained_end;
            }
        } else {
            debug_assert!(false, "a newly encoded tail must have an owned segment");
            return;
        }
        self.len = start;
        self.append_shared(bytes, byte_hash);
    }

    pub(in crate::platform::nova) fn segments(
        &self,
    ) -> impl Iterator<Item = (usize, &[u8], Option<u64>)> {
        self.segments.iter().map(|segment| {
            let (bytes, hash) = match &segment.source {
                Source::Owned(range) => (&self.owned[range.clone()], None),
                Source::Shared { bytes, byte_hash } => (bytes.as_slice(), Some(*byte_hash)),
            };
            (segment.offset, bytes, hash)
        })
    }

    pub(in crate::platform::nova) fn shared_bytes(&self) -> impl Iterator<Item = &Arc<Vec<u8>>> {
        self.segments
            .iter()
            .filter_map(|segment| match &segment.source {
                Source::Shared { bytes, .. } => Some(bytes),
                Source::Owned(_) => None,
            })
    }

    // A primitive record always belongs to one segment; retained chunks cannot contain animation.
    fn segment_at(&self, offset: usize) -> usize {
        self.segments
            .partition_point(|segment| segment.offset <= offset)
            .checked_sub(1)
            .expect("packed record must belong to a segment")
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn slice(&self, range: Range<usize>) -> &[u8] {
        let segment = &self.segments[self.segment_at(range.start)];
        let start = range.start - segment.offset;
        let end = range.end - segment.offset;
        match &segment.source {
            Source::Owned(source) => &self.owned[source.clone()][start..end],
            Source::Shared { bytes, .. } => &bytes[start..end],
        }
    }

    pub(in crate::platform::nova) fn slice_mut(&mut self, range: Range<usize>) -> &mut [u8] {
        let index = self.segment_at(range.start);
        let segment = &self.segments[index];
        let Source::Owned(source) = &segment.source else {
            unreachable!("animation records are excluded from retained chunks");
        };
        let start = range.start - segment.offset;
        let end = range.end - segment.offset;
        &mut self.owned[source.clone()][start..end]
    }

    /// Intersects a logical dirty range with its physical sources, without flattening.
    pub(in crate::platform::nova) fn slices(
        &self,
        range: Range<usize>,
    ) -> impl Iterator<Item = (usize, &[u8])> {
        debug_assert!(range.start <= range.end && range.end <= self.len);
        let start = self
            .segments
            .partition_point(|segment| segment.offset < range.start);
        let start = start.saturating_sub(1);
        self.segments[start..]
            .iter()
            .take_while(move |segment| segment.offset < range.end)
            .filter_map(move |segment| {
                let bytes = match &segment.source {
                    Source::Owned(source) => &self.owned[source.clone()],
                    Source::Shared { bytes, .. } => bytes.as_slice(),
                };
                let begin = range.start.max(segment.offset);
                let end = range.end.min(segment.offset + bytes.len());
                (begin < end).then(|| (begin, &bytes[begin - segment.offset..end - segment.offset]))
            })
    }

    pub(super) fn trim(&mut self, floor: usize, multiplier: usize) {
        let target = floor.max(self.owned.len());
        if self.owned.capacity() > target.saturating_mul(multiplier) {
            self.owned.shrink_to(target);
        }
        let target = 16.max(self.segments.len());
        if self.segments.capacity() > target.saturating_mul(multiplier) {
            self.segments.shrink_to(target);
        }
    }

    #[cfg(test)]
    pub(in crate::platform::nova) fn to_vec(&self) -> Vec<u8> {
        self.segments()
            .flat_map(|(_, bytes, _)| bytes.iter().copied())
            .collect()
    }
}

#[cfg(test)]
impl From<Vec<u8>> for PackedByteStream {
    fn from(owned: Vec<u8>) -> Self {
        let len = owned.len();
        let segments = if len == 0 {
            Vec::new()
        } else {
            vec![Segment {
                offset: 0,
                source: Source::Owned(0..len),
            }]
        };
        Self {
            owned,
            segments,
            len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_payloads_keep_logical_order_without_owned_copies() {
        let chunk = Arc::new(vec![2; 8]);
        let mut stream = PackedByteStream::default();
        stream.write(|bytes| bytes.extend_from_slice(&[1; 4]));
        stream.append_shared(Arc::clone(&chunk), 9);
        stream.write(|bytes| bytes.extend_from_slice(&[3; 4]));
        assert_eq!(stream.len(), 16);
        assert_eq!(stream.owned_len(), 8);
        assert_eq!(
            stream.shared_bytes().next().unwrap().as_ptr(),
            chunk.as_ptr()
        );
        stream.slice_mut(12..16).copy_from_slice(&[4; 4]);
        assert_eq!(
            stream.to_vec(),
            [vec![1; 4], vec![2; 8], vec![4; 4]].concat()
        );
        let writes: Vec<_> = stream
            .slices(2..14)
            .map(|(offset, bytes)| (offset, bytes.to_vec()))
            .collect();
        assert_eq!(
            writes,
            vec![(2, vec![1; 2]), (4, vec![2; 8]), (12, vec![4; 2])]
        );
        let flat = stream.to_vec();
        for start in 0..=flat.len() {
            for end in start..=flat.len() {
                let actual: Vec<_> = stream
                    .slices(start..end)
                    .flat_map(|(_, bytes)| bytes.iter().copied())
                    .collect();
                assert_eq!(
                    actual,
                    flat[start..end],
                    "dirty intersection {start}..{end}"
                );
            }
        }
        stream.clear();
        assert_eq!(Arc::strong_count(&chunk), 1);
        assert_eq!(stream.len(), 0);
    }

    #[test]
    fn repeated_chunk_references_count_one_backing_even_without_a_cache_entry() {
        let bytes = Arc::new(vec![7; 32]);
        let mut frame = FrameUpload::default();
        frame.quads.append_shared(Arc::clone(&bytes), 1);
        frame.quads.append_shared(Arc::clone(&bytes), 1);
        assert_eq!(frame.quads.len(), 64);
        assert_eq!(frame.quads.owned_len(), 0);
        assert_eq!(
            frame
                .retained_chunk_memory(&RetainedChunkCache::default())
                .used_bytes,
            32
        );
        assert_eq!(
            frame
                .retained_chunk_memory(&RetainedChunkCache::default())
                .capacity_bytes,
            bytes.capacity() as u64
        );
        frame.quads.clear();
        assert_eq!(
            frame.retained_chunk_memory(&RetainedChunkCache::default()),
            crate::MemoryUsage::default()
        );
    }
}

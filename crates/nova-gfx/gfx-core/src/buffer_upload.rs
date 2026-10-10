//! Borrowed buffer upload planning and backend upload accounting.

use std::ops::Range;

use smallvec::SmallVec;

use crate::{BufferId, BufferWriteDescriptor, Error, Result};

/// Borrowed bytes to write into a device-owned buffer.
///
/// The source is needed only for the batch call. A backend must copy it or consume it into
/// owned staging before returning; a batch does not transfer ownership of the destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferWrite<'a> {
    /// Destination buffer and byte offset.
    pub descriptor: BufferWriteDescriptor,
    /// Bytes to upload, without padding or implicit range expansion.
    pub data: &'a [u8],
}

/// Destination upload work accepted by a successful buffer batch.
///
/// Calls count native buffer updates, mapped destination copies, or staging-to-destination
/// copies, rather than map/bind/barrier calls. Bytes include backend range expansion such as
/// DX11's full constant-buffer update. CPU shadow copies are excluded. These are CPU-side
/// accounting values, not GPU completion or timing measurements.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BufferUploadStats {
    /// Number of destination upload operations.
    pub calls: u64,
    /// Destination bytes written by those operations.
    pub bytes: u64,
}

#[derive(Debug)]
struct PlannedWrite<'a> {
    buffer: BufferId,
    source_offset: u64,
    source: &'a [u8],
    range: Range<usize>,
}

/// Plans writes from immutable snapshots or independent segments without copying their bytes.
///
/// Consecutive overlapping or adjacent ranges merge only when their destination and source
/// snapshot are identical. Clean gaps and writes from different snapshots remain separate,
/// preserving caller order and later-write-wins semantics. This is not a staging allocator;
/// callers still submit on the owning device thread and retain destinations through GPU use.
#[derive(Debug, Default)]
pub struct BufferUploadBatch<'a> {
    writes: SmallVec<[PlannedWrite<'a>; 16]>,
    requested_writes: u64,
    requested_bytes: u64,
}

impl<'a> BufferUploadBatch<'a> {
    /// Adds a dirty byte range within the complete destination snapshot.
    ///
    /// Empty ranges are ignored. Source bounds are validated before changing the plan;
    /// destination handles and native buffer sizes are validated by the device at submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] if the range is reversed or exceeds the source snapshot.
    pub fn push(&mut self, buffer: BufferId, source: &'a [u8], range: Range<usize>) -> Result<()> {
        if range.start > range.end || range.end > source.len() {
            return Err(Error::InvalidInput(
                "buffer upload source range out of bounds".into(),
            ));
        }
        if range.is_empty() {
            return Ok(());
        }
        self.plan(buffer, 0, source, range);
        Ok(())
    }

    /// Adds an independent source segment at its logical destination byte offset.
    ///
    /// Empty segments are ignored. The source is borrowed until submission; backends consume or
    /// copy it before returning. Different segments remain separate without a flattening copy.
    /// Destination handles and buffer bounds are validated by the device at submission.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] if the destination offset plus segment length overflows.
    pub fn push_at(&mut self, buffer: BufferId, offset: u64, data: &'a [u8]) -> Result<()> {
        offset.checked_add(data.len() as u64).ok_or_else(|| {
            Error::InvalidInput("buffer upload destination range overflow".into())
        })?;
        if !data.is_empty() {
            self.plan(buffer, offset, data, 0..data.len());
        }
        Ok(())
    }

    fn plan(
        &mut self,
        buffer: BufferId,
        source_offset: u64,
        source: &'a [u8],
        range: Range<usize>,
    ) {
        self.requested_writes = self.requested_writes.saturating_add(1);
        self.requested_bytes = self.requested_bytes.saturating_add(range.len() as u64);
        match self.writes.last_mut() {
            Some(last)
                if last.buffer == buffer
                    && last.source_offset == source_offset
                    && std::ptr::eq(last.source, source)
                    && range.start <= last.range.end
                    && range.end >= last.range.start =>
            {
                last.range.start = last.range.start.min(range.start);
                last.range.end = last.range.end.max(range.end);
            }
            _ => self.writes.push(PlannedWrite {
                buffer,
                source_offset,
                source,
                range,
            }),
        }
        self.merge_tail();
    }

    fn merge_tail(&mut self) {
        while self.writes.len() >= 2 {
            let index = self.writes.len() - 2;
            let previous = &self.writes[index];
            let last = &self.writes[index + 1];
            if previous.buffer != last.buffer
                || previous.source_offset != last.source_offset
                || !std::ptr::eq(previous.source, last.source)
                || previous.range.start > last.range.end
                || previous.range.end < last.range.start
            {
                break;
            }
            let range =
                previous.range.start.min(last.range.start)..previous.range.end.max(last.range.end);
            self.writes.truncate(index + 1);
            self.writes[index].range = range;
        }
    }

    /// Iterates the merged borrowed writes in submission order.
    #[must_use]
    pub fn writes(&self) -> impl ExactSizeIterator<Item = BufferWrite<'a>> + '_ {
        self.writes.iter().map(|write| BufferWrite {
            descriptor: BufferWriteDescriptor {
                buffer: write.buffer,
                offset: write.source_offset + write.range.start as u64,
            },
            data: &write.source[write.range.clone()],
        })
    }

    /// Nonempty dirty ranges requested before merging.
    #[must_use]
    pub fn requested_writes(&self) -> u64 {
        self.requested_writes
    }

    /// Sum of requested range lengths before merging, including repeated overlaps.
    #[must_use]
    pub fn requested_bytes(&self) -> u64 {
        self.requested_bytes
    }
}

pub(crate) fn execute_buffer_batch<'a>(
    writes: impl IntoIterator<Item = BufferWrite<'a>>,
    mut upload: impl FnMut(BufferWrite<'a>) -> Result<BufferUploadStats>,
) -> Result<BufferUploadStats> {
    let mut stats = BufferUploadStats::default();
    for write in writes {
        if write.data.is_empty() {
            continue;
        }
        write
            .descriptor
            .offset
            .checked_add(write.data.len() as u64)
            .ok_or_else(|| {
                Error::InvalidInput("buffer upload destination range overflow".into())
            })?;
        let written = upload(write)?;
        stats.calls = stats.calls.saturating_add(written.calls);
        stats.bytes = stats.bytes.saturating_add(written.bytes);
    }
    Ok(stats)
}

#[cfg(test)]
mod tests;

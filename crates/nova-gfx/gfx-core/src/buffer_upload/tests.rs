use super::*;

fn buffer(index: u32) -> BufferId {
    BufferId::from_parts(index, 1)
}

#[test]
fn adjacent_and_overlapping_ranges_merge_without_copying_clean_gaps() {
    let source = *b"abcdefghijkl";
    let mut batch = BufferUploadBatch::default();
    for range in [0..3, 3..5, 2..6, 8..10] {
        batch.push(buffer(1), &source, range).unwrap();
    }
    let writes: Vec<_> = batch.writes().collect();
    assert_eq!(batch.requested_writes(), 4);
    assert_eq!(batch.requested_bytes(), 11);
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].data, b"abcdef");
    assert_eq!(writes[1].descriptor.offset, 8);
    assert_eq!(writes[1].data, b"ij");
    assert_eq!(writes[0].data.as_ptr(), source.as_ptr());
}

#[test]
fn different_snapshots_and_destinations_preserve_write_order() {
    let first = *b"old!";
    let second = *b"new!";
    let mut batch = BufferUploadBatch::default();
    batch.push(buffer(1), &first, 0..4).unwrap();
    batch.push(buffer(1), &second, 0..4).unwrap();
    batch.push(buffer(2), &second, 0..4).unwrap();
    let mut destinations = [[0; 4]; 2];
    let stats = execute_buffer_batch(batch.writes(), |write| {
        let index = usize::from(write.descriptor.buffer != buffer(1));
        destinations[index].copy_from_slice(write.data);
        Ok(BufferUploadStats {
            calls: 1,
            bytes: write.data.len() as u64,
        })
    })
    .unwrap();
    assert_eq!(
        stats,
        BufferUploadStats {
            calls: 3,
            bytes: 12
        }
    );
    assert_eq!(destinations, [second, second]);
}

#[test]
fn independent_segments_preserve_destination_offsets_and_source_borrows() {
    let chunk = *b"cached";
    let gap = *b"dirty";
    let mut batch = BufferUploadBatch::default();
    batch.push_at(buffer(1), 7, &chunk).unwrap();
    batch.push_at(buffer(1), 13, &gap).unwrap();
    batch.push_at(buffer(1), 19, &chunk).unwrap();
    assert!(batch.push_at(buffer(1), u64::MAX, &gap).is_err());
    let writes: Vec<_> = batch.writes().collect();
    assert_eq!(
        writes.len(),
        3,
        "the same source at different offsets must not merge"
    );
    assert_eq!(batch.requested_bytes(), 17);
    assert_eq!(writes[0].descriptor.offset, 7);
    assert_eq!(writes[1].descriptor.offset, 13);
    assert_eq!(writes[2].descriptor.offset, 19);
    assert_eq!(writes[0].data.as_ptr(), chunk.as_ptr());
    assert_eq!(writes[2].data.as_ptr(), chunk.as_ptr());
}

#[test]
fn a_later_range_bridging_an_earlier_gap_merges_the_whole_run() {
    let source = *b"abcdefghij";
    let mut batch = BufferUploadBatch::default();
    for range in [0..3, 8..10, 3..8] {
        batch.push(buffer(1), &source, range).unwrap();
    }
    let writes: Vec<_> = batch.writes().collect();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].descriptor.offset, 0);
    assert_eq!(writes[0].data, &source);
    assert_eq!(batch.requested_writes(), 3);
    assert_eq!(batch.requested_bytes(), 10);
}

#[test]
fn invalid_ranges_leave_the_plan_unchanged_and_empty_ranges_are_ignored() {
    let source = [0; 4];
    let mut batch = BufferUploadBatch::default();
    batch.push(buffer(1), &source, 0..2).unwrap();
    assert!(batch.push(buffer(1), &source, 0..5).is_err());
    let reversed = Range { start: 3, end: 1 };
    assert!(batch.push(buffer(1), &source, reversed).is_err());
    batch.push(buffer(1), &source, 4..4).unwrap();
    assert_eq!(batch.requested_writes(), 1);
    assert_eq!(batch.writes().len(), 1);
}

#[test]
fn upload_failure_stops_later_writes_and_does_not_claim_atomicity() {
    let source = [1; 3];
    let mut batch = BufferUploadBatch::default();
    for index in 1..=3 {
        batch.push(buffer(index), &source, 0..3).unwrap();
    }
    let mut accepted = Vec::new();
    let result = execute_buffer_batch(batch.writes(), |write| {
        if write.descriptor.buffer == buffer(2) {
            return Err(Error::Backend("injected failure".into()));
        }
        accepted.push(write.descriptor.buffer);
        Ok(BufferUploadStats { calls: 1, bytes: 3 })
    });
    assert!(result.is_err());
    assert_eq!(accepted, [buffer(1)]);
}

#[test]
fn destination_overflow_is_rejected_before_backend_upload() {
    let write = BufferWrite {
        descriptor: BufferWriteDescriptor {
            buffer: buffer(1),
            offset: u64::MAX,
        },
        data: &[1],
    };
    let result = execute_buffer_batch([write], |_| panic!("overflow must not reach backend"));
    assert!(matches!(result, Err(Error::InvalidInput(_))));
}

#[test]
fn empty_writes_do_not_reach_the_backend_or_inflate_accounting() {
    let write = BufferWrite {
        descriptor: BufferWriteDescriptor {
            buffer: buffer(1),
            offset: u64::MAX,
        },
        data: &[],
    };
    let stats = execute_buffer_batch([write], |_| panic!("empty write must be skipped")).unwrap();
    assert_eq!(stats, BufferUploadStats::default());
}

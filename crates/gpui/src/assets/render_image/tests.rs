use super::*;

fn completed_stream() -> RenderImage {
    let frame = |sequence, color| {
        AnimatedFrame::from_bgra_bytes(sequence, size(1.into(), 1.into()), vec![color, 0, 0, 255])
    };
    let image = RenderImage::streaming(
        EncodedImage::new(image::ImageFormat::Gif, Vec::new()),
        frame(0, 0),
        SmallVec::from_vec(vec![frame(1, 10), frame(2, 20)]),
        AnimatedImageConfig::default(),
    );
    let RenderImageStorage::Streaming(state) = &image.storage else {
        panic!("fixture must use streaming storage");
    };
    state.completed.store(true, Ordering::Release);
    image
}

#[test]
fn streaming_consumers_reuse_the_same_delivered_frame() {
    let image = completed_stream();
    let first = image.next_streaming_frame(0).unwrap();
    let second = image.next_streaming_frame(0).unwrap();
    assert_eq!(first.sequence(), 1);
    assert_eq!(second.sequence(), first.sequence());
    assert!(std::ptr::eq(
        first.bytes().as_ptr(),
        second.bytes().as_ptr()
    ));

    let fast = image.next_streaming_frame(1).unwrap();
    let slow = image.next_streaming_frame(1).unwrap();
    assert_eq!(fast.sequence(), 2);
    assert_eq!(slow.sequence(), fast.sequence());
    assert!(std::ptr::eq(fast.bytes().as_ptr(), slow.bytes().as_ptr()));
    assert!(image.next_streaming_frame(2).is_none());
}

#[test]
fn sharing_a_delivered_frame_preserves_queue_accounting() {
    let image = completed_stream();
    let initial = image.resident_byte_len();
    let first = image.next_streaming_frame(0).unwrap();
    let after_delivery = image.resident_byte_len();
    assert_eq!(after_delivery, initial);
    let second = image.next_streaming_frame(0).unwrap();
    assert_eq!(second.sequence(), first.sequence());
    assert_eq!(image.resident_byte_len(), after_delivery);

    image.next_streaming_frame(1).unwrap();
    assert_eq!(image.resident_byte_len(), 8);
    image.next_streaming_frame(1).unwrap();
    assert_eq!(image.resident_byte_len(), 8);
}

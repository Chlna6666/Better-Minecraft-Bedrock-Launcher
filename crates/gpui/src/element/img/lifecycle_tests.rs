use super::loader::ImageRenderRequest;
use super::retained::{SizedImageElementState, SizedImageRequestLease};
use crate::{AssetLocation, ImageRenderSize, ObjectFit, SharedString, TestAppContext};

fn request(label: &'static str, size: u32) -> ImageRenderRequest {
    ImageRenderRequest::new(
        AssetLocation::Embedded(SharedString::from(label)),
        ImageRenderSize::new(size, size).expect("test image size should be valid"),
        1.0,
        ObjectFit::Cover,
    )
}

#[test]
fn dropping_sized_image_state_releases_current_and_pending_leases() {
    let mut test = TestAppContext::single();
    let current = request("current", 256);
    let pending = request("pending", 384);

    test.update(|cx| {
        let mut state = SizedImageElementState::new(None);
        state.sized_image_request = Some(SizedImageRequestLease::acquire(&current, cx));
        state.pending_sized_image_drop = Some(SizedImageRequestLease::acquire(&pending, cx));

        assert_eq!(cx.sized_image_element_ref_count_for_test(&current), 1);
        assert_eq!(cx.sized_image_element_ref_count_for_test(&pending), 1);

        drop(state);
    });

    test.run_until_parked();
    test.read(|cx| {
        assert_eq!(cx.sized_image_element_ref_count_for_test(&current), 0);
        assert_eq!(cx.sized_image_element_ref_count_for_test(&pending), 0);
    });
}

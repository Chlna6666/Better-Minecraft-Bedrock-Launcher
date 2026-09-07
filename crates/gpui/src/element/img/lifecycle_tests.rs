use super::loader::ImageRenderRequest;
use super::retained::{SizedImageElementState, SizedImageRequestLease};
use crate::{AssetLocation, ImageRenderSize, ObjectFit, SharedString, TestAppContext};

struct EvictingImageCache(Option<std::sync::Arc<crate::RenderImage>>);

impl crate::ImageCache for EvictingImageCache {
    fn load(
        &mut self,
        _: &AssetLocation,
        _: &mut crate::Window,
        _: &mut crate::App,
    ) -> Option<Result<std::sync::Arc<crate::RenderImage>, crate::ImageCacheError>> {
        self.0.take().map(Ok)
    }
}

#[gpui::test]
fn ordinary_image_survives_cache_eviction_between_layout_and_paint(cx: &mut TestAppContext) {
    use crate::window::DrawPhase;
    use crate::{AppContext, Drawable, Styled, px, size};

    let window = cx.add_empty_window();
    let cache = window.update(|_, cx| {
        cx.new(|_| {
            EvictingImageCache(Some(std::sync::Arc::new(crate::RenderImage::new(vec![
                image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    2,
                    image::Rgba([255, 0, 0, 255]),
                )),
            ]))))
        })
    });
    window.update(|window, cx| {
        let mut image = Drawable::new(super::img("evicted.png").image_cache(&cache).size(px(20.)));
        window.invalidator.set_phase(DrawPhase::Prepaint);
        image.layout_as_root(size(px(20.), px(20.)).into(), window, cx);
        image.prepaint(window, cx);
        let before = window.next_frame.scene.len();
        window.invalidator.set_phase(DrawPhase::Paint);
        image.paint(window, cx);
        assert!(
            window.next_frame.scene.len() > before,
            "the resolved image must still emit a sprite after cache eviction"
        );
        window.invalidator.set_phase(DrawPhase::None);
    });
}

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

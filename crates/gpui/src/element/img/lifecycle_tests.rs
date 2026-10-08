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
        let view_id = window
            .root::<crate::Empty>()
            .flatten()
            .expect("test window should have an Empty root")
            .entity_id();
        window.with_rendered_view(view_id, |window| {
            image.layout_as_root(size(px(20.), px(20.)).into(), window, cx)
        });
        window.with_rendered_view(view_id, |window| image.prepaint(window, cx));
        let before = window.next_frame.scene.len();
        window.invalidator.set_phase(DrawPhase::Paint);
        window.with_rendered_view(view_id, |window| image.paint(window, cx));
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

#[gpui::test]
fn sized_image_starts_loading_in_prepaint(cx: &mut TestAppContext) {
    use crate::window::DrawPhase;
    use crate::{Drawable, Styled, StyledImage, px, size};

    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let mut image = Drawable::new(
            super::img("prepaint-icon.png")
                .render_to_bounds()
                .size(px(32.0)),
        );
        let initial_assets = cx.asset_entries.len();
        window.invalidator.set_phase(DrawPhase::Prepaint);
        let view_id = window
            .root::<crate::Empty>()
            .flatten()
            .expect("test window should have an Empty root")
            .entity_id();
        window.with_rendered_view(view_id, |window| {
            image.layout_as_root(size(px(32.0), px(32.0)).into(), window, cx)
        });
        assert_eq!(cx.asset_entries.len(), initial_assets);
        window.with_rendered_view(view_id, |window| image.prepaint(window, cx));
        let prepared_assets = cx.asset_entries.len();
        assert!(
            prepared_assets > initial_assets,
            "prepaint must start visible image loading"
        );
        window.invalidator.set_phase(DrawPhase::Paint);
        window.with_rendered_view(view_id, |window| image.paint(window, cx));
        assert_eq!(
            cx.asset_entries.len(),
            prepared_assets,
            "paint must reuse the prepared request"
        );
        window.invalidator.set_phase(DrawPhase::None);
    });
}

#[gpui::test]
fn clipped_sized_image_does_not_start_loading(cx: &mut TestAppContext) {
    use crate::window::DrawPhase;
    use crate::{Bounds, ContentMask, Drawable, Styled, StyledImage, point, px, size};

    let window = cx.add_empty_window();
    window.update(|window, cx| {
        let mut image = Drawable::new(
            super::img("clipped-icon.png")
                .render_to_bounds()
                .size(px(32.0)),
        );
        cx.image_pipeline_config.bounds_policy = crate::ImageBoundsPolicy::Visible;
        let initial_assets = cx.asset_entries.len();
        window.invalidator.set_phase(DrawPhase::Prepaint);
        let view_id = window
            .root::<crate::Empty>()
            .flatten()
            .expect("test window should have an Empty root")
            .entity_id();
        window.with_rendered_view(view_id, |window| {
            image.layout_as_root(size(px(32.0), px(32.0)).into(), window, cx)
        });
        let mask = ContentMask::new(Bounds::new(
            point(px(1000.0), px(1000.0)),
            size(px(32.0), px(32.0)),
        ));
        window.with_rendered_view(view_id, |window| {
            window.with_content_mask(Some(mask), |window| image.prepaint(window, cx))
        });
        assert_eq!(cx.asset_entries.len(), initial_assets);
        window.invalidator.set_phase(DrawPhase::None);
    });
}

#[test]
fn dropping_sized_image_state_releases_current_and_pending_asset_leases() {
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

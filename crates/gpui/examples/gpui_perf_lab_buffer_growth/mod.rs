//! Alternate small/large visible workloads to exercise per-slot buffer growth.
use gpui::{
    Animation, AnimationDirection, AnimationExt as _, AnimationProperty, AnimationSpec,
    PathBuilder, RepeatMode, SharedString, canvas, div, img, point, prelude::*, px, rgb,
};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

pub(super) fn workload(frame: usize) -> gpui::AnyElement {
    static IMAGE: OnceLock<Arc<gpui::RenderImage>> = OnceLock::new();
    let image = IMAGE.get_or_init(|| {
        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_pixel(14, 14, image::Rgba([0x20, 0x80, 0xff, 0xff])),
        )]))
    });
    let count = if (frame / 32) % 2 == 0 { 16 } else { 192 };
    div()
        .relative()
        .size_full()
        .bg(rgb(0x111827))
        .child(
            canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    for index in 0..count {
                        let origin = bounds.origin
                            + point(
                                px((index % 16) as f32 * 48.0),
                                px((index / 16) as f32 * 40.0),
                            );
                        let mut path = PathBuilder::fill();
                        path.move_to(origin + point(px(1.0), px(32.0)));
                        path.line_to(origin + point(px(8.0), px(24.0)));
                        path.line_to(origin + point(px(15.0), px(32.0)));
                        path.close();
                        window.paint_path(
                            path.build().expect("fixture triangle is a valid path"),
                            rgb(0x22c55e),
                        );
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .children((0..count).map(|index| card(index, Arc::clone(image))))
        .into_any_element()
}

fn card(index: usize, image: Arc<gpui::RenderImage>) -> gpui::AnyElement {
    let animation = Animation::from_spec(
        AnimationSpec::new(Duration::from_secs(1))
            .repeat(RepeatMode::Forever)
            .direction(AnimationDirection::Alternate),
    )
    .with_property(AnimationProperty::opacity(0.75, 1.0));
    div()
        .id(SharedString::from(format!("growth-card-{index}")))
        .absolute()
        .left(px((index % 16) as f32 * 48.0))
        .top(px((index / 16) as f32 * 40.0))
        .w(px(44.0))
        .h(px(22.0))
        .bg(rgb(0x334155))
        .shadow_md()
        .text_size(px(10.0))
        .child(img(image).size(px(12.0)))
        .child("ABC")
        .with_visual_animation(
            SharedString::from(format!("growth-animation-{index}")),
            animation,
        )
        .expect("fixture opacity is supported by retained presentation")
        .into_any_element()
}

use super::*;
use std::path::PathBuf;

pub(super) const MANAGE_LIST_THUMBNAIL_RADIUS_PX: f32 = 5.0;

pub(super) fn rounded_manage_thumbnail(
    colors: &ThemeColors,
    image_path: &SharedString,
    placeholder_icon: SharedString,
    background: Hsla,
) -> AnyElement {
    let radius = px(MANAGE_LIST_THUMBNAIL_RADIUS_PX);
    let border_color = Hsla {
        a: 0.18,
        ..colors.border
    };

    let mut thumbnail = div()
        .relative()
        .w(px(32.))
        .h(px(32.))
        .flex_none()
        .rounded(radius)
        .bg(background)
        .overflow_hidden();

    thumbnail = thumbnail.child(
        img(PathBuf::from(image_path.as_ref()))
            .absolute()
            .inset_0()
            .size_full()
            .rounded(radius)
            .object_fit(ObjectFit::Cover)
            .decode_to_bounds()
            .with_fallback({
                let colors = colors.clone();
                let placeholder_icon = placeholder_icon.clone();
                move || {
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(placeholder_icon.clone())
                                .w(px(16.))
                                .h(px(16.))
                                .text_color(colors.text_secondary),
                        )
                        .into_any_element()
                }
            }),
    );

    thumbnail
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius)
                .border_1()
                .border_color(border_color),
        )
        .into_any_element()
}

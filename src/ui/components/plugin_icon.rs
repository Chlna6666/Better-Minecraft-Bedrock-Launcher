use gpui::{
    AnyElement, Div, Hsla, Img, IntoElement, ObjectFit, ParentElement, Pixels, Styled, StyledImage,
    div, px,
};

use crate::ui::components::icon::themed_icon;
use crate::ui::theme::ThemeColors;

pub(crate) fn render(colors: &ThemeColors, size: Pixels, image: Option<Img>) -> AnyElement {
    let colors = *colors;
    let Some(image) = image else {
        return placeholder(&colors, size).into_any_element();
    };
    div()
        .size(size)
        .flex_none()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .bg(colors.surface)
        .child(
            image
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_loading(move || placeholder(&colors, size).into_any_element())
                .with_fallback(move || placeholder(&colors, size).into_any_element()),
        )
        .into_any_element()
}

fn placeholder(colors: &ThemeColors, size: Pixels) -> Div {
    div()
        .size(size)
        .flex_none()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.12,
            ..colors.accent
        })
        .flex()
        .items_center()
        .justify_center()
        .child(themed_icon(
            lucide_gpui::icon!(plug),
            (size / px(1.) * 0.58).clamp(12.0, 24.0),
            colors.accent,
        ))
}

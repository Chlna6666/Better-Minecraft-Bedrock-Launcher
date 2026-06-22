use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

pub(super) fn page_shell(content: impl IntoElement, _colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .left(px(22.))
        .right(px(22.))
        .top(px(92.))
        .bottom(px(20.))
        .p(px(18.))
        .child(content)
}

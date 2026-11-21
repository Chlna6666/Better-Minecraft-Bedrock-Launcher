use gpui::{Div, Hsla, ParentElement, Styled, div, px, relative};

use crate::ui::theme::ThemeColors;
use crate::ui::theme::tokens::radius;

pub(crate) fn render(colors: &ThemeColors, tags: &[String]) -> Option<Div> {
    let mut tags = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .peekable();
    tags.peek()?;
    Some(
        div()
            .w_full()
            .min_w(px(0.))
            .flex()
            .flex_wrap()
            .gap(px(4.))
            .children(tags.map(|tag| {
                div()
                    .min_w(px(0.))
                    .max_w(relative(1.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(radius::XS))
                    .bg(Hsla {
                        a: 0.08,
                        ..colors.text_secondary
                    })
                    .text_size(px(10.))
                    .text_color(colors.text_secondary)
                    .child(tag.to_owned())
            })),
    )
}

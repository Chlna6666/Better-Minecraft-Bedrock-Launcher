//! Lightweight, reusable selection chip for compact tool inspectors.
//! This is a presentation primitive only; the owner supplies the action and state.

use crate::ui::theme::colors::ThemeColors;
use gpui::{div, px, Div, FontWeight, ParentElement, SharedString, Styled};

/// A compact mutually-selectable option. Unlike a primary action button, this
/// communicates state and deliberately does not launch any command by itself.
pub(crate) fn choice_chip(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    selected: bool,
) -> Div {
    div()
        .px(px(10.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(if selected { colors.accent } else { colors.border })
        .bg(if selected { colors.surface_hover } else { colors.surface })
        .text_size(px(12.0))
        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
        .text_color(if selected { colors.accent } else { colors.text_primary })
        .cursor_pointer()
        .child(label.into())
}

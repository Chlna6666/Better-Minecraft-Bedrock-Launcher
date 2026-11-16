//! Shared editor-inspector primitives used by OBJ, image-to-block, and map-item import.
//! Presentation only: the owning view keeps task state, file validation and callbacks.

use std::path::Path;

use gpui::{div, px, Div, FontWeight, IntoElement, ParentElement, SharedString, Styled};
use crate::ui::theme::ThemeColors;

/// A restrained, high-contrast section separator for long parameter inspectors.
/// Unlike a button this carries no hover or interaction state.
pub(crate) fn section_title(
    colors: &ThemeColors,
    title: impl Into<SharedString>,
) -> Div {
    div()
        .pt(px(8.0))
        .pb(px(7.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .w(px(3.0))
                .h(px(13.0))
                .rounded(px(2.0))
                .bg(colors.accent),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(title.into()),
        )
}

/// A stable file-identity card, independent of the OS path length. Source status,
/// browse action, and drag/drop affordance remain visually grouped.
pub(crate) fn source_card(
    colors: &ThemeColors,
    kind: impl Into<SharedString>,
    source: Option<&Path>,
    hint: impl Into<SharedString>,
    browse: impl IntoElement,
) -> Div {
    let file_name = source
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| t!("Inspector.no_file_selected").to_string());
    let folder = source
        .and_then(|path| path.parent())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| t!("Inspector.select_or_drop").to_string());
    div()
        .min_w(px(0.0))
        .p(px(12.0))
        .rounded(px(9.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .rounded(px(5.0))
                        .px(px(7.0))
                        .py(px(4.0))
                        .bg(colors.surface_hover)
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.accent)
                        .child(kind.into()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .child(if source.is_some() {
                            t!("Inspector.asset_imported")
                        } else {
                            t!("Inspector.waiting_import")
                        }),
                ),
        )
        .child(
            div()
                .min_w(px(0.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .truncate()
                .child(file_name),
        )
        .child(
            div()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .text_color(colors.text_secondary)
                .truncate()
                .child(folder),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(7.0))
                .child(browse)
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .child(hint.into()),
                ),
        )
}

/// Compact editor result identity, kept separate from primary actions.
pub(crate) fn result_summary(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Div {
    div()
        .min_w(px(0.0))
        .p(px(10.0))
        .rounded(px(7.0))
        .bg(colors.surface_hover)
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(10.0))
                .text_color(colors.text_secondary)
                .child(label.into()),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(value.into()),
        )
}

 /// A navigable workflow step. The caller provides the actual action, so there
 /// are no placeholder clicks or state machines inside this presentation primitive.
pub(crate) fn stage_tab(
    colors: &ThemeColors,
    ordinal: &'static str,
    label: impl Into<SharedString>,
    active: bool,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(7.0))
        .px(px(11.0))
        .py(px(9.0))
        .rounded(px(7.0))
        .border_1()
        .border_color(if active { colors.accent } else { colors.border })
        .bg(if active { colors.surface_hover } else { colors.surface })
        .text_color(if active { colors.accent } else { colors.text_primary })
        .cursor_pointer()
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(ordinal),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .child(label.into()),
        )
}

 /// Surface for a cohesive field set. The owner adds its real inputs as children;
 /// no duplicate form state or validation lives here.
pub(crate) fn field_group(colors: &ThemeColors) -> Div {
    div()
        .min_w(px(0.0))
        .p(px(12.0))
        .rounded(px(9.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.settings_field_bg)
        .flex()
        .flex_col()
        .gap(px(12.0))
}

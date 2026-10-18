//! Shared editor-inspector primitives used by OBJ, image-to-block, and map-item import.
//! Presentation only: the owning view keeps task state, file validation and callbacks.

use std::path::Path;

use gpui::{div, px, Div, FontWeight, IntoElement, ParentElement, SharedString, Styled};
use crate::ui::theme::colors::ThemeColors;

/// A restrained, high-contrast section separator for long parameter inspectors.
/// Unlike a button this carries no hover or interaction state.
pub(crate) fn inspector_section_title(
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
pub(crate) fn inspector_source_card(
    colors: &ThemeColors,
    kind: &'static str,
    source: Option<&Path>,
    hint: &'static str,
    browse: impl IntoElement,
) -> Div {
    let file_name = source
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "尚未选择文件".to_owned());
    let folder = source
        .and_then(|path| path.parent())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "可通过文件选择器或拖入文件导入".to_owned());
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
                        .child(kind),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .child(if source.is_some() { "已导入素材" } else { "等待导入" }),
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
                        .child(hint),
                ),
        )
}

/// Compact editor result identity, kept separate from primary actions.
pub(crate) fn inspector_result_summary(
    colors: &ThemeColors,
    label: &'static str,
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
        .child(div().text_size(px(10.0)).text_color(colors.text_secondary).child(label))
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
pub(crate) fn inspector_stage_tab(
    colors: &ThemeColors,
    ordinal: &'static str,
    label: &'static str,
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
                .child(label),
        )
}

 /// Surface for a cohesive field set. The owner adds its real inputs as children;
 /// no duplicate form state or validation lives here.
pub(crate) fn inspector_field_group(colors: &ThemeColors) -> Div {
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

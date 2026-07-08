use super::preview::{SkinPreviewWindowSkin, SkinPreviewWindowView};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::ops::Range;
use std::path::PathBuf;

const CURRENT_PREVIEW_SIZE: f32 = 38.0;
const SELECTOR_COLLAPSED_ITEM_SIZE: f32 = 58.0;
const SELECTOR_COLLAPSED_IMAGE_SIZE: f32 = 50.0;
const SELECTOR_EXPANDED_ITEM_SIZE: f32 = 46.0;
const SELECTOR_EXPANDED_IMAGE_SIZE: f32 = 40.0;
const SELECTOR_EXPANDED_MAX_HEIGHT: f32 = 180.0;
const SELECTOR_MAX_WIDTH: f32 = 584.0;
const SELECTOR_PAGE_SIZE: usize = 30;

pub(super) fn skin_selector_page_count(skin_count: usize) -> usize {
    if skin_count == 0 {
        0
    } else {
        (skin_count - 1) / SELECTOR_PAGE_SIZE + 1
    }
}

pub(super) fn skin_selector_page_for_index(index: usize) -> usize {
    index / SELECTOR_PAGE_SIZE
}

pub(super) fn skin_selector_range(
    skin_count: usize,
    expanded: bool,
    page_index: usize,
) -> Range<usize> {
    if expanded {
        return 0..skin_count;
    }

    let page_count = skin_selector_page_count(skin_count);
    let page_index = page_index.min(page_count.saturating_sub(1));
    let page_start = page_index
        .saturating_mul(SELECTOR_PAGE_SIZE)
        .min(skin_count);
    let page_end = page_start
        .saturating_add(SELECTOR_PAGE_SIZE)
        .min(skin_count);
    page_start..page_end
}

pub(super) fn render_current_preview(
    skin: Option<&SkinPreviewWindowSkin>,
    colors: &ThemeColors,
) -> Div {
    render_skin_image_frame(
        skin.and_then(|skin| skin.preview_path.as_ref()),
        colors,
        CURRENT_PREVIEW_SIZE,
        8.0,
        false,
    )
}

pub(super) fn render_skin_selector(
    skins: &[SkinPreviewWindowSkin],
    selected_index: usize,
    expanded: bool,
    page_index: usize,
    colors: &ThemeColors,
    cx: &mut Context<SkinPreviewWindowView>,
) -> Div {
    let page_count = skin_selector_page_count(skins.len());
    let page_index = page_index.min(page_count.saturating_sub(1));
    let selector_range = skin_selector_range(skins.len(), expanded, page_index);
    let item_size = if expanded {
        SELECTOR_EXPANDED_ITEM_SIZE
    } else {
        SELECTOR_COLLAPSED_ITEM_SIZE
    };
    let image_size = if expanded {
        SELECTOR_EXPANDED_IMAGE_SIZE
    } else {
        SELECTOR_COLLAPSED_IMAGE_SIZE
    };
    let item_radius = if expanded { 9.0 } else { 10.0 };
    let image_radius = if expanded { 6.0 } else { 7.0 };
    let mut list = div()
        .flex()
        .items_center()
        .gap(px(if expanded { 6.0 } else { 8.0 }))
        .py(px(1.0));
    if expanded {
        list = list.flex_wrap().content_start().items_start();
    }
    for (offset, skin) in skins[selector_range.clone()].iter().enumerate() {
        let index = selector_range.start + offset;
        list = list.child(render_skin_selector_item(
            index,
            skin,
            index == selected_index,
            colors,
            item_size,
            image_size,
            item_radius,
            image_radius,
            cx,
        ));
    }

    let scroller = if expanded {
        div()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .max_h(px(SELECTOR_EXPANDED_MAX_HEIGHT))
            .overflow_y_scrollbar()
            .scrollbar_width(px(4.0))
            .child(list)
    } else {
        div()
            .flex_1()
            .min_w(px(0.0))
            .max_w(px(SELECTOR_MAX_WIDTH))
            .overflow_x_scrollbar()
            .scrollbar_width(px(4.0))
            .child(list)
    };

    let content = div()
        .flex_1()
        .min_w(px(0.0))
        .max_w(px(SELECTOR_MAX_WIDTH))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .when(expanded, |this| {
            this.child(render_selector_summary(
                skins,
                selected_index,
                expanded,
                page_index,
                page_count,
                colors,
            ))
        })
        .child(scroller);

    div()
        .border_t_1()
        .border_color(colors.border)
        .bg(colors.settings_panel_bg)
        .px(px(16.0))
        .py(px(8.0))
        .flex()
        .items_start()
        .justify_center()
        .gap(px(10.0))
        .child(content)
        .when(!expanded && page_count > 1, |this| {
            this.child(
                selector_icon_button(
                    colors,
                    "skin-preview-selector-page-previous",
                    lucide_gpui::icons::icon_chevron_left(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.select_previous_selector_page(cx);
                        cx.stop_propagation();
                    }),
                ),
            )
            .child(
                selector_icon_button(
                    colors,
                    "skin-preview-selector-page-next",
                    lucide_gpui::icons::icon_chevron_right(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.select_next_selector_page(cx);
                        cx.stop_propagation();
                    }),
                ),
            )
        })
        .child(selector_toggle_button(colors, expanded).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.toggle_selector_expanded(cx);
                cx.stop_propagation();
            }),
        ))
}

fn render_selector_summary(
    skins: &[SkinPreviewWindowSkin],
    selected_index: usize,
    expanded: bool,
    page_index: usize,
    page_count: usize,
    colors: &ThemeColors,
) -> Div {
    let selected_name = skins
        .get(selected_index)
        .map(|skin| skin.display_name.clone())
        .unwrap_or_else(|| SharedString::from("皮肤"));
    let count = if skins.is_empty() {
        SharedString::from("0/0")
    } else {
        let selected_counter = format!("{}/{}", selected_index + 1, skins.len());
        if expanded {
            SharedString::from(selected_counter)
        } else {
            SharedString::from(format!(
                "{selected_counter} · {}/{}",
                page_index + 1,
                page_count.max(1)
            ))
        }
    };

    div()
        .h(px(18.0))
        .min_w(px(0.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(11.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_secondary)
                .child(selected_name),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(colors.text_secondary)
                .child(count),
        )
}

fn selector_icon_button(
    colors: &ThemeColors,
    id: &'static str,
    icon: &'static str,
) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.0))
        .h(px(34.0))
        .flex_none()
        .rounded(px(8.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(
            svg()
                .path(icon)
                .w(px(15.0))
                .h(px(15.0))
                .text_color(colors.text_secondary),
        )
}

fn selector_toggle_button(colors: &ThemeColors, expanded: bool) -> Stateful<Div> {
    selector_icon_button(
        colors,
        "skin-preview-selector-toggle",
        if expanded {
            lucide_gpui::icons::icon_chevron_down()
        } else {
            lucide_gpui::icons::icon_chevron_up()
        },
    )
}

fn render_skin_selector_item(
    index: usize,
    skin: &SkinPreviewWindowSkin,
    selected: bool,
    colors: &ThemeColors,
    item_size: f32,
    image_size: f32,
    item_radius: f32,
    image_radius: f32,
    cx: &mut Context<SkinPreviewWindowView>,
) -> Div {
    let border_color = if selected {
        colors.accent
    } else {
        Hsla {
            a: 0.20,
            ..colors.border
        }
    };
    let background = if selected {
        Hsla {
            a: 0.10,
            ..colors.accent
        }
    } else {
        colors.surface
    };

    div()
        .relative()
        .w(px(item_size))
        .h(px(item_size))
        .min_w(px(item_size))
        .max_w(px(item_size))
        .min_h(px(item_size))
        .max_h(px(item_size))
        .flex_none()
        .rounded(px(item_radius))
        .border_1()
        .border_color(border_color)
        .bg(background)
        .p(px(((item_size - image_size) * 0.5 - 1.0).max(2.0)))
        .cursor_pointer()
        .child(render_skin_image_frame(
            skin.preview_path.as_ref(),
            colors,
            image_size,
            image_radius,
            selected,
        ))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.select_skin(index, cx);
                cx.stop_propagation();
            }),
        )
}

fn render_skin_image_frame(
    preview_path: Option<&SharedString>,
    colors: &ThemeColors,
    size_px: f32,
    radius_px: f32,
    selected: bool,
) -> Div {
    let radius = px(radius_px);
    let border_color = if selected {
        Hsla {
            a: 0.58,
            ..colors.accent
        }
    } else {
        Hsla {
            a: 0.18,
            ..colors.border
        }
    };
    let mut frame = div()
        .relative()
        .w(px(size_px))
        .h(px(size_px))
        .min_w(px(size_px))
        .max_w(px(size_px))
        .min_h(px(size_px))
        .max_h(px(size_px))
        .flex_none()
        .rounded(radius)
        .bg(colors.surface)
        .overflow_hidden();

    frame = if let Some(path) = preview_path {
        frame.child(
            img(PathBuf::from(path.as_ref()))
                .absolute()
                .inset_0()
                .size_full()
                .rounded(radius)
                .object_fit(ObjectFit::Contain)
                .decode_to_bounds()
                .with_fallback({
                    let colors = colors.clone();
                    move || skin_preview_placeholder(&colors).into_any_element()
                }),
        )
    } else {
        frame.child(skin_preview_placeholder(colors))
    };

    frame.child(
        div()
            .absolute()
            .inset_0()
            .rounded(radius)
            .border_1()
            .border_color(border_color),
    )
}

fn skin_preview_placeholder(colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(lucide_gpui::icons::icon_user())
                .w(px(17.0))
                .h(px(17.0))
                .text_color(colors.text_secondary),
        )
}

#[cfg(test)]
#[path = "selector_tests.rs"]
mod tests;

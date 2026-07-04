use super::preview::{SkinPreviewWindowSkin, SkinPreviewWindowView};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::*;
use std::path::PathBuf;

const CURRENT_PREVIEW_SIZE: f32 = 38.0;
const SELECTOR_ITEM_SIZE: f32 = 58.0;
const SELECTOR_IMAGE_SIZE: f32 = 50.0;

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
    colors: &ThemeColors,
    cx: &mut Context<SkinPreviewWindowView>,
) -> Div {
    let mut row = div().flex().items_center().gap(px(8.0)).py(px(1.0));
    for (index, skin) in skins.iter().enumerate() {
        row = row.child(render_skin_selector_item(
            index,
            skin,
            index == selected_index,
            colors,
            cx,
        ));
    }

    div()
        .border_t_1()
        .border_color(colors.border)
        .bg(colors.settings_panel_bg)
        .px(px(16.0))
        .py(px(10.0))
        .child(
            div()
                .w_full()
                .overflow_x_scrollbar()
                .scrollbar_width(px(4.0))
                .child(row),
        )
}

fn render_skin_selector_item(
    index: usize,
    skin: &SkinPreviewWindowSkin,
    selected: bool,
    colors: &ThemeColors,
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
        .w(px(SELECTOR_ITEM_SIZE))
        .h(px(SELECTOR_ITEM_SIZE))
        .flex_none()
        .rounded(px(10.0))
        .border_1()
        .border_color(border_color)
        .bg(background)
        .p(px(3.0))
        .cursor_pointer()
        .child(render_skin_image_frame(
            skin.preview_path.as_ref(),
            colors,
            SELECTOR_IMAGE_SIZE,
            7.0,
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
                .object_fit(ObjectFit::Cover)
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

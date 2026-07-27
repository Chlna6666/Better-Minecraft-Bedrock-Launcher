use crate::ui::{
    components::{
        button::{ghost_button, primary_button},
        modal::{self, ModalDismissHandle},
        scroll::ScrollableElement,
    },
    theme::ThemeColors,
};
use gpui::*;
use std::rc::Rc;

/// Reusable modal dialog container shell with consistent styling, padding, and layout.
pub fn dialog_container(colors: &ThemeColors, max_width: Pixels) -> Div {
    div()
        .w_full()
        .max_w(max_width)
        .max_h(relative(1.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .overflow_hidden()
        .flex()
        .flex_col()
}

/// Header section for standard dialogs (Title + optional Description).
pub fn dialog_header(
    colors: &ThemeColors,
    title: impl Into<SharedString>,
    description: Option<impl Into<SharedString>>,
) -> Div {
    let mut header = div().p(px(22.)).flex().flex_col().gap(px(10.)).child(
        div()
            .text_size(px(17.))
            .font_weight(FontWeight::BOLD)
            .text_color(colors.text_primary)
            .child(title.into()),
    );

    if let Some(desc) = description {
        header = header.child(
            div()
                .text_size(px(13.))
                .line_height(relative(1.5))
                .text_color(colors.text_secondary)
                .child(desc.into()),
        );
    }

    header
}

/// Action buttons footer for standard dialogs.
pub fn dialog_actions(
    colors: &ThemeColors,
    cancel_button: impl IntoElement,
    confirm_button: impl IntoElement,
) -> Div {
    div()
        .px(px(22.))
        .pb(px(22.))
        .min_h(px(62.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_end()
        .gap(px(10.))
        .child(cancel_button)
        .child(confirm_button)
}

/// Unified Confirmation Dialog Component.
pub fn confirm_dialog(
    colors: &ThemeColors,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    confirm_label: impl Into<SharedString>,
    danger: bool,
    pending: bool,
    dismiss_handle: ModalDismissHandle,
    on_dismiss: Rc<dyn Fn(&mut App)>,
    on_confirm: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let cancel_dismiss = dismiss_handle.clone();
    let confirm_label: SharedString = confirm_label.into();
    let mut cancel_button = ghost_button(colors, "dialog-confirm-cancel", "取消");
    if pending {
        cancel_button = cancel_button.opacity(0.55);
    } else {
        cancel_button = cancel_button.on_mouse_down(MouseButton::Left, move |_, _, cx| {
            cancel_dismiss.dismiss(cx);
        });
    }
    let mut confirm_button = primary_button(
        colors,
        "dialog-confirm-save",
        if pending {
            SharedString::from("处理中...")
        } else {
            confirm_label
        },
    )
    .bg(if danger { colors.danger } else { colors.accent })
    .opacity(if pending { 0.72 } else { 1.0 });
    if !pending {
        confirm_button = confirm_button.on_mouse_down(MouseButton::Left, on_confirm);
    }

    let content = dialog_container(colors, px(480.))
        .child(
            dialog_header(colors, title, Some(description))
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar(),
        )
        .child(dialog_actions(colors, cancel_button, confirm_button));

    modal::modal_layer_dismissible_with_handle(dismiss_handle, content, colors.backdrop, on_dismiss)
}

/// Unified Single-Input Prompt Dialog Component.
pub fn prompt_dialog(
    colors: &ThemeColors,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    input_element: impl IntoElement,
    confirm_label: impl Into<SharedString>,
    pending: bool,
    dismiss_handle: ModalDismissHandle,
    on_dismiss: Rc<dyn Fn(&mut App)>,
    on_confirm: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let cancel_dismiss = dismiss_handle.clone();
    let confirm_label: SharedString = confirm_label.into();

    let content = dialog_container(colors, px(520.))
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .p(px(22.))
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(relative(1.5))
                        .text_color(colors.text_secondary)
                        .child(description.into()),
                )
                .child(input_element),
        )
        .child(dialog_actions(
            colors,
            ghost_button(colors, "dialog-prompt-cancel", "取消").on_mouse_down(
                MouseButton::Left,
                move |_, _, cx| {
                    cancel_dismiss.dismiss(cx);
                },
            ),
            primary_button(
                colors,
                "dialog-prompt-save",
                if pending {
                    SharedString::from("处理中...")
                } else {
                    confirm_label
                },
            )
            .opacity(if pending { 0.72 } else { 1.0 })
            .on_mouse_down(MouseButton::Left, on_confirm),
        ));

    modal::modal_layer_dismissible_with_handle(dismiss_handle, content, colors.backdrop, on_dismiss)
}

#[cfg(test)]
mod tests {
    use super::{dialog_actions, dialog_container};
    use crate::ui::theme::light_colors;
    use gpui::{AlignItems, Overflow, Refineable, Style, Styled, div, px, relative};

    #[test]
    fn dialog_container_is_bounded_by_the_available_window_height() {
        let mut container = dialog_container(light_colors(), px(480.));
        let mut style = Style::default();
        style.refine(container.style());

        assert_eq!(style.max_size.height, relative(1.0).into());
        assert_eq!(style.overflow.x, Overflow::Hidden);
        assert_eq!(style.overflow.y, Overflow::Hidden);
    }

    #[test]
    fn dialog_actions_reserve_button_height_and_bottom_spacing() {
        let mut actions = dialog_actions(light_colors(), div(), div());
        let mut style = Style::default();
        style.refine(actions.style());

        assert_eq!(style.min_size.height, px(62.).into());
        assert_eq!(style.padding.bottom, px(22.).into());
        assert_eq!(style.align_items, Some(AlignItems::Center));
        assert_eq!(style.flex_shrink, 0.0);
    }
}

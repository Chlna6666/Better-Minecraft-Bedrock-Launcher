use super::model::*;
use super::panels::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

impl MapViewerWindowView {
    pub(super) fn render_right_dock(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(self.ui_state.right_panel_width))
            .flex_none()
            .h_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(Hsla {
                a: CHROME_SURFACE_ALPHA,
                ..colors.surface
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.cancel_pointer_captures_for_panel_interaction("right dock mouse down", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _event, _window, cx| {
                    this.cancel_pointer_captures_for_panel_interaction(
                        "right dock right mouse down",
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
            .child(match self.ui_state.active_right_panel {
                MapViewerRightPanel::Nbt => self.render_nbt_right_panel(colors, cx),
                MapViewerRightPanel::Preview3d => {
                    self.render_preview_3d_panel(colors, cx).into_any_element()
                }
            })
    }

    pub(super) fn render_nbt_right_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dirty_label = if self.editor_document.dirty {
            "已修改"
        } else {
            "同步"
        };
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(46.0))
                    .flex_none()
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(Hsla {
                        a: CHROME_HAIRLINE_ALPHA,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(self.editor_document.title.clone()),
                    )
                    .child(status_badge(colors, dirty_label))
                    .child(dock_close_button(colors).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.close_right_panel(cx)),
                    ))
                    .child(
                        div()
                            .id("nbt-save")
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(Hsla {
                                a: CHROME_HAIRLINE_ALPHA,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: CHROME_ELEVATED_ALPHA,
                                ..colors.surface_hover
                            })
                            .hover(|style| {
                                style.bg(Hsla {
                                    a: CHROME_ELEVATED_ALPHA + 0.15,
                                    ..colors.surface_hover
                                })
                            })
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(themed_icon(
                                lucide_icons::icon_save(),
                                CHROME_TAB_ICON_SIZE,
                                colors.text_primary,
                            ))
                            .child(SharedString::from("保存"))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.request_editor_save(cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.cancel_pointer_captures_for_panel_interaction(
                                "code editor wrapper mouse down",
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _event, _window, cx| {
                            this.cancel_pointer_captures_for_panel_interaction(
                                "code editor wrapper right mouse down",
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
                    .child(
                        CodeEditor::new(&self.editor_state, colors)
                            .size_full()
                            .min_w(px(0.0))
                            .min_h(px(0.0)),
                    ),
            )
            .into_any_element()
    }
}

use super::editor::*;
use super::model::*;
use super::panels::*;
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn render_professional_detail_panel(
        &self,
        colors: &ThemeColors,
        detail: &ProfessionalDetail,
        cx: &mut Context<Self>,
    ) -> Div {
        overlay_panel(colors)
            .right(px(16.0))
            .top(px(86.0))
            .w(px(420.0))
            .max_h(px((self.viewport.height - 140.0).max(260.0)))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child(detail.title()),
                    )
                    .child(toolbar_button(colors, "关闭").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_professional_detail(None, cx);
                            cx.notify();
                        }),
                    )),
            )
            .when_some(detail.editor_sections(), |this, sections| {
                this.child(render_editor_sections(colors, sections))
            })
            .when_some(detail.edit_target(), |this, target| {
                this.child(div().flex().items_center().gap(px(8.0)).children(
                    editor_action_buttons(
                        colors,
                        target,
                        self.professional.write_mode,
                        self.professional.pending_edit_confirmation.as_ref(),
                        cx,
                    ),
                ))
            })
            .child(
                div()
                    .max_h(px((self.viewport.height - 210.0).max(200.0)))
                    .overflow_y_scrollbar()
                    .px(px(8.0))
                    .py(px(8.0))
                    .rounded(px(6.0))
                    .bg(Hsla {
                        a: 0.46,
                        ..colors.surface_hover
                    })
                    .text_size(px(11.0))
                    .line_height(px(17.0))
                    .text_color(colors.text_primary)
                    .child(detail.json()),
            )
    }
}

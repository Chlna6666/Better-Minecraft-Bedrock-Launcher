use super::*;

impl View {
    pub(in super::super) fn image_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_generator_busy();
        div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .when(self.image_generator.preview_result_active && !busy, |bar| {
                bar.child(
                    crate::ui::components::button::primary_button(
                        colors,
                        "image-workspace-place",
                        t!("MapViewer.generator.image_generator.render.choose_placement"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.set_import_workspace_mode(
                                super::super::state::ImportWorkspaceMode::Placement,
                                cx,
                            )
                        }),
                    ),
                )
            })
            .child(
                crate::ui::components::button::secondary_button(
                    colors,
                    "image-workspace-export",
                    if busy {
                        t!("MapViewer.generator.image_generator.render.processing")
                    } else {
                        t!("MapViewer.generator.image_generator.render.export_mcstructure")
                    },
                )
                .opacity(if self.image_generator.source.is_some() && !busy {
                    1.0
                } else {
                    0.45
                })
                .when(self.image_generator.source.is_some() && !busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.start_image_export(cx)),
                    )
                }),
            )
    }

    pub(in super::super) fn image_generator_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.image_generator_busy();

        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.render_import_panel_header(
                colors,
                t!("MapViewer.generator.image_generator.render.panel_title"),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.generator.image_generator.render.section_source"),
                    ))
                    .child(crate::ui::components::inspector::source_card(
                        colors,
                        "IMAGE",
                        self.image_generator.source.as_deref(),
                        t!("MapViewer.generator.image_generator.render.source_hint"),
                        crate::ui::components::button::secondary_button(
                            colors,
                            "image-block-source",
                            t!("MapViewer.generator.image_generator.render.change_image"),
                        )
                        .when(!busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.choose_generator_image(cx)),
                            )
                        })
                        .opacity(if busy { 0.45 } else { 1.0 }),
                    ))
                    .when(self.image_generator.preview_refresh_pending, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child(t!(
                                    "MapViewer.generator.image_generator.render.parameters_updated_regenerating"
                                )),
                        )
                    })
                    .when_some(self.image_generator.preview_size, |panel, size| {
                        panel.child(crate::ui::components::inspector::result_summary(
                            colors,
                            t!("MapViewer.generator.image_generator.render.result_label"),
                            t!(
                                "MapViewer.generator.image_generator.render.result_size",
                                width = size[0],
                                height = size[1],
                                depth = size[2],
                            ),
                        ))
                    })
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.generator.image_generator.render.section_output_size"),
                    ))
                    .child(crate::ui::components::inspector::field_group(colors)
                        .child(self.render_import_height(colors, cx)))
                    .child(
                        crate::ui::components::inspector::field_group(colors)
                            .child(self.render_image_dimension(ImageAxis::Width, colors, cx))
                            .child(self.render_image_dimension(ImageAxis::Height, colors, cx)),
                    )
                    .child(crate::ui::components::inspector::result_summary(
                        colors,
                        t!("MapViewer.generator.image_generator.render.canvas_label"),
                        t!(
                            "MapViewer.generator.image_generator.render.canvas_size",
                            width = self.image_generator.options.width,
                            height = self.image_generator.options.height,
                        ),
                    ))
                    .child(inspector::section_title(
                        colors,
                        t!(
                            "MapViewer.generator.image_generator.render.section_appearance_and_material"
                        ),
                    ))
                    .child(
                        crate::ui::components::inspector::field_group(colors)
                            .child(self.render_image_dithering(colors, cx))
                            .child(self.render_image_depth_controls(colors, cx))
                            .child(self.render_image_appearance(colors, cx)),
                    ),
            )
            .into_any_element()
    }
}

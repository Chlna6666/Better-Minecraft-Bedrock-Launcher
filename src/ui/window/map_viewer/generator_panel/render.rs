use super::*;

impl View {
    pub(in super::super) fn obj_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.generator_busy();
        // Keep import/export/placement in the same toolbar as the Preview/Placement
        // workflow tabs. A second full-width floating action card consumed a
        // large part of the model canvas and disconnected the actions from the
        // step that owns them.
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .px(px(5.0))
            .py(px(4.0))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface_hover)
            .child(
                crate::ui::components::button::secondary_button(
                    colors,
                    "obj-workspace-import",
                    if self.generator.source.is_some() {
                        t!("MapViewer.generator.render.change_model")
                    } else {
                        t!("MapViewer.generator.render.import_model")
                    },
                )
                .px(px(12.0))
                .py(px(7.0))
                .opacity(if busy { 0.45 } else { 1.0 })
                .when(!busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.choose_generator_obj(cx)),
                    )
                }),
            )
            .when(
                self.generator.preview_result_active
                    && self.generator.preview_displayed_options == Some(self.generator.options)
                    && !busy,
                |bar| {
                bar.child(
                    crate::ui::components::button::primary_button(
                        colors,
                        "obj-workspace-place",
                        t!("MapViewer.generator.render.place_in_world"),
                    )
                    .px(px(12.0))
                    .py(px(7.0))
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
                    "obj-workspace-export",
                    if busy {
                        t!("MapViewer.generator.render.converting")
                    } else {
                        t!("MapViewer.generator.render.export_structure")
                    },
                )
                .px(px(12.0))
                .py(px(7.0))
                .opacity(if self.generator.source.is_some() && !busy {
                    1.0
                } else {
                    0.45
                })
                .when(self.generator.source.is_some() && !busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.export_generator_obj(cx)),
                    )
                }),
            )
            .child(
                crate::ui::components::button::secondary_button(
                    colors,
                    "model-workspace-export",
                    t!("MapViewer.generator.render.export_model"),
                )
                .px(px(12.0))
                .py(px(7.0))
                .opacity(if self.generator.source.is_some() && !busy {
                    1.0
                } else {
                    0.45
                })
                .when(self.generator.source.is_some() && !busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.export_generator_model_file(cx)),
                    )
                }),
            )
    }

    pub(in super::super) fn generator_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.generator_busy();

        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.render_import_panel_header(
                colors,
                t!("MapViewer.generator.render.panel_title"),
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
                        t!("MapViewer.generator.render.section_source"),
                    ))
                    .child(crate::ui::components::inspector::source_card(
                        colors,
                        "OBJ / MTL · glTF / GLB",
                        self.generator.source.as_deref(),
                        t!("MapViewer.generator.render.source_hint"),
                        crate::ui::components::button::secondary_button(
                            colors,
                            "obj-inspector-source",
                            t!("MapViewer.generator.render.choose_model"),
                        )
                        .when(!busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.choose_generator_obj(cx)),
                            )
                        })
                        .opacity(if busy { 0.45 } else { 1.0 }),
                    ))
                    .when(busy, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child(t!(
                                    "MapViewer.generator.render.background_conversion_hint"
                                )),
                        )
                    })
                    .when_some(self.generator.preview_size, |panel, size| {
                        panel.child(crate::ui::components::inspector::result_summary(
                            colors,
                            t!("MapViewer.generator.render.result_label"),
                            t!(
                                "MapViewer.generator.render.result_size",
                                width = size[0],
                                height = size[1],
                                depth = size[2],
                                note = if self.generator.preview_displayed_options
                                    == Some(self.generator.options)
                                {
                                    String::new()
                                } else {
                                    t!(
                                        "MapViewer.generator.render.result_size_stale_note"
                                    )
                                    .to_string()
                                },
                            ),
                        ))
                    })
                    .child(
                        crate::ui::components::inspector::field_group(colors)
                            .child(self.render_import_height(colors, cx)),
                    )
                    .child(
                        crate::ui::components::inspector::field_group(colors)
                            .child(self.render_obj_parameters(colors, cx)),
                    ),
            )
            .into_any_element()
    }
}

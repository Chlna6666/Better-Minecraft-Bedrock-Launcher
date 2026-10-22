use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_obj_import_actions(
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
                    if self.generator.source.is_some() { "更换 OBJ" } else { "导入 OBJ" },
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
            .when(self.generator.preview_result_active && !busy, |bar| {
                bar.child(
                    crate::ui::components::button::primary_button(
                        colors, "obj-workspace-place", "放置到世界",
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
                    if busy { "转换中…" } else { "导出结构…" },
                )
                .px(px(12.0))
                .py(px(7.0))
                .opacity(if self.generator.source.is_some() && !busy { 1.0 } else { 0.45 })
                .when(self.generator.source.is_some() && !busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.export_generator_obj(cx)),
                    )
                }),
            )
    }

    pub(in super::super) fn render_generator_panel(
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
            .child(self.render_import_panel_header(colors, "模型转方块", cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(generator_section_title(colors, "01  素材"))
                    .child(crate::ui::components::inspector::inspector_source_card(
                        colors,
                        "OBJ / MTL",
                        self.generator.source.as_deref(),
                        "支持拖入 OBJ",
                        crate::ui::components::button::secondary_button(
                            colors, "obj-inspector-source", "选择模型…",
                        )
                        .when(!busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.choose_generator_obj(cx)),
                            )
                        })
                        .opacity(if busy { 0.45 } else { 1.0 }),
                    ))
                    .when(busy, |panel| panel.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("后台转换中 · 可以在任务面板取消"),
                    ))
                    .when_some(self.generator.preview_size, |panel, size| {
                        panel.child(crate::ui::components::inspector::inspector_result_summary(
                            colors, "方块化结果",
                            format!("{} × {} × {} 方块", size[0], size[1], size[2]),
                        ))
                    })
                    .child(crate::ui::components::inspector::inspector_field_group(colors)
                        .child(self.render_obj_parameters(colors, cx))),
            )
            .into_any_element()
    }
}

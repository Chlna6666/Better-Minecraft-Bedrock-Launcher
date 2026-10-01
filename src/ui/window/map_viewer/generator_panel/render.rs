use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_obj_import_actions(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.generator_busy();
        div()
            .flex()
            .flex_wrap()
            .gap(px(8.0))
            .when(self.generator.preview_result_active && !busy, |bar| {
                bar.child(
                    generator_choice(colors, "选择地图放置位置", true).on_mouse_down(
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
                generator_choice(
                    colors,
                    if busy {
                        "正在处理…"
                    } else {
                        "导出 .mcstructure…"
                    },
                    false,
                )
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
        let selected = self
            .generator
            .source
            .as_ref()
            .map(|source| source.display().to_string());

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
                    .child(generator_section_title(colors, "源模型"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(colors.text_secondary)
                            .child("读取 OBJ/MTL 与模型目录内的纹理；原存档只读。"),
                    )
                    .when(busy, |panel| {
                        panel.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_secondary)
                                .child("转换进行中，设置暂时锁定；可在任务页查看和取消。"),
                        )
                    })
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(selected.unwrap_or_else(|| "尚未选择模型".to_owned())),
                    )
                    .child(
                        generator_choice(colors, "选择 OBJ…", false).when(!busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.choose_generator_obj(cx)
                                }),
                            )
                        }),
                    )
                    .when_some(self.generator.preview_size, |panel, size| {
                        panel.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_primary)
                                .child(format!(
                                    "当前预览尺寸：{} × {} × {} 方块；瓦片与应用内 3D 面板同步",
                                    size[0], size[1], size[2]
                                )),
                        )
                    })
                    .child(self.render_obj_parameters(colors, cx)),
            )
            .into_any_element()
    }
}

use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_image_import_actions(
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
                .when(self.image_generator.source.is_some() && !busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.start_image_export(cx)),
                    )
                }),
            )
    }

    pub(in super::super) fn render_image_generator_panel(
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
            .child(self.render_import_panel_header(colors, "图片转方块", cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(15.0))
                    .child(generator_section_title(colors, "源图片"))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(self.image_generator.source.as_ref().map_or_else(
                                || "尚未选择图片".to_owned(),
                                |path| path.display().to_string(),
                            )),
                    )
                    .child(
                        generator_choice(colors, "选择图片…", false).when(!busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.choose_generator_image(cx)
                                }),
                            )
                        }),
                    )
                    .when(self.image_generator.preview_refresh_pending, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child("高度设置已更新，正在准备新的 3D 预览…"),
                        )
                    })
                    .when_some(self.image_generator.preview_size, |panel, size| {
                        panel.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(colors.text_primary)
                                .child(format!(
                                    "当前预览尺寸：{} × {} × {} 方块；瓦片与应用内 3D 预览同步",
                                    size[0], size[1], size[2]
                                )),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .child("支持 PNG、JPEG、TGA、WebP；源图只读。"),
                    )
                    .child(self.render_image_dimension(ImageAxis::Width, colors, cx))
                    .child(self.render_image_dimension(ImageAxis::Height, colors, cx))
                    .child(self.render_generator_y_control(colors, cx))
                    .child(self.render_image_dithering(colors, cx))
                    .child(self.render_image_depth_controls(colors, cx))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .child(format!(
                                "输出 {}×{} 方块；尺寸无需是 128 的倍数。",
                                self.image_generator.options.width,
                                self.image_generator.options.height
                            )),
                    )
                    .child(self.render_image_appearance(colors, cx)),
            )
            .into_any_element()
    }
}

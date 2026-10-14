use super::*;

impl MapViewerWindowView {
    pub(super) fn render_obj_parameters(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.generator_busy();
        div().flex().flex_col().gap(px(14.0))
                    .child(generator_section_title(colors, "目标尺寸"))
                    .child(self.render_generator_y_control(colors, cx))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(colors.text_secondary)
                            .child(
                                "最长边 1–384 格，其余两轴等比缩放；现代主世界支持 -64 到 319 高度，体积上限 800 万格。",
                            ),
                    )
                    .when(self.generator.preview_refresh_pending, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child("尺寸已更新，正在准备新的方块预览…"),
                        )
                    })
                    .child({
                        let changed_view = cx.entity().downgrade();
                        let committed_view = changed_view.clone();
                        let size = self.generator.options.longest_side_blocks;
                        let slider = Slider::new(
                            "obj-longest-side",
                            colors,
                            1.0,
                            384.0,
                            f32::from(size),
                            move |value, app| {
                                let next = value.round().clamp(1.0, 384.0) as u16;
                                if let Err(error) = changed_view.update(app, |this, cx| {
                                    if this.generator.options.longest_side_blocks != next {
                                        this.generator.options.longest_side_blocks = next;
                                        this.num_obj_size.sync(u32::from(next), cx);
                                        this.schedule_obj_live_preview(cx);
                                    }
                                }) {
                                    tracing::warn!(%error, "could not update OBJ size slider");
                                }
                            },
                        )
                        .width(px(245.0))
                        .on_commit(move |value, app| {
                            let next = value.round().clamp(1.0, 384.0) as u16;
                            if let Err(error) = committed_view.update(app, |this, cx| {
                                this.generator.options.longest_side_blocks = next;
                                this.num_obj_size.sync(u32::from(next), cx);
                                this.ensure_obj_live_preview(cx);
                                cx.notify();
                            }) {
                                tracing::warn!(%error, "could not apply OBJ size slider");
                            }
                        });

                        crate::ui::components::numeric_field::numeric_dimension_editor(
                            &self.num_obj_size,
                            colors,
                            "方块",
                            generator_choice(colors, "−", false).when(!busy && size > 1, |button| {
                                button.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_obj_precision_size(size - 1, cx)
                                    }),
                                )
                            }),
                            generator_choice(colors, "+", false).when(!busy && size < 384, |button| {
                                button.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_obj_precision_size(size + 1, cx)
                                    }),
                                )
                            }),
                            slider,
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child("常用尺寸"),
                            )
                            .child(div().flex_1())
                            .children(
                                [16_u16, 64, 128, 256].into_iter().map(|size| {
                                    generator_choice(
                                        colors,
                                        format!("{size}"),
                                        self.generator.options.longest_side_blocks == size,
                                    )
                                    .when(!busy, |button| {
                                        button.on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_obj_precision_size(size, cx)
                                            }),
                                        )
                                    })
                                }),
                            ),
                    )
                    .child(generator_section_title(colors, "填充方式"))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.0))
                            .child(
                                generator_choice(
                                    colors,
                                    "空心 · 仅表面",
                                    self.generator.options.fill == ObjFill::Surface,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.generator.options.fill = ObjFill::Surface;
                                            this.start_obj_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                }),
                            )
                            .child(
                                generator_choice(
                                    colors,
                                    "实心 · 填内部",
                                    self.generator.options.fill == ObjFill::Solid,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.generator.options.fill = ObjFill::Solid;
                                            this.start_obj_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(colors.text_secondary)
                            .child("实心模式填充封闭外壳；开放模型不会凭空补面。默认空心。"),
                    )
                    .child(generator_section_title(colors, "纹理透明度"))
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                (1_u8, "保留透明边缘"),
                                (128, "半透明以下忽略"),
                                (255, "仅不透明"),
                            ]
                            .into_iter()
                            .map(|(alpha, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.generator.options.alpha_threshold == alpha,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.generator.options.alpha_threshold = alpha;
                                            this.start_obj_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
                    .child(generator_section_title(colors, "半透明纹理底色"))
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                ([255_u8; 3], "白色"),
                                ([128_u8; 3], "灰色"),
                                ([0_u8; 3], "黑色"),
                            ]
                            .into_iter()
                            .map(|(background, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.generator.options.background == background,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.generator.options.background = background;
                                            this.start_obj_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
    }
}

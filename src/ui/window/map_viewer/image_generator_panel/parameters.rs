use super::*;

impl MapViewerWindowView {
    pub(super) fn render_image_appearance(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_generator_busy();
        div().flex().flex_col().gap(px(14.0))
                    .child(generator_section_title(colors, "图片比例"))
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .whitespace_normal()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child("居中裁剪会自动匹配当前 X×Z 尺寸比例；拖动宽、高滑条即可实时查看裁切与方块预览。"),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                (ImageFit::CenterCrop, "居中裁剪 · 不拉伸"),
                                (ImageFit::Stretch, "保留全图 · 可拉伸"),
                            ]
                            .into_iter()
                            .map(|(fit, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.image_generator.options.fit == fit,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.image_generator.options.fit = fit;
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
                    .child(generator_section_title(colors, "缩放采样"))
                    .child(
                        div().flex().gap(px(6.0)).children(
                            [
                                (MapResample::Nearest, "最近邻 · 像素画"),
                                (MapResample::Lanczos3, "Lanczos3 · 照片"),
                            ]
                            .into_iter()
                            .map(|(resample, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.image_generator.options.resample == resample,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.image_generator.options.resample = resample;
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
                    .child(generator_section_title(colors, "透明度"))
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                (1_u8, "保留透明边缘"),
                                (128, "忽略半透明"),
                                (255, "仅不透明"),
                            ]
                            .into_iter()
                            .map(|(alpha, label)| {
                                generator_choice(
                                    colors,
                                    label,
                                    self.image_generator.options.alpha_threshold == alpha,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.image_generator.options.alpha_threshold = alpha;
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
                    .child(generator_section_title(colors, "半透明底色"))
                    .child(
                        div().flex().gap(px(6.0)).children(
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
                                    self.image_generator.options.background == background,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            this.image_generator.options.background = background;
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                })
                            }),
                        ),
                    )
                    .child(generator_section_title(colors, "透明区域方块"))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(
                                generator_choice(
                                    colors,
                                    "留空",
                                    self.image_generator.options.transparent_fill.is_none(),
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.image_generator.options.transparent_fill = None;
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                }),
                            )
                            .child(
                                generator_choice(
                                    colors,
                                    "填充方块",
                                    self.image_generator.options.transparent_fill.is_some(),
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.image_generator
                                                .options
                                                .transparent_fill
                                                .get_or_insert_with(|| {
                                                    "minecraft:white_wool".to_owned()
                                                });
                                            this.start_image_live_preview(cx);
                                            cx.notify();
                                        }),
                                    )
                                }),
                            ),
                    )
                    .when_some(
                        self.image_generator.options.transparent_fill.as_ref(),
                        |panel, selected| {
                            panel.child(
                                generator_choice(
                                    colors,
                                    format!(
                                        "{} · 更换",
                                        selected.strip_prefix("minecraft:").unwrap_or(selected)
                                    ),
                                    false,
                                )
                                .when(!busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.image_generator.fill_picker_open =
                                                !this.image_generator.fill_picker_open;
                                            cx.notify();
                                        }),
                                    )
                                }),
                            )
                        },
                    )
                    .when(
                        self.image_generator.fill_picker_open
                            && self.image_generator.options.transparent_fill.is_some(),
                        |panel| {
                            panel.child(
                                div().flex().flex_wrap().gap(px(6.0)).children(
                                    self.image_generator.fill_candidates.iter().cloned().map(
                                        |name| {
                                            let selected = self
                                                .image_generator
                                                .options
                                                .transparent_fill
                                                .as_deref()
                                                == Some(name.as_str());
                                            generator_choice(
                                                colors,
                                                name.strip_prefix("minecraft:")
                                                    .unwrap_or(&name)
                                                    .to_owned(),
                                                selected,
                                            )
                                            .when(
                                                !busy,
                                                |button| {
                                                    button.on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(
                                                            move |this, _event, _window, cx| {
                                                                this.image_generator
                                                                    .options
                                                                    .transparent_fill =
                                                                    Some(name.clone());
                                                                this.image_generator
                                                                    .fill_picker_open = false;
                                                                this.start_image_live_preview(cx);
                                                                cx.notify();
                                                            },
                                                        ),
                                                    )
                                                },
                                            )
                                        },
                                    ),
                                ),
                            )
                        },
                    )
    }
}

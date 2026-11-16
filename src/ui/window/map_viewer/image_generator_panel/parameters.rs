use super::*;

impl View {
    pub(super) fn render_image_appearance(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_generator_busy();
        div().flex().flex_col().gap(px(14.0))
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.generator.image_generator.parameters.section_image_ratio"),
                    ))
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .whitespace_normal()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(t!(
                                "MapViewer.generator.image_generator.parameters.image_ratio_hint"
                            )),
                    )
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                (
                                    ImageFit::CenterCrop,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.fit_center_crop"
                                    ),
                                ),
                                (
                                    ImageFit::Stretch,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.fit_stretch"
                                    ),
                                ),
                            ]
                            .into_iter()
                            .map(|(fit, label)| {
                                choice::chip(
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
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.generator.image_generator.parameters.section_resample"),
                    ))
                    .child(
                        div().flex().gap(px(6.0)).children(
                            [
                                (
                                    MapResample::Nearest,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.resample_nearest"
                                    ),
                                ),
                                (
                                    MapResample::Lanczos3,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.resample_lanczos3"
                                    ),
                                ),
                            ]
                            .into_iter()
                            .map(|(resample, label)| {
                                choice::chip(
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
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.generator.image_generator.parameters.section_alpha"),
                    ))
                    .child(
                        div().flex().flex_wrap().gap(px(6.0)).children(
                            [
                                (
                                    1_u8,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.alpha_keep_edges"
                                    ),
                                ),
                                (
                                    128,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.alpha_ignore_translucent"
                                    ),
                                ),
                                (
                                    255,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.alpha_opaque_only"
                                    ),
                                ),
                            ]
                            .into_iter()
                            .map(|(alpha, label)| {
                                choice::chip(
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
                    .child(inspector::section_title(
                        colors,
                        t!(
                            "MapViewer.generator.image_generator.parameters.section_alpha_background"
                        ),
                    ))
                    .child(
                        div().flex().gap(px(6.0)).children(
                            [
                                (
                                    [255_u8; 3],
                                    t!("MapViewer.generator.image_generator.parameters.color_white"),
                                ),
                                (
                                    [128_u8; 3],
                                    t!("MapViewer.generator.image_generator.parameters.color_gray"),
                                ),
                                (
                                    [0_u8; 3],
                                    t!("MapViewer.generator.image_generator.parameters.color_black"),
                                ),
                            ]
                            .into_iter()
                            .map(|(background, label)| {
                                choice::chip(
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
                    .child(inspector::section_title(
                        colors,
                        t!(
                            "MapViewer.generator.image_generator.parameters.section_transparent_fill"
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.0))
                            .child(
                                choice::chip(
                                    colors,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.transparent_leave_empty"
                                    ),
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
                                choice::chip(
                                    colors,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.transparent_fill_block"
                                    ),
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
                                choice::chip(
                                    colors,
                                    t!(
                                        "MapViewer.generator.image_generator.parameters.selected_block_change",
                                        name = selected
                                            .strip_prefix("minecraft:")
                                            .unwrap_or(selected),
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
                                            choice::chip(
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

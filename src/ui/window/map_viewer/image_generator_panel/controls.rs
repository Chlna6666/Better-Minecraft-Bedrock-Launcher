use super::*;

impl MapViewerWindowView {
    pub(super) fn render_image_dithering(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_generator_busy();
        let selected = self.image_generator.options.dithering;
        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(generator_section_title(colors, "颜色匹配"))
            .child(div().flex().flex_wrap().gap(px(6.0)).children(
                [
                    (Dithering::None, "最近色（无抖动）"),
                    (Dithering::FloydSteinberg, "Floyd–Steinberg"),
                ]
                .into_iter()
                .map(|(dithering, label)| {
                    generator_choice(colors, label, selected == dithering).when(!busy, |button| {
                        button.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.set_image_dithering(dithering, cx)
                            }),
                        )
                    })
                }),
            ))
            .child(
                div()
                    .whitespace_normal()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(colors.text_secondary)
                    .child("方块材质的颜色有限。最近色保留清晰边界；误差扩散适合插画与柔和渐变，远看更接近原图，近看会有颗粒。"),
            )
    }

    pub(super) fn render_image_depth_controls(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_generator_busy();
        let depth = self.image_generator.options.depth;
        let max_allowed = self.image_generator.options.max_relief_height();
        let relief = match depth {
            ImageBlockHeight::Flat => None,
            ImageBlockHeight::LuminanceRelief { max_height, fill } => {
                Some((max_height.min(max_allowed), fill))
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(generator_section_title(colors, "高度模式"))
            .child(self.render_image_height_modes(colors, cx, busy, depth, relief))
            .when_some(relief, |panel, (max_height, fill)| {
                panel.child(self.render_relief_image_settings(
                    colors,
                    cx,
                    busy,
                    max_allowed,
                    max_height,
                    fill,
                ))
            })
    }

    pub(super) fn render_image_height_modes(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
        busy: bool,
        depth: ImageBlockHeight,
        relief: Option<(u16, ReliefImageFill)>,
    ) -> Div {
        div().flex().flex_wrap().gap(px(6.0)).children(
            [
                (ImageBlockHeight::Flat, "Flat 平面"),
                (
                    ImageBlockHeight::LuminanceRelief {
                        max_height: relief.map_or(16, |(height, _)| height),
                        fill: relief.map_or(ReliefImageFill::Solid, |(_, fill)| fill),
                    },
                    "2.5D 亮度阶梯",
                ),
            ]
            .into_iter()
            .map(|(mode, label)| {
                let selected = matches!(
                    (depth, mode),
                    (ImageBlockHeight::Flat, ImageBlockHeight::Flat)
                        | (
                            ImageBlockHeight::LuminanceRelief { .. },
                            ImageBlockHeight::LuminanceRelief { .. }
                        )
                );
                generator_choice(colors, label, selected).when(!busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_image_depth(mode, cx)
                        }),
                    )
                })
            }),
        )
    }

    pub(super) fn render_relief_image_settings(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
        busy: bool,
        max_allowed: u16,
        max_height: u16,
        fill: ReliefImageFill,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(self.render_relief_height_slider(
                colors,
                cx,
                busy,
                max_allowed,
                max_height,
                fill,
            ))
            .child(self.render_relief_fill_choices(colors, cx, busy, max_height, fill))
            .child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(colors.text_secondary)
                    .child("按图像亮度生成高度。真实地图样本支持“南侧更高时像素更亮”的方向关系；地图色表和阴影强度尚未逐色拟合。"),
            )
    }

    pub(super) fn render_relief_height_slider(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
        busy: bool,
        max_allowed: u16,
        max_height: u16,
        fill: ReliefImageFill,
    ) -> Div {
        let changed_view = cx.entity().downgrade();
        let committed_view = changed_view.clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child(format!("阶梯高度 · {max_height} 层")),
            )
            .when(!busy, |row| {
                row.child(
                    Slider::new(
                        "image-relief-height",
                        colors,
                        2.0,
                        f32::from(max_allowed),
                        f32::from(max_height),
                        move |value, app| {
                            let next = value.round().clamp(2.0, f32::from(max_allowed)) as u16;
                            if let Err(error) = changed_view.update(app, |this, cx| {
                                this.update_image_relief_height(next, fill, cx);
                            }) {
                                tracing::warn!(%error, "could not update image relief height");
                            }
                        },
                    )
                    .width(px(170.0))
                    .on_commit(move |value, app| {
                        let next = value.round().clamp(2.0, f32::from(max_allowed)) as u16;
                        if let Err(error) = committed_view.update(app, |this, cx| {
                            this.update_image_relief_height(next, fill, cx);
                        }) {
                            tracing::warn!(%error, "could not commit image relief height");
                        }
                    }),
                )
            })
    }

    pub(super) fn render_relief_fill_choices(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
        busy: bool,
        max_height: u16,
        fill: ReliefImageFill,
    ) -> Div {
        div().flex().flex_wrap().gap(px(6.0)).children(
            [
                (ReliefImageFill::SurfaceOnly, "仅表面"),
                (ReliefImageFill::Solid, "实心阶梯"),
            ]
            .into_iter()
            .map(|(next_fill, label)| {
                generator_choice(colors, label, fill == next_fill).when(!busy, |button| {
                    button.on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_image_depth(
                                ImageBlockHeight::LuminanceRelief {
                                    max_height,
                                    fill: next_fill,
                                },
                                cx,
                            );
                        }),
                    )
                })
            }),
        )
    }

    pub(super) fn render_image_dimension(
        &self,
        axis: ImageAxis,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let value = match axis {
            ImageAxis::Width => self.image_generator.options.width,
            ImageAxis::Height => self.image_generator.options.height,
        };
        let busy = self.image_generator_busy();
        let slider_id = match axis {
            ImageAxis::Width => "image-block-width",
            ImageAxis::Height => "image-block-height",
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(generator_section_title(
                colors,
                match axis {
                    ImageAxis::Width => "宽度 · X",
                    ImageAxis::Height => "高度 · Z",
                },
            ))
            .child(div().flex().flex_wrap().gap(px(6.0)).children(
                [64_u32, 128, 256, 512].into_iter().map(|size| {
                    generator_choice(colors, format!("{size}"), value == size).when(
                        !busy,
                        |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_image_dimension(axis, size, cx)
                                }),
                            )
                        },
                    )
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(generator_choice(colors, "−1", false).when(!busy, |button| {
                        button.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.set_image_dimension(axis, value.saturating_sub(1), cx)
                            }),
                        )
                    }))
                    .when(!busy, |row| row.child(
                        Slider::new(
                            slider_id,
                            colors,
                            1.0,
                            512.0,
                            value as f32,
                            {
                                let changed_view = cx.entity().downgrade();
                                move |value, app| {
                                    let next = value.round().clamp(1.0, 512.0) as u32;
                                    if let Err(error) = changed_view.update(app, |this, cx| {
                                        this.update_image_dimension_from_slider(axis, next, cx);
                                        cx.notify();
                                    }) {
                                        tracing::warn!(%error, "could not update image dimension slider");
                                    }
                                }
                            },
                        )
                        .width(px(180.0))
                        .on_commit({
                            let committed_view = cx.entity().downgrade();
                            move |value, app| {
                                let next = value.round().clamp(1.0, 512.0) as u32;
                                if let Err(error) = committed_view.update(app, |this, cx| {
                                    this.set_image_dimension(axis, next, cx);
                                }) {
                                    tracing::warn!(%error, "could not apply image dimension slider");
                                }
                            }
                        })
                    ))
                    .child(
                        div()
                            .min_w(px(72.0))
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(format!("{value} 方块")),
                    )
                    .child(generator_choice(colors, "+1", false).when(!busy, |button| {
                        button.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.set_image_dimension(axis, value.saturating_add(1), cx)
                            }),
                        )
                    })),
            )
    }
}

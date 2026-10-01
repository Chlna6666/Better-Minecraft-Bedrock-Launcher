use super::*;

impl MapViewerWindowView {
    pub(super) fn render_grid_axis(
        &self,
        axis: GridAxis,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let count = match axis {
            GridAxis::Columns => self.map_image.options.columns,
            GridAxis::Rows => self.map_image.options.rows,
        };
        let other = match axis {
            GridAxis::Columns => self.map_image.options.rows,
            GridAxis::Rows => self.map_image.options.columns,
        };
        let max_count = (1024 / other.max(1)).min(64);
        let count_label = if self.map_image.options.auto_fit
            && !self.map_image.grid_estimated
            && self.map_image.preview_done == 0
        {
            "自动".to_owned()
        } else {
            format!("{count} 张")
        };
        let slider_id = match axis {
            GridAxis::Columns => "map-grid-columns",
            GridAxis::Rows => "map-grid-rows",
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(generator_section_title(
                colors,
                match axis {
                    GridAxis::Columns => "列 · 从西向东",
                    GridAxis::Rows => "行 · 从北向南",
                },
            ))
            .child(
                div().flex().flex_wrap().gap(px(6.0)).children(
                    [1_u32, 2, 4, 8, 16, 32, 64]
                        .into_iter()
                        .filter(|size| *size <= max_count)
                        .map(|size| {
                            generator_choice(colors, format!("{size}"), count == size)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        this.set_grid_size(axis, size, cx)
                                    }),
                                )
                        }),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        generator_choice(colors, "−1", false).when(count > 1, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_grid_size(axis, count - 1, cx)
                                }),
                            )
                        }),
                    )
                    .child({
                        let changed_view = cx.entity().downgrade();
                        let committed_view = changed_view.clone();
                        Slider::new(
                            slider_id,
                            colors,
                            1.0,
                            64.0,
                            count as f32,
                            move |value, app| {
                                let next = value.round().clamp(1.0, 64.0) as u32;
                                if let Err(error) = changed_view.update(app, |this, cx| {
                                    this.set_grid_size_from_slider(axis, next, cx);
                                }) {
                                    tracing::warn!(%error, "could not update map grid slider");
                                }
                            },
                        )
                        .width(px(180.0))
                        .on_commit(move |value, app| {
                            let next = value.round().clamp(1.0, 64.0) as u32;
                            if let Err(error) = committed_view.update(app, |this, cx| {
                                this.set_grid_size(axis, next, cx);
                            }) {
                                tracing::warn!(%error, "could not apply map grid slider");
                            }
                        })
                    })
                    .child(
                        div()
                            .min_w(px(48.0))
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(count_label),
                    )
                    .child(generator_choice(colors, "+1", false).when(
                        count < max_count,
                        |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.set_grid_size(axis, count + 1, cx)
                                }),
                            )
                        },
                    )),
            )
    }
}

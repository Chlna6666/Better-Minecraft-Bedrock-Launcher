use super::*;
use crate::ui::components::numeric_field::numeric_dimension_editor;

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
        let busy = self.map_image_operation_busy();
        let (slider_id, label, field) = match axis {
            GridAxis::Columns => ("map-grid-columns", "列 · 从西向东", &self.num_map_columns),
            GridAxis::Rows => ("map-grid-rows", "行 · 从北向南", &self.num_map_rows),
        };
        let changed_view = cx.entity().downgrade();
        let committed_view = changed_view.clone();
        let slider = Slider::new(
            slider_id,
            colors,
            1.0,
            max_count as f32,
            count as f32,
            move |value, app| {
                let next = value.round().clamp(1.0, max_count as f32) as u32;
                if let Err(error) = changed_view.update(app, |this, cx| {
                    this.set_grid_size_from_slider(axis, next, cx);
                }) {
                    tracing::warn!(%error, "could not update map grid slider");
                }
            },
        )
        .width(px(245.0))
        .on_commit(move |value, app| {
            let next = value.round().clamp(1.0, max_count as f32) as u32;
            if let Err(error) = committed_view.update(app, |this, cx| {
                this.set_grid_size(axis, next, cx);
            }) {
                tracing::warn!(%error, "could not apply map grid slider");
            }
        });
        let decrease = crate::ui::components::numeric_field::numeric_step_button(
            colors, "−", !busy && count > 1,
        ).when(!busy && count > 1, |button| {
            button.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| this.set_grid_size(axis, count - 1, cx)),
            )
        });
        let increase = crate::ui::components::numeric_field::numeric_step_button(
            colors, "+", !busy && count < max_count,
        ).when(
            !busy && count < max_count,
            |button| {
                button.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.set_grid_size(axis, count + 1, cx)),
                )
            },
        );

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(generator_section_title(colors, label))
            .child(numeric_dimension_editor(
                field,
                colors,
                "张",
                decrease,
                increase,
                slider,
            ))
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
                            .child(if self.map_image.options.auto_fit {
                                if !self.map_image.grid_estimated && self.map_image.preview_done == 0 {
                                    "自动匹配中".to_owned()
                                } else {
                                    format!("自动匹配 · {count} 张")
                                }
                            } else {
                                format!("允许范围 1–{max_count}")
                            }),
                    )
                    .child(div().flex_1())
                    .children(
                        [1_u32, 4, 16, 64]
                            .into_iter()
                            .filter(|size| *size <= max_count)
                            .map(|size| {
                                generator_choice(colors, format!("{size}"), count == size)
                                    .when(!busy, |button| {
                                        button.on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_grid_size(axis, size, cx)
                                            }),
                                        )
                                    })
                            }),
                    ),
            )
    }
}

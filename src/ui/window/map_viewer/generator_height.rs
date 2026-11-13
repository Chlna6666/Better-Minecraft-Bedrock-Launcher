use super::model::View;
use super::prelude::*;
use crate::ui::components::input::InputSize;
use crate::ui::components::slider::Slider;

impl View {
    pub(super) fn import_height(&self) -> i32 {
        if matches!(
            self.ui_state.active_right_panel,
            MapViewerRightPanel::Generator | MapViewerRightPanel::ImageGenerator
        ) {
            self.professional
                .imported_structure
                .as_ref()
                .map_or(self.y_layer, |import| import.origin_y)
        } else {
            self.y_layer
        }
    }

    fn import_height_range(&self) -> Option<(i32, i32)> {
        let (min, max) = ChunkPos {
            x: 0,
            z: 0,
            dimension: self.dimension,
        }
        .y_range(ChunkVersion::New);
        let height = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.preview_size,
            MapViewerRightPanel::ImageGenerator => self.image_generator.preview_size,
            _ => None,
        }
        .map_or(1, |size| size[1]);
        let support = self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
            && self.map_image.install_destination
                == super::map_image_panel::MapImageInstallDestination::NewUpFrame;
        height_range(min, max, height, support)
    }

    pub(super) fn set_import_height(
        &mut self,
        value: i32,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.map_write_busy()
            || (self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
                && self.map_image_operation_busy())
        {
            return Err("正在写入或导出，暂不能调整高度".to_owned());
        }
        let (min, max) = self
            .import_height_range()
            .ok_or("结构高度超过当前维度范围")?;
        if !(min..=max).contains(&value) {
            return Err(format!("放置高度须在 {min}..{max} 内"));
        }
        if self.y_layer == value && self.import_height() == value {
            return Ok(());
        }
        let structure = matches!(
            self.ui_state.active_right_panel,
            MapViewerRightPanel::Generator | MapViewerRightPanel::ImageGenerator
        );
        if structure
            && self.professional.imported_structure.is_some()
            && self.professional.paste_preview.is_some()
        {
            self.set_2d_paste_height(value, cx)?;
        } else {
            self.step_y(value.saturating_sub(self.y_layer), cx);
        }
        if structure {
            if let Some(import) = self.professional.imported_structure.as_mut() {
                import.origin_y = value;
            }
        }
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
        Ok(())
    }

    pub(super) fn render_import_height(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let range = self.import_height_range();
        let current = self.import_height();
        let (min, max) = range.unwrap_or((current, current));
        let enabled = range.is_some()
            && !self.map_write_busy()
            && !(self.ui_state.active_right_panel == MapViewerRightPanel::MapImage
                && self.map_image_operation_busy());
        let frame = self.ui_state.active_right_panel == MapViewerRightPanel::MapImage;
        let owner = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(colors.text_primary)
                            .child(if frame {
                                "展示框高度 · Y"
                            } else {
                                "放置底部 · Y"
                            }),
                    )
                    .child(if enabled {
                        Input::new(&self.input_fields.paste_origin_y)
                            .with_size(InputSize::Small)
                            .w(px(80.0))
                            .into_any_element()
                    } else {
                        div()
                            .w(px(80.0))
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .child(current.to_string())
                            .into_any_element()
                    }),
            )
            .child(
                div().opacity(if enabled { 1.0 } else { 0.45 }).child(
                    Slider::new(
                        "import-placement-height",
                        colors,
                        min as f32,
                        max as f32,
                        current as f32,
                        move |value, app| {
                            if enabled {
                                let _ = owner.update(app, |this, cx| {
                                    if let Err(error) =
                                        this.set_import_height(value.round() as i32, cx)
                                    {
                                        this.status = SharedString::from(error);
                                    } else {
                                        let text = this.import_height().to_string();
                                        this.input_fields
                                            .paste_origin_y
                                            .update(cx, |input, cx| input.set_text(text, cx));
                                        this.input_fields
                                            .dirty_fields
                                            .remove(&super::model::InputField::PasteOriginY);
                                    }
                                    cx.notify();
                                });
                            }
                        },
                    )
                    .width(px((self.ui_state.right_panel_width - 80.0).max(120.0))),
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(10.0))
                    .text_color(colors.text_secondary)
                    .child(format!("{min}"))
                    .child(if frame {
                        format!("支撑层 Y {}", self.y_layer.saturating_sub(1))
                    } else {
                        "可输入精确数值".to_owned()
                    })
                    .child(format!("{max}")),
            )
    }
}

fn height_range(min: i32, max: i32, height: i32, support: bool) -> Option<(i32, i32)> {
    let min = min.saturating_add(i32::from(support));
    let max = max.saturating_sub(height.max(1)).saturating_add(1);
    (min <= max).then_some((min, max))
}

#[cfg(test)]
mod tests {
    use super::height_range;
    #[test]
    fn bounds_include_structure_top_and_frame_support() {
        assert_eq!(height_range(-64, 319, 128, false), Some((-64, 192)));
        assert_eq!(height_range(-64, 319, 1, true), Some((-63, 319)));
        assert_eq!(height_range(0, 127, 129, false), None);
    }
}

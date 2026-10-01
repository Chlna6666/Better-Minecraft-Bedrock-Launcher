use super::generator_panel::generator_choice;
use super::model::MapViewerWindowView;
use super::prelude::*;

impl MapViewerWindowView {
    fn step_generator_y(&mut self, delta: i32, cx: &mut Context<Self>) {
        let preview_size = match self.ui_state.active_right_panel {
            MapViewerRightPanel::Generator => self.generator.preview_size,
            MapViewerRightPanel::ImageGenerator => self.image_generator.preview_size,
            _ => None,
        };
        let Some(size) = preview_size else {
            self.step_y(delta, cx);
            return;
        };
        let (min_y, max_y) = ChunkPos {
            x: 0,
            z: 0,
            dimension: self.dimension,
        }
        .y_range(ChunkVersion::New);
        let max_origin = max_y.saturating_sub(size[1]).saturating_add(1);
        if max_origin < min_y {
            self.status = SharedString::from("结构高度超过当前维度范围");
            cx.notify();
            return;
        }
        let target = self.y_layer.saturating_add(delta).clamp(min_y, max_origin);
        self.step_y(target - self.y_layer, cx);
        if let Some(import) = self.professional.imported_structure.as_mut() {
            import.origin_y = self.y_layer;
        }
        if let Some(preview) = self.professional.paste_preview.clone() {
            self.set_paste_preview(
                preview.target_anchor,
                preview.transform,
                preview.display_degrees,
                None,
                cx,
            );
        }
    }

    pub(super) fn render_generator_y_control(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(generator_choice(colors, "Y −1", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.step_generator_y(-1, cx)),
            ))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.text_primary)
                    .child(format!("预览/放置底部 Y {}", self.y_layer)),
            )
            .child(generator_choice(colors, "Y +1", false).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| this.step_generator_y(1, cx)),
            ))
    }
}

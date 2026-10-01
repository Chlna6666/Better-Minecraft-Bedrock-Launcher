use super::super::import_workspace::import_preview_label;
use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_image_block_comparison(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let aspect =
            self.image_generator.options.width as f32 / self.image_generator.options.height as f32;
        let source_width = (self.viewport.width * 0.5 - 24.0)
            .max(1.0)
            .min((self.viewport.height - 180.0).max(1.0) * aspect);
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(import_preview_label(colors, "源图片 · 当前裁剪比例"))
                    .when_some(self.image_generator.source.as_ref(), |pane, source| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .overflow_hidden()
                                .child(
                                    img(source.clone())
                                        .w(px(source_width))
                                        .h(px(source_width / aspect))
                                        .object_fit(
                                            if self.image_generator.options.fit
                                                == ImageFit::CenterCrop
                                            {
                                                ObjectFit::Cover
                                            } else {
                                                ObjectFit::Fill
                                            },
                                        ),
                                ),
                        )
                    }),
            )
            .child(self.render_import_block_canvas(colors, cx))
    }
}

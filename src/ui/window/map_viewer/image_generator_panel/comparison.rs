use super::super::import_workspace::import_preview_label;
use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_image_block_comparison(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let compact = self.viewport.width < 720.0;
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .when(compact, |layout| layout.flex_col())
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
                                        .w_full()
                                        .h_full()
                                        .object_fit(
                                            if self.image_generator.options.fit
                                                == ImageFit::CenterCrop
                                            {
                                                ObjectFit::Cover
                                            } else {
                                                ObjectFit::Contain
                                            },
                                        ),
                                ),
                        )
                    }),
            )
            .child(self.render_import_block_canvas(colors, cx))
    }
}

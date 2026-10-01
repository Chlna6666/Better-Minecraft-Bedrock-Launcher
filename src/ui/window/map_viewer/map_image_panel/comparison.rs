use super::super::import_workspace::import_preview_label;
use super::*;

impl MapViewerWindowView {
    pub(in super::super) fn render_map_image_comparison(&self, colors: &ThemeColors) -> Div {
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
                    .child(import_preview_label(colors, "源图片"))
                    .when_some(self.map_image.source.as_ref(), |pane, source| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .child(
                                    img(source.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(import_preview_label(
                        colors,
                        format!(
                            "地图像素 · {} 列 × {} 行",
                            self.map_image.options.columns, self.map_image.options.rows
                        ),
                    ))
                    .when_some(self.map_image.preview.as_ref(), |pane, preview| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .child(
                                    img(preview.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain),
                                ),
                        )
                    })
                    .when_some(self.map_image.preview_note.as_ref(), |pane, note| {
                        pane.child(import_preview_label(colors, note.clone()))
                    }),
            )
    }
}

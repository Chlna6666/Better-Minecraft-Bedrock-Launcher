use super::super::import_workspace::import_preview_label;
use super::*;

impl View {
    /// Inventory delivery is not a world placement. Show the actual map mosaic and
    /// selected player here instead of opening an unrelated terrain canvas.
    pub(in super::super) fn render_map_inventory_workspace(&self, colors: &ThemeColors) -> Div {
        let player = super::super::players::player_id_label(&self.map_image.selected_player);
        let inventory_image = if self.map_image.pending_bundle.is_some() {
            None
        } else {
            self.map_image.preview.as_ref()
        };
        let columns = self.map_image.options.columns;
        let rows = self.map_image.options.rows;
        div()
            .size_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(colors.text_primary)
                    .child(t!("MapViewer.imagegen.comparison.inventory_preview_title")),
            )
            .child(import_preview_label(
                colors,
                if self.map_image.pending_bundle.is_some() {
                    t!(
                        "MapViewer.imagegen.comparison.inventory_target_pending",
                        player = &player,
                    )
                } else {
                    t!(
                        "MapViewer.imagegen.comparison.inventory_target_summary",
                        player = &player,
                        columns = columns,
                        rows = rows,
                        count = columns * rows,
                    )
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(colors.surface)
                    .overflow_hidden()
                    .when_some(inventory_image, |pane, preview| {
                        pane.child(
                            img(preview.clone())
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        )
                    })
                    .when(inventory_image.is_none(), |pane| {
                        pane.child(import_preview_label(
                            colors,
                            if self.map_image.pending_bundle.is_some() {
                                t!("MapViewer.imagegen.comparison.inventory_no_preview_pending")
                            } else {
                                t!("MapViewer.imagegen.comparison.inventory_no_preview")
                            },
                        ))
                    }),
            )
            .child(import_preview_label(
                colors,
                t!("MapViewer.imagegen.comparison.inventory_hint"),
            ))
    }

    /// Non-destructive placement inspector sharing the generated map image.
    pub(in super::super) fn render_map_placement_thumbnail(&self, colors: &ThemeColors) -> Div {
        let placement_image = if self.map_image.pending_bundle.is_some() {
            None
        } else {
            self.map_image.preview.as_ref()
        };
        let target = match self.map_image.install_destination {
            MapImageInstallDestination::Player => {
                t!("MapViewer.imagegen.comparison.target_inventory").to_string()
            }
            MapImageInstallDestination::SelectedBlock => self.map_image.selected_block.map_or_else(
                || t!("MapViewer.imagegen.comparison.target_container_missing").to_string(),
                |(_, block)| {
                    t!(
                        "MapViewer.imagegen.comparison.target_coords",
                        x = block.x,
                        y = block.y,
                        z = block.z,
                    )
                    .to_string()
                },
            ),
            MapImageInstallDestination::NewUpFrame => {
                if let Some((x, z)) = self.map_frame_confirmed_center() {
                    if self.map_image.pending_bundle.is_some() {
                        t!(
                            "MapViewer.imagegen.comparison.frame_center_pending",
                            x = x,
                            y = self.y_layer,
                            z = z,
                        )
                        .to_string()
                    } else {
                        t!(
                            "MapViewer.imagegen.comparison.frame_center_maps",
                            x = x,
                            y = self.y_layer,
                            z = z,
                            columns = self.map_image.options.columns,
                            rows = self.map_image.options.rows,
                        )
                        .to_string()
                    }
                } else {
                    t!("MapViewer.imagegen.comparison.frame_center_missing").to_string()
                }
            }
        };
        // This is an optional inspector column, not an overlay over world tiles.
        div()
            .flex_none()
            .h_full()
            .w(px(254.0))
            .p(px(10.0))
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .flex()
            .flex_col()
            .gap(px(6.0))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .child(import_preview_label(
                colors,
                t!("MapViewer.imagegen.comparison.placement_thumbnail_title"),
            ))
            .when_some(placement_image, |panel, preview| {
                panel.child(
                    div().w_full().h(px(152.0)).overflow_hidden().child(
                        img(preview.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                )
            })
            .when(placement_image.is_none(), |panel| {
                panel.child(import_preview_label(
                    colors,
                    if self.map_image.pending_bundle.is_some() {
                        t!("MapViewer.imagegen.comparison.thumbnail_no_preview_pending")
                    } else {
                        t!("MapViewer.imagegen.comparison.thumbnail_no_preview")
                    },
                ))
            })
            .child(import_preview_label(colors, target))
            .child(import_preview_label(
                colors,
                match self.map_image.install_destination {
                    MapImageInstallDestination::SelectedBlock => {
                        t!("MapViewer.imagegen.comparison.placement_highlight_hint")
                    }
                    MapImageInstallDestination::NewUpFrame => {
                        t!("MapViewer.imagegen.comparison.placement_frame_hint")
                    }
                    MapImageInstallDestination::Player => {
                        t!("MapViewer.imagegen.comparison.placement_inventory_hint")
                    }
                },
            ))
    }

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
                    .child(import_preview_label(
                        colors,
                        t!("MapViewer.imagegen.comparison.source_image_label"),
                    ))
                    .when_some(self.map_image.source.as_ref(), |pane, source| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .flex()
                                .items_start()
                                .justify_center()
                                .child(
                                    img(source.clone())
                                        .w_full()
                                        .h_full()
                                        .max_h(px(440.0))
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
                        t!(
                            "MapViewer.imagegen.comparison.pixel_grid_label",
                            columns = self.map_image.options.columns,
                            rows = self.map_image.options.rows,
                        ),
                    ))
                    .when_some(self.map_image.preview.as_ref(), |pane, preview| {
                        pane.child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .p(px(12.0))
                                .overflow_hidden()
                                .flex()
                                .items_start()
                                .justify_center()
                                .child(
                                    img(preview.clone())
                                        .w_full()
                                        .h_full()
                                        .max_h(px(440.0))
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

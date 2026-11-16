use super::*;

impl View {
    pub(in super::super) fn map_image_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let operation_busy = self.map_image_operation_busy();
        let can_export = self.map_image.preview.is_some()
            && self.map_image.task_id.as_ref().is_some_and(|id| {
                self.task_snapshots
                    .get(id.as_str())
                    .is_some_and(|snapshot| snapshot.status.as_ref() == "completed")
            });
        let map_count = self.map_image.options.columns * self.map_image.options.rows;
        let grid_resolved = !self.map_image.options.auto_fit
            || self.map_image.grid_estimated
            || self.map_image.preview_done > 0
            || can_export;
        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .child(self.render_import_panel_header(
                colors,
                t!("MapViewer.imagegen.render.title"),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scrollbar()
                    .p(px(14.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_source"),
                    ))
                    .child(crate::ui::components::inspector::source_card(
                        colors,
                        if self.map_image.pending_bundle.is_some() {
                            "NBT / MAP"
                        } else {
                            "MAP / 128"
                        },
                        self.map_image.source.as_deref()
                            .or(self.map_image.pending_bundle.as_deref()),
                        if self.map_image.pending_bundle.is_some() {
                            t!("MapViewer.imagegen.render.source_hint_staged_nbt")
                        } else {
                            t!("MapViewer.imagegen.render.source_hint_formats")
                        },
                        crate::ui::components::button::secondary_button(
                            colors,
                            "map-image-source",
                            t!("MapViewer.imagegen.render.choose_image"),
                        )
                        .when(!operation_busy, |button| {
                            button.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.choose_map_source(cx)),
                            )
                        })
                        .opacity(if operation_busy { 0.45 } else { 1.0 }),
                    ))
                    .when(operation_busy, |panel| panel.child(
                        div().text_size(px(11.0)).text_color(colors.text_secondary)
                            .child(t!("MapViewer.imagegen.render.operation_busy_hint")),
                    ))
                    .child(crate::ui::components::inspector::result_summary(
                        colors,
                        t!("MapViewer.imagegen.render.map_grid_summary_label"),
                        if grid_resolved {
                            t!(
                                "MapViewer.imagegen.render.map_grid_summary",
                                columns = self.map_image.options.columns,
                                rows = self.map_image.options.rows,
                                count = map_count,
                                width = self.map_image.options.columns * 128,
                                height = self.map_image.options.rows * 128,
                            )
                        } else {
                            t!("MapViewer.imagegen.render.calculating_tiles")
                        },
                    ))
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_target"),
                    ))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(t!("MapViewer.imagegen.render.target_hint")),
                    )
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            (
                                MapImageInstallDestination::NewUpFrame,
                                t!("MapViewer.imagegen.render.destination_frame"),
                            ),
                            (
                                MapImageInstallDestination::Player,
                                t!("MapViewer.imagegen.render.destination_player"),
                            ),
                            (
                                MapImageInstallDestination::SelectedBlock,
                                t!("MapViewer.imagegen.render.destination_container"),
                            ),
                        ].into_iter().map(|(destination, label)| {
                            choice::chip(colors, label, self.map_image.install_destination == destination)
                                .when(!operation_busy, |button| button.on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| this.set_map_install_destination(destination, cx)),
                                ))
                        })
                    ))
                    .when(
                        self.map_image.install_destination == MapImageInstallDestination::Player,
                        |panel| {
                            panel.child(self.render_map_player_picker(colors, cx))
                        },
                    )
                    .when(
                        self.map_image.install_destination == MapImageInstallDestination::SelectedBlock,
                        |panel| {
                            panel
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(colors.text_primary)
                                        .child(self.map_image.selected_block.map_or_else(
                                            || {
                                                t!(
                                                    "MapViewer.imagegen.render.no_container_selected"
                                                )
                                                .to_string()
                                            },
                                            |(chunk, block)| {
                                                if chunk.dimension != self.dimension {
                                                    t!(
                                                        "MapViewer.imagegen.render.selected_target_other_dimension"
                                                    )
                                                    .to_string()
                                                } else {
                                                    t!(
                                                        "MapViewer.imagegen.render.selected_target_coords",
                                                        x = block.x,
                                                        y = block.y,
                                                        z = block.z,
                                                    )
                                                    .to_string()
                                                }
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(colors.text_secondary)
                                        .child(t!("MapViewer.imagegen.render.container_pick_hint")),
                                )
                                .when(self.map_image.selected_block.is_some(), |panel| {
                                    panel.child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                crate::ui::components::button::secondary_button(
                                                    colors,
                                                    "map-container-focus",
                                                    t!("MapViewer.imagegen.render.focus_target"),
                                                )
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(|this, _, _, cx| {
                                                        this.focus_selected_map_image_block(cx);
                                                    }),
                                                ),
                                            )
                                            .when(!self.map_image_operation_busy(), |row| {
                                                row.child(
                                                    crate::ui::components::button::ghost_button(
                                                        colors,
                                                        "map-container-clear",
                                                        t!("MapViewer.imagegen.render.clear_selection"),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, _, cx| {
                                                            this.clear_selected_map_image_block(cx);
                                                        }),
                                                    ),
                                                )
                                            }),
                                    )
                                })
                                .child(self.render_container_candidates(colors, cx))
                        },
                    )
                    .when(self.map_image.install_destination == MapImageInstallDestination::NewUpFrame, |panel| panel
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_frame_placement"),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(if self.map_frame_confirmed_center().is_some() {
                                        colors.text_primary
                                    } else {
                                        colors.text_secondary
                                    })
                                    .child(self.map_frame_confirmed_center().map_or_else(
                                        || {
                                            t!(
                                                "MapViewer.imagegen.render.no_frame_center"
                                            )
                                            .to_string()
                                        },
                                        |(x, z)| {
                                            t!(
                                                "MapViewer.imagegen.render.frame_center_selected",
                                                x = x,
                                                z = z,
                                            )
                                            .to_string()
                                        },
                                    )),
                            )
                            .child(
                                crate::ui::components::button::secondary_button(
                                    colors,
                                    "map-frame-use-viewport-center",
                                    t!("MapViewer.imagegen.render.use_viewport_center"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.center_map_frame_anchor(cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child(t!("MapViewer.imagegen.render.frame_drag_hint")),
                            ),
                    )
                    .child(inspector::field_group(colors).child(self.render_import_height(colors, cx)))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            ("minecraft:stone", t!("MapViewer.imagegen.render.support_stone")),
                            ("minecraft:glass", t!("MapViewer.imagegen.render.support_glass")),
                            (
                                "minecraft:white_stained_glass",
                                t!("MapViewer.imagegen.render.support_white_stained_glass"),
                            ),
                            (
                                "minecraft:coal_block",
                                t!("MapViewer.imagegen.render.support_coal_block"),
                            ),
                            (
                                "minecraft:iron_block",
                                t!("MapViewer.imagegen.render.support_iron_block"),
                            ),
                            (
                                "minecraft:diamond_block",
                                t!("MapViewer.imagegen.render.support_diamond_block"),
                            ),
                        ]
                        .into_iter()
                        .map(|(name, label)| {
                            choice::chip(colors, label, self.map_image.frame_support == name)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        this.set_map_frame_support(name, cx)
                                    }),
                                )
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(t!("MapViewer.imagegen.render.support_hint")),
                    ))
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .whitespace_normal()
                            .text_size(px(12.0))
                            .text_color(colors.text_secondary)
                            .child(if self.map_image.source.is_none() {
                                t!("MapViewer.imagegen.render.source_missing_hint").to_string()
                            } else if grid_resolved {
                                t!(
                                    "MapViewer.imagegen.render.grid_pixel_summary",
                                    count = map_count,
                                    width = self.map_image.options.columns * 128,
                                    height = self.map_image.options.rows * 128,
                                )
                                .to_string()
                            } else {
                                t!("MapViewer.imagegen.render.grid_estimate_hint").to_string()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(t!("MapViewer.imagegen.render.grid_numbering_hint")),
                    )
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_map_grid"),
                    ))
                    .child(
                        crate::ui::components::inspector::field_group(colors)
                            .child(
                                choice::chip(
                                    colors,
                                    t!("MapViewer.imagegen.render.auto_fit_chip"),
                                    self.map_image.options.auto_fit,
                                )
                                .when(self.map_image.source.is_some() && !operation_busy, |button| {
                                    button.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _event, _window, cx| {
                                            this.enable_grid_auto_fit(cx)
                                        }),
                                    )
                                }),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child(t!("MapViewer.imagegen.render.auto_fit_hint")),
                            )
                            .child(self.render_grid_axis(GridAxis::Columns, colors, cx))
                            .child(self.render_grid_axis(GridAxis::Rows, colors, cx)),
                    )
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_fit_sampling"),
                    ))
                    .child(div().flex().flex_wrap().gap(px(6.0)).children(
                        [
                            (
                                MapImageFit::Contain,
                                t!("MapViewer.imagegen.render.fit_contain"),
                            ),
                            (
                                MapImageFit::CenterCrop,
                                t!("MapViewer.imagegen.render.fit_center_crop"),
                            ),
                            (
                                MapImageFit::Stretch,
                                t!("MapViewer.imagegen.render.fit_stretch"),
                            ),
                        ]
                        .into_iter()
                        .map(|(fit, label)| {
                            choice::chip(colors, label, self.map_image.options.fit == fit)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if this.map_image_operation_busy() { return; }
                                        this.map_image.options.fit = fit;
                                        this.start_map_preview(cx);
                                    }),
                                )
                        }),
                    ))
                    .when(self.map_image.options.fit == MapImageFit::Contain, |panel| {
                        panel
                            .child(
                                choice::chip(
                                    colors,
                                    t!("MapViewer.imagegen.render.trim_transparent_chip"),
                                    self.map_image.options.trim_transparent_edges,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        if this.map_image_operation_busy() {
                                            return;
                                        }
                                        this.map_image.options.trim_transparent_edges =
                                            !this.map_image.options.trim_transparent_edges;
                                        this.start_map_preview(cx);
                                    }),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_secondary)
                                    .child(if self.map_image.options.auto_fit {
                                        t!("MapViewer.imagegen.render.trim_auto_hint")
                                    } else {
                                        t!("MapViewer.imagegen.render.trim_manual_hint")
                                    }),
                            )
                    })
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_resample"),
                    ))
                    .child(div().flex().gap(px(6.0)).children(
                        [
                            (
                                MapResample::Nearest,
                                t!("MapViewer.imagegen.render.resample_nearest"),
                            ),
                            (MapResample::Lanczos3, SharedString::from("Lanczos3")),
                        ]
                        .into_iter()
                        .map(|(resample, label)| {
                            choice::chip(
                                colors,
                                label,
                                self.map_image.options.resample == resample,
                            )
                            .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if this.map_image_operation_busy() { return; }
                                        this.map_image.options.resample = resample;
                                        this.start_map_preview(cx);
                                    }),
                                )
                        }),
                    ))
                    .child(inspector::section_title(
                        colors,
                        t!("MapViewer.imagegen.render.section_alpha"),
                    ))
                    .child(div().flex().gap(px(6.0)).children(
                        [
                            (1_u8, t!("MapViewer.imagegen.render.alpha_keep_edges")),
                            (128, t!("MapViewer.imagegen.render.alpha_ignore_semi")),
                            (255, t!("MapViewer.imagegen.render.alpha_opaque_only")),
                        ]
                            .into_iter()
                            .map(|(alpha, label)| {
                                choice::chip(
                                    colors,
                                    label,
                                    self.map_image.options.alpha_threshold == alpha,
                                )
                                .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _event, _window, cx| {
                                            if this.map_image_operation_busy() { return; }
                                            this.map_image.options.alpha_threshold = alpha;
                                            this.start_map_preview(cx);
                                        }),
                                    )
                            }),
                    ))
                    ,
            )
            .into_any_element()
    }
}


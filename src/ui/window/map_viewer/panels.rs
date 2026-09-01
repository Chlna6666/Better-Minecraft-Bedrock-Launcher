use super::helpers::*;
use super::model::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

impl MapViewerWindowView {
    pub(super) fn top_bar_snapshot(&self, i18n: &I18n) -> MapTopBarSnapshot {
        MapTopBarSnapshot {
            window_width: self.window_width,
            asset_name: self.asset.display_name.clone(),
            version_name: SharedString::from(self.version.display_name()),
            mode: self.mode,
            dimension: self.dimension,
            y_layer: self.y_layer,
            zoom_percent: self.viewport.scale * 100.0,
            activity: compact_activity_label(i18n, self),
            chunk_transfer_progress: self.professional.chunk_transfer_progress.clone(),
        }
    }

    pub(super) fn tool_stripe_snapshot(&self) -> MapToolStripeSnapshot {
        MapToolStripeSnapshot {
            left_panel_open: self.ui_state.left_panel_open,
            right_panel_open: self.ui_state.right_panel_open,
            bottom_panel_open: self.ui_state.bottom_panel_open,
            active_left_panel: self.ui_state.active_left_panel,
            active_bottom_tab: self.ui_state.active_bottom_tab,
            active_right_panel: self.ui_state.active_right_panel,
        }
    }

    pub(super) fn menu_overlay_snapshot(&self) -> MapMenuOverlaySnapshot {
        MapMenuOverlaySnapshot {
            open: self.ui_state.top_more_open
                || self.context_menu.is_some()
                || self.player_workspace.item_context_menu.is_some(),
        }
    }

    pub(super) fn render_dock_drag_overlay(&self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .occlude()
            .cursor(match self.ui_state.dock_drag.map(|drag| drag.drag) {
                Some(DockDrag::RightPanel) => CursorStyle::ResizeColumn,
                Some(DockDrag::BottomPanel) => CursorStyle::ResizeRow,
                None => CursorStyle::Arrow,
            })
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if !event.dragging() {
                    this.release_pointer_captures(
                        "dock overlay mouse move without pressed button",
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
                this.update_dock_drag(event.position, cx);
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay mouse up", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay mouse up out", cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay stale mouse down", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay stale right mouse down", cx);
                    cx.stop_propagation();
                }),
            )
    }

    pub(super) fn render_menu_overlay(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let has_menu = self.ui_state.top_more_open
            || self.context_menu.is_some()
            || self.player_workspace.item_context_menu.is_some();
        div().absolute().inset_0().when(has_menu, |this| {
            this.child(self.menu_overlay_view.clone()).child(
                div()
                    .absolute()
                    .inset_0()
                    .when(self.ui_state.top_more_open, |this| {
                        this.child(self.render_top_more_menu(colors, cx))
                    })
                    .when_some(self.context_menu, |this, menu| {
                        this.child(self.render_context_menu(colors, menu, cx))
                    })
                    .when_some(self.player_workspace.item_context_menu, |this, menu| {
                        this.child(self.render_player_item_context_menu(colors, menu, cx))
                    }),
            )
        })
    }

    pub(super) fn render_workspace(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .overflow_hidden()
            .bg(colors.surface)
            .child(self.tool_stripe_view.clone())
            .child(splitter_line(SplitPaneAxis::Horizontal, colors.border))
            .when(self.ui_state.left_panel_open, |this| {
                this.child(self.render_left_dock(colors, cx))
                    .child(splitter_line(SplitPaneAxis::Horizontal, colors.border))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        if self.player_workspace_active()
                            && self.player_workspace.center != PlayerWorkspaceCenter::Map
                        {
                            self.render_player_center_workspace(colors, cx)
                                .into_any_element()
                        } else {
                            self.canvas_view.clone().into_any_element()
                        },
                    ),
            )
            .when(self.ui_state.right_panel_open, |this| {
                this.child(
                    split_handle(SplitPaneAxis::Horizontal, colors.border).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.begin_right_panel_resize(event.position, cx)
                        }),
                    ),
                )
                .child(self.render_right_dock(colors, cx))
            })
    }

    pub(super) fn render_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.ui_state.active_left_panel {
            MapViewerLeftPanel::Tools => self.render_tools_left_dock(colors, cx).into_any_element(),
            MapViewerLeftPanel::Players => {
                self.render_player_left_dock(colors, cx).into_any_element()
            }
        }
    }

    fn render_tools_left_dock(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let i18n = cx.global::<I18n>().clone();
        div()
            .w(px(IDE_LEFT_DOCK_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(CHROME_SECTION_GAP))
            .py(px(12.0))
            .px(px(12.0))
            .bg(colors.surface)
            .overflow_y_scrollbar()
            .child(panel_title(colors, t!("MapViewer.tools")))
            .child(self.render_viewport_inputs(colors, &i18n))
            .child(
                panel_section_body(colors)
                    .child(panel_section_header(
                        colors,
                        lucide_icons::icon_map(),
                        t!("MapViewer.dimension"),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(8.0))
                            .children(dimension_buttons(
                                self.dimension,
                                self.custom_dimension_id,
                                colors,
                                &i18n,
                                cx,
                            )),
                    )
                    .when(matches!(self.dimension, Dimension::Unknown(_)), |this| {
                        this.child(self.render_map_input(
                            colors,
                            MapInputField::DimensionId,
                            t!("MapViewer.dimension_custom_id"),
                            px(252.0),
                        ))
                    }),
            )
            .child(self.render_overlay_section(colors, cx))
    }

    pub(super) fn render_viewport_inputs(&self, colors: &ThemeColors, i18n: &I18n) -> Div {
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_search(),
                t!("MapViewer.locate_zoom"),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(self.render_map_input(
                        colors,
                        MapInputField::CenterX,
                        t!("MapViewer.center_x"),
                        px(122.0),
                    ))
                    .child(self.render_map_input(
                        colors,
                        MapInputField::CenterZ,
                        t!("MapViewer.center_z"),
                        px(122.0),
                    ))
                    .child(self.render_map_input(
                        colors,
                        MapInputField::ZoomPercent,
                        t!("MapViewer.zoom_percent"),
                        px(122.0),
                    )),
            )
    }

    pub(super) fn render_map_input(
        &self,
        colors: &ThemeColors,
        field: MapInputField,
        label: impl Into<SharedString>,
        width: Pixels,
    ) -> Div {
        let invalid = self.input_fields.validation.invalid_field == Some(field);
        div()
            .w(width)
            .flex()
            .flex_col()
            .items_start()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(if invalid {
                        colors.danger
                    } else {
                        colors.text_muted
                    })
                    .child(label.into()),
            )
            .child(
                div()
                    .w_full()
                    .h(px(30.0))
                    .px(px(8.0))
                    .rounded(px(crate::ui::theme::tokens::radius::MD))
                    .border_1()
                    .border_color(if invalid {
                        colors.danger
                    } else {
                        Hsla {
                            a: CHROME_HAIRLINE_ALPHA,
                            ..colors.border
                        }
                    })
                    .bg(Hsla {
                        a: CHROME_ELEVATED_ALPHA,
                        ..colors.surface_hover
                    })
                    .child(
                        Input::new(self.input_fields.entity(field))
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .cleanable(false)
                            .w_full()
                            .h_full()
                            .px(px(0.0))
                            .text_size(px(13.0)),
                    ),
            )
    }

    pub(super) fn render_overlay_section(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(CHROME_SECTION_GAP))
            .child(self.render_display_options(colors, cx))
            .child(self.render_data_overlays(colors, cx))
            .child(self.render_slime_analysis(colors, cx))
            .child(self.render_selection_tools(colors, cx))
    }

    fn render_display_options(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_eye(),
                t!("MapViewer.display"),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        mode_button(colors, t!("MapViewer.axis"), self.overlay_options.axis)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.toggle_axis(cx)),
                            ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.chunk_grid"),
                            self.overlay_options.dense_grid,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_dense_grid(cx)),
                        ),
                    )
                    .child(
                        mode_button(colors, t!("MapViewer.ruler"), self.overlay_options.ruler)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.toggle_ruler(cx)),
                            ),
                    ),
            )
    }

    fn render_data_overlays(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_layers(),
                t!("MapViewer.data_overlays"),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.player_overlay"),
                            self.overlay_options.players,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_player_overlay(cx)),
                        ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.entity_overlay"),
                            self.overlay_options.entities,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_entity_overlay(cx)),
                        ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.block_entity_overlay"),
                            self.overlay_options.block_entities,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.toggle_block_entity_overlay(cx)
                            }),
                        ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.village_overlay"),
                            self.overlay_options.villages,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.toggle_village_overlay(cx)
                            }),
                        ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.pending_ticks"),
                            self.overlay_options.pending_ticks,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.toggle_pending_tick_overlay(cx)
                            }),
                        ),
                    )
                    .child(
                        mode_button(
                            colors,
                            t!("MapViewer.hardcoded_spawn"),
                            self.overlay_options.hardcoded_spawn_areas,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_hsa_overlay(cx)),
                        ),
                    ),
            )
    }

    fn render_slime_analysis(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let candidate_count = self
            .professional
            .slime_window_candidates
            .as_ref()
            .map_or(0, |cache| cache.windows.len());
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_search(),
                t!("MapViewer.slime_analysis"),
            ))
            .child(
                mode_button(
                    colors,
                    t!("MapViewer.show_slime_chunks"),
                    self.overlay_options.slime_chunks,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| this.toggle_slime_overlay(cx)),
                ),
            )
            .child(panel_field_label(colors, t!("MapViewer.slime_window_size")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(slime_query_window_buttons(
                        self.slime_query_window_size,
                        colors,
                        cx,
                    )),
            )
            .when(self.professional.slime_window_candidates_loading, |this| {
                this.child(status_badge(colors, t!("MapViewer.calculating_candidates")))
            })
            .when(candidate_count > 0, |this| {
                this.child(status_badge(
                    colors,
                    t!(
                        "MapViewer.candidate_windows",
                        count = &candidate_count.to_string()
                    ),
                ))
            })
            .children(self.slime_window_candidate_buttons(colors, cx))
    }

    fn render_selection_tools(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let selection = self.professional.selection.map_or_else(
            || t!("MapViewer.no_selection").to_string(),
            |selection| {
                let bounds = selection.bounds();
                format!(
                    "chunk {},{} 至 {},{}",
                    bounds.min_chunk_x, bounds.min_chunk_z, bounds.max_chunk_x, bounds.max_chunk_z
                )
            },
        );
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_box(),
                t!("MapViewer.current_selection"),
            ))
            .child(status_badge(colors, selection))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        toolbar_button(colors, t!("MapViewer.selection_stats")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.exact_selection_stats(cx)
                            }),
                        ),
                    )
                    .child(
                        toolbar_button(colors, t!("MapViewer.clear_selection")).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.clear_professional_selection(cx)
                            }),
                        ),
                    ),
            )
    }

    pub(super) fn slime_window_candidate_buttons(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let i18n = cx.global::<I18n>().clone();
        let Some(cache) = self.professional.slime_window_candidates.as_ref() else {
            return Vec::new();
        };
        if cache.size != self.slime_query_window_size
            || self.professional_query_bounds() != Some(cache.bounds)
        {
            return Vec::new();
        }
        cache
            .windows
            .clone()
            .into_iter()
            .enumerate()
            .map(|(index, window)| {
                let index = (index + 1).to_string();
                let slime_count = window.slime_count.to_string();
                let total_count = window.total_count.to_string();
                let center_x = window.center.x.to_string();
                let center_z = window.center.z.to_string();
                let label = t!(
                    "MapViewer.candidate_window",
                    index = &index,
                    slime = &slime_count,
                    total = &total_count,
                    x = &center_x,
                    z = &center_z
                );
                toolbar_button(colors, label)
                    .w_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.highlight_slime_window(window.clone(), cx)
                        }),
                    )
                    .into_any_element()
            })
            .collect()
    }

    pub(super) fn render_status_bar(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let i18n = cx.global::<I18n>().clone();
        let validation = self
            .input_fields
            .validation
            .message
            .clone()
            .unwrap_or_else(|| SharedString::from("-"));
        let fps = format!("{:.1}", self.frame_stats.fps);
        let tile_loaded = self.tile_manager.loaded_count().to_string();
        let tile_queued = self.tile_manager.queued_count().to_string();
        let tile_loading = self.tile_manager.loading_count().to_string();
        let tile_failed = self.tile_manager.failed_count().to_string();
        let tile_empty = self.tile_manager.empty_count().to_string();
        let batches = self.tile_reveal_state.ready_batches.to_string();
        let last_batch = self.tile_reveal_state.last_batch_size.to_string();
        let tiles_diagnostics = t!(
            "MapViewer.tiles_diagnostics",
            fps = &fps,
            loaded = &tile_loaded,
            queued = &tile_queued,
            loading = &tile_loading,
            failed = &tile_failed,
            empty = &tile_empty,
            batches = &batches,
            last = &last_batch
        );
        let chunks = self
            .chunk_bounds
            .map(|bounds| bounds.chunk_count)
            .unwrap_or(0)
            .to_string();
        let cache_probes = self.render_stats.cache_probes.to_string();
        let cache_hits = self.render_stats.cache_disk_fresh_hits.to_string();
        let cache_misses = self.render_stats.cache_misses.to_string();
        let cache_empty = self.render_stats.cache_empty_negative_hits.to_string();
        let cache_read = self.render_stats.cache_read_ms.to_string();
        let cache_decode = self.render_stats.cache_decode_ms.to_string();
        let blob_decode = self.render_stats.tile_blob_decode_ms.to_string();
        let chunk_diagnostics = t!(
            "MapViewer.chunk_diagnostics",
            chunks = &chunks,
            probes = &cache_probes,
            hits = &cache_hits,
            misses = &cache_misses,
            empty = &cache_empty,
            read = &cache_read,
            decode = &cache_decode,
            blob = &blob_decode
        );
        overlay_panel(colors)
            .left(px(12.0))
            .bottom(px(12.0))
            .max_w(px(620.0))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(t!("MapViewer.diagnostics")),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
                    .child(tiles_diagnostics),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
                    .child(chunk_diagnostics),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
                    .child(format!(
                        "渲染 线程 {} · 后端 {} · CPU 瓦片 {} · GPU 瓦片 {} · {} · GPU 队列 {}ms · 准备/上传/派发/回读 {}/{}/{}/{}ms · 上传/回读 {}/{} 字节 · 并发峰值 {} · 缓冲复用 {}{}",
                        self.render_stats.peak_worker_threads,
                        resolved_backend_label_zh(self.render_stats.resolved_backend),
                        self.render_stats.cpu_tiles,
                        self.render_stats.gpu_tiles,
                        render_gpu_backend_status_zh(&self.render_stats),
                        self.render_stats.gpu_queue_wait_ms,
                        self.render_stats.gpu_prepare_ms,
                        self.render_stats.gpu_upload_ms,
                        self.render_stats.gpu_dispatch_ms,
                        self.render_stats.gpu_readback_ms,
                        self.render_stats.gpu_uploaded_bytes,
                        self.render_stats.gpu_readback_bytes,
                        self.render_stats.gpu_peak_in_flight,
                        self.render_stats.gpu_buffer_reuses,
                        self.render_stats
                            .gpu_fallback_reason
                            .as_ref()
                            .map(|reason| format!(" · CPU 回退原因 {}", localize_gpu_reason(reason)))
                            .unwrap_or_default(),
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
                    .child(format!(
                        "数据 缓存 区域 {}/{} · 瓦片索引 T/V/M/E {}/{}/{}/{} · 索引读 {}ms · 依赖校验 {}ms · 写入丢弃 {} · 损坏 miss {} · 局部 chunk {} · 刷新渲染 {} · 冷渲染 {} · 队列未命中 {} · 距离² {} · 缺失区块 {} · 未知方块 {} · 透明像素 {} · 错误像素 {} · 校验 {}",
                        self.render_stats.region_cache_hits,
                        self.render_stats.region_cache_misses,
                        self.render_stats.tile_index_trusted_hits,
                        self.render_stats.tile_index_validated_hits,
                        self.render_stats.tile_index_misses,
                        self.render_stats.tile_index_empty_hits,
                        self.render_stats.tile_index_read_ms,
                        self.render_stats.tile_dep_validation_ms,
                        self.render_stats.tile_cache_writer_dropped,
                        self.render_stats.index_corrupt_misses,
                        self.partial_refreshed_chunks,
                        self.refresh_rendered_tiles,
                        self.cold_rendered_tiles,
                        self.tile_manager.cache_miss_count(),
                        self.last_queue_distance_squared,
                        self.diagnostics.missing_chunks,
                        self.diagnostics.unknown_blocks,
                        self.diagnostics.transparent_pixels,
                        self.diagnostics.purple_error_pixels,
                        validation.as_ref(),
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
        ,
            )
    }
}

pub(super) fn dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "主世界".to_string(),
        Dimension::Nether => "下界".to_string(),
        Dimension::End => "末地".to_string(),
        Dimension::Unknown(id) => format!("维度 {id}"),
    }
}

pub(super) fn compact_activity_label(i18n: &I18n, view: &MapViewerWindowView) -> SharedString {
    if let Some(progress) = view.professional.chunk_transfer_progress.as_ref() {
        return progress.label().to_string().into();
    }
    if view.metadata_loading {
        return t!("MapViewer.scanning");
    }
    if view.render_batch_active {
        let running_batches = view.render_cancels.len();
        let loading = view.tile_manager.loading_count().to_string();
        let batches = running_batches.to_string();
        return t!(
            "MapViewer.loading_batches",
            loading = &loading,
            batches = &batches
        );
    }
    let queued = view.tile_manager.queued_count();
    if queued > 0 {
        return t!("MapViewer.waiting", count = &queued.to_string());
    }
    if view.tile_manager.failed_count() > 0 {
        return t!(
            "MapViewer.failed_count",
            count = &view.tile_manager.failed_count().to_string()
        );
    }
    if view.tile_manager.empty_count() > 0 {
        return t!(
            "MapViewer.empty_count",
            count = &view.tile_manager.empty_count().to_string()
        );
    }
    t!("MapViewer.ready")
}

pub(super) fn panel_title(colors: &ThemeColors, title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(title.into())
}

pub(super) fn panel_section_body(colors: &ThemeColors) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        // No border box: sections are separated by whitespace + a header row,
        // reducing the visual noise the old bordered card produced.
        .child(div().h(px(1.0)).w_full().bg(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        }))
}

/// Icon + label header for a left-dock section.
pub(super) fn panel_section_header(
    colors: &ThemeColors,
    icon_path: &'static str,
    title: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_muted)
        .child(themed_icon(
            icon_path,
            CHROME_TOOLBAR_ICON_SIZE,
            colors.text_muted,
        ))
        .child(title.into())
}

pub(super) fn panel_field_label(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_secondary)
        .child(label.into())
}

pub(super) fn dock_close_button(colors: &ThemeColors) -> Div {
    div()
        .size(px(30.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .cursor(CursorStyle::PointingHand)
        .text_color(colors.text_secondary)
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            })
        })
        .child(themed_icon(
            lucide_icons::icon_x(),
            CHROME_TAB_ICON_SIZE,
            colors.text_secondary,
        ))
}

/// Tab button variant with a leading icon (for the bottom dock tab strip).
pub(super) fn tab_button_with_icon(
    colors: &ThemeColors,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    active: bool,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::XS))
        .text_size(px(12.0))
        .cursor(CursorStyle::PointingHand)
        .text_color(if active {
            colors.text_primary
        } else {
            colors.text_secondary
        })
        .bg(if active {
            Hsla {
                a: 0.20,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.0,
                ..colors.surface
            }
        })
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            })
        })
        .child(themed_icon(
            icon_path,
            CHROME_TAB_ICON_SIZE,
            colors.text_muted,
        ))
        .child(label.into())
}

pub(super) fn db_node_icon(kind: &DbTreeNodeKind) -> &'static str {
    match kind {
        DbTreeNodeKind::Dimension(_) => "◇",
        DbTreeNodeKind::Chunk(_) => "▣",
    }
}

pub(super) fn overlay_panel(colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .px(px(8.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.86,
            ..colors.surface
        })
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
}

pub(super) fn separator(colors: &ThemeColors) -> Div {
    div().w(px(1.0)).h(px(22.0)).bg(Hsla {
        a: CHROME_HAIRLINE_ALPHA,
        ..colors.border
    })
}

pub(super) fn toolbar_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        })
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA + 0.15,
                ..colors.surface_hover
            })
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(label.into())
}

pub(super) fn mode_button(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    active: bool,
) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(if active { colors.accent } else { colors.border })
        .bg(if active {
            Hsla {
                a: 0.18,
                ..colors.accent
            }
        } else {
            Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            }
        })
        .hover(|style| {
            if active {
                style
            } else {
                style.bg(Hsla {
                    a: CHROME_ELEVATED_ALPHA + 0.15,
                    ..colors.surface_hover
                })
            }
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active {
            colors.accent
        } else {
            colors.text_primary
        })
        .child(label.into())
}

pub(super) fn status_badge(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(9.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .text_size(px(12.0))
        .text_color(colors.text_secondary)
        .child(label.into())
}

pub(super) fn danger_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.danger
        })
        .bg(Hsla {
            a: 0.14,
            ..colors.danger
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.danger)
        .child(label.into())
}

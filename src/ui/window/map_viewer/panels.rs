use super::helpers::*;
use super::model::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

impl MapViewerWindowView {
    pub(super) fn top_bar_snapshot(&self) -> MapTopBarSnapshot {
        MapTopBarSnapshot {
            window_width: self.window_width,
            asset_name: self.asset.display_name.clone(),
            version_name: SharedString::from(self.version.display_name()),
            mode: self.mode,
            dimension: self.dimension,
            y_layer: self.y_layer,
            zoom_percent: self.viewport.scale * 100.0,
            activity: SharedString::from(compact_activity_label(self)),
            chunk_transfer_progress: self.professional.chunk_transfer_progress.clone(),
        }
    }

    pub(super) fn tool_stripe_snapshot(&self) -> MapToolStripeSnapshot {
        MapToolStripeSnapshot {
            left_panel_open: self.ui_state.left_panel_open,
            right_panel_open: self.ui_state.right_panel_open,
            bottom_panel_open: self.ui_state.bottom_panel_open,
            active_bottom_tab: self.ui_state.active_bottom_tab,
            active_right_panel: self.ui_state.active_right_panel,
        }
    }

    pub(super) fn menu_overlay_snapshot(&self) -> MapMenuOverlaySnapshot {
        MapMenuOverlaySnapshot {
            open: self.ui_state.top_more_open || self.context_menu.is_some(),
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
        let has_menu = self.ui_state.top_more_open || self.context_menu.is_some();
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
                    }),
            )
        })
    }

    pub(super) fn render_workspace(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
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
                    .min_w(px(MIN_CENTER_WIDTH))
                    .min_h(px(0.0))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.canvas_view.clone()),
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
    ) -> impl IntoElement {
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
            .child(panel_title(colors, "地图工具"))
            .child(self.render_viewport_inputs(colors))
            .child(
                panel_section_body(colors)
                    .child(panel_section_header(
                        colors,
                        lucide_icons::icon_map(),
                        "维度",
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
                                cx,
                            )),
                    )
                    .when(matches!(self.dimension, Dimension::Unknown(_)), |this| {
                        this.child(self.render_map_input(
                            colors,
                            MapInputField::DimensionId,
                            "自定义维度 ID",
                            px(252.0),
                        ))
                    }),
            )
            .child(self.render_overlay_section(colors, cx))
    }

    pub(super) fn render_viewport_inputs(&self, colors: &ThemeColors) -> Div {
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_search(),
                "定位与缩放",
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
                        "中心 X",
                        px(122.0),
                    ))
                    .child(self.render_map_input(
                        colors,
                        MapInputField::CenterZ,
                        "中心 Z",
                        px(122.0),
                    ))
                    .child(self.render_map_input(
                        colors,
                        MapInputField::ZoomPercent,
                        "缩放百分比",
                        px(122.0),
                    )),
            )
    }

    pub(super) fn render_map_input(
        &self,
        colors: &ThemeColors,
        field: MapInputField,
        label: &'static str,
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
                    .child(label),
            )
            .child(
                div()
                    .w_full()
                    .h(px(30.0))
                    .px(px(8.0))
                    .rounded(px(8.0))
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
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_eye(),
                "地图显示",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        mode_button(colors, "坐标轴", self.overlay_options.axis).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_axis(cx)),
                        ),
                    )
                    .child(
                        mode_button(colors, "区块网格", self.overlay_options.dense_grid)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| this.toggle_dense_grid(cx)),
                            ),
                    )
                    .child(
                        mode_button(colors, "地图标尺", self.overlay_options.ruler).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| this.toggle_ruler(cx)),
                        ),
                    ),
            )
    }

    fn render_data_overlays(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_layers(),
                "数据叠加",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        mode_button(colors, "生物实体", self.overlay_options.entities)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_entity_overlay(cx)
                                }),
                            ),
                    )
                    .child(
                        mode_button(colors, "方块实体", self.overlay_options.block_entities)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_block_entity_overlay(cx)
                                }),
                            ),
                    )
                    .child(
                        mode_button(colors, "村庄范围", self.overlay_options.villages)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.toggle_village_overlay(cx)
                                }),
                            ),
                    )
                    .child(
                        mode_button(colors, "计划刻队列", self.overlay_options.pending_ticks)
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
                            "硬编码生成区",
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
        let candidate_count = self
            .professional
            .slime_window_candidates
            .as_ref()
            .map_or(0, |cache| cache.windows.len());
        panel_section_body(colors)
            .child(panel_section_header(
                colors,
                lucide_icons::icon_search(),
                "史莱姆群落分析",
            ))
            .child(
                mode_button(colors, "显示史莱姆区块", self.overlay_options.slime_chunks)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.toggle_slime_overlay(cx)),
                    ),
            )
            .child(panel_field_label(colors, "连续窗口大小（区块）"))
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
                this.child(status_badge(colors, "正在计算候选窗口"))
            })
            .when(candidate_count > 0, |this| {
                this.child(status_badge(colors, format!("候选窗口 {candidate_count}")))
            })
            .children(self.slime_window_candidate_buttons(colors, cx))
    }

    fn render_selection_tools(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let selection = self.professional.selection.map_or_else(
            || "未选择区块".to_string(),
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
                "当前选区",
            ))
            .child(status_badge(colors, selection))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(toolbar_button(colors, "统计选区").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.query_selection_stats(cx)),
                    ))
                    .child(toolbar_button(colors, "清除选区").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.clear_professional_selection(cx)
                        }),
                    )),
            )
    }

    pub(super) fn slime_window_candidate_buttons(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
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
                let label = format!(
                    "候选 {} · {}/{} 史莱姆 · 中心 {},{}",
                    index + 1,
                    window.slime_count,
                    window.total_count,
                    window.center.x,
                    window.center.z
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

    pub(super) fn render_status_bar(&self, colors: &ThemeColors) -> Div {
        let validation = self
            .input_fields
            .validation
            .message
            .clone()
            .unwrap_or_else(|| SharedString::from("-"));
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
                    .child("诊断"),
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
                    .child(format!(
                        "帧率 {:.1} · 瓦片 已加载 {} / 排队 {} / 加载中 {} / 失败 {} / 空瓦片 {} · 显示批次 {} · 上批 {}",
                        self.frame_stats.fps,
                        self.tile_manager.loaded_count(),
                        self.tile_manager.queued_count(),
                        self.tile_manager.loading_count(),
                        self.tile_manager.failed_count(),
                        self.tile_manager.invalid_count(),
                        self.tile_reveal_state.ready_batches,
                        self.tile_reveal_state.last_batch_size,
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_muted)
                    .child(format!(
                        "区块 {:?} · 渲染缓存 探测 {} / 命中 {} / 未命中 {} / 空负缓存 {} · 读取 {}ms · 解压 {}ms · blob 解码 {}ms",
                        self.chunk_bounds.map(|bounds| bounds.chunk_count).unwrap_or(0),
                        self.render_stats.cache_probes,
                        self.render_stats.cache_disk_fresh_hits,
                        self.render_stats.cache_misses,
                        self.render_stats.cache_empty_negative_hits,
                        self.render_stats.cache_read_ms,
                        self.render_stats.cache_decode_ms,
                        self.render_stats.tile_blob_decode_ms,
                    )),
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
                    .child(format!(
                        "Probe 诊断 最近编辑 #{} {} · edit后 manifest_probe_start {} 次",
                        self.manifest_probe_diagnostics.last_edit_serial,
                        self.manifest_probe_diagnostics.last_edit_label,
                        self.manifest_probe_diagnostics.probe_starts_since_last_edit,
                    )),
            )
            .children(
                self.manifest_probe_diagnostics
                    .recent_events
                    .iter()
                    .rev()
                    .map(|event| {
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child(event.clone())
                            .into_any_element()
                    }),
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

pub(super) fn compact_activity_label(view: &MapViewerWindowView) -> String {
    if let Some(progress) = view.professional.chunk_transfer_progress.as_ref() {
        return progress.label().to_string();
    }
    if view.metadata_loading {
        return "扫描中".to_string();
    }
    if view.render_batch_active {
        let running_batches = view.render_cancels.len();
        return format!(
            "加载 {} · 批次 {running_batches}",
            view.tile_manager.loading_count()
        );
    }
    let queued = view.tile_manager.queued_count();
    if queued > 0 {
        return format!("等待 {queued}");
    }
    if view.tile_manager.failed_count() > 0 {
        return format!("失败 {}", view.tile_manager.failed_count());
    }
    if view.tile_manager.invalid_count() > 0 {
        return format!("空 {}", view.tile_manager.invalid_count());
    }
    "就绪".to_string()
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
        .rounded(px(6.0))
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
        .rounded(px(5.0))
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
        .rounded(px(10.0))
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
        .rounded(px(8.0))
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
        .rounded(px(8.0))
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
        .rounded(px(8.0))
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
        .rounded(px(8.0))
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

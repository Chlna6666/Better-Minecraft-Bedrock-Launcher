use super::model::MapViewerWindowView;
use super::panels::toolbar_button;
use super::prelude::*;
use super::tile_state::TilePriority;

impl MapViewerWindowView {
    pub(super) fn refresh_history(&mut self, cx: &mut Context<Self>) {
        self.history.loading = true;
        self.history.error = None;
        let world_path = self.world_path.clone();
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move { list_history(&world_path) })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                this.history.loading = false;
                match result {
                    Ok(entries) => {
                        let selected_exists =
                            this.history
                                .selected_entry_id
                                .as_ref()
                                .is_some_and(|selected| {
                                    entries.iter().any(|entry| &entry.id == selected)
                                });
                        this.history.entries = Arc::new(entries);
                        if !selected_exists {
                            this.history.selected_entry_id =
                                this.history.entries.first().map(|entry| entry.id.clone());
                        }
                        this.load_selected_history_visualization(cx);
                    }
                    Err(error) => {
                        this.history.error = Some(SharedString::from(error));
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn select_history_entry(&mut self, id: String, cx: &mut Context<Self>) {
        self.history.selected_entry_id = Some(id);
        self.load_selected_history_visualization(cx);
        cx.notify();
    }

    fn load_selected_history_visualization(&mut self, cx: &mut Context<Self>) {
        if !self.history.visualization_enabled {
            self.professional.overlay_generation =
                self.professional.overlay_generation.saturating_add(1);
            self.last_synced_canvas_snapshot_key = None;
            return;
        }
        let Some(entry_id) = self.history.selected_entry_id.clone() else {
            self.history.visualization = Arc::new(MapHistoryVisualization::default());
            self.history.visualization_loading = false;
            self.history.visualization_error = None;
            self.professional.overlay_generation =
                self.professional.overlay_generation.saturating_add(1);
            self.last_synced_canvas_snapshot_key = None;
            return;
        };
        self.history.visualization_loading = true;
        self.history.visualization_error = None;
        let world_path = self.world_path.clone();
        let task_entry_id = entry_id.clone();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(
                    async move { load_history_visualization(&world_path, &task_entry_id) },
                )
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                if this.history.selected_entry_id.as_deref() != Some(entry_id.as_str()) {
                    return;
                }
                this.history.visualization_loading = false;
                match result {
                    Ok(visualization) => {
                        this.history.visualization = Arc::new(visualization);
                        this.history.visualization_error = None;
                    }
                    Err(error) => {
                        this.history.visualization = Arc::new(MapHistoryVisualization::default());
                        this.history.visualization_error = Some(SharedString::from(error));
                    }
                }
                this.professional.overlay_generation =
                    this.professional.overlay_generation.saturating_add(1);
                this.last_synced_canvas_snapshot_key = None;
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn toggle_history_visualization(&mut self, cx: &mut Context<Self>) {
        self.history.visualization_enabled = !self.history.visualization_enabled;
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
        if self.history.visualization_enabled {
            self.load_selected_history_visualization(cx);
        }
        cx.notify();
    }

    fn toggle_history_visualization_filter(
        &mut self,
        kind: MapHistoryVisualFilterKind,
        cx: &mut Context<Self>,
    ) {
        self.history.visualization_filter.toggle(kind);
        self.professional.overlay_generation =
            self.professional.overlay_generation.saturating_add(1);
        self.last_synced_canvas_snapshot_key = None;
        cx.notify();
    }

    pub(super) fn undo_map_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_history_operation(
            "撤回修改",
            "撤回历史",
            |world_path, progress| apply_undo_with_progress(&world_path, progress),
            cx,
        );
    }

    pub(super) fn redo_map_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_history_operation(
            "重做修改",
            "重做历史",
            |world_path, progress| apply_redo_with_progress(&world_path, progress),
            cx,
        );
    }

    pub(super) fn restore_selected_history_entry(&mut self, cx: &mut Context<Self>) {
        let Some(entry_id) = self.history.selected_entry_id.clone() else {
            toast::error(cx, SharedString::from("请先选择一个历史项"));
            return;
        };
        let selected_chunks = self
            .history
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .map(|entry| entry.chunks.iter().copied().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        self.apply_history_operation(
            "回档历史",
            "回档历史",
            move |world_path, progress| {
                if !selected_chunks.is_empty() {
                    create_restore_protection_point(
                        &world_path,
                        selected_chunks.clone(),
                        "回档前保护点",
                    )?;
                }
                restore_history_entry_with_progress(&world_path, &entry_id, progress)
            },
            cx,
        );
    }

    pub(super) fn create_map_backup(&mut self, cx: &mut Context<Self>) {
        let world_path = self.world_path.clone();
        let map_name = self.asset.display_name.to_string();
        self.begin_edit_toast(SharedString::from("正在创建地图整图备份..."), cx);
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::ui::views::manage::data::backup_map(
                        &world_path.to_string_lossy(),
                        &map_name,
                    )
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                match result {
                    Ok(path) => {
                        let message = format!("地图备份已创建: {path}");
                        this.status = SharedString::from(message.clone());
                        this.resolve_edit_toast(
                            toast::ToastKind::Success,
                            SharedString::from(message),
                            cx,
                        );
                    }
                    Err(error) => {
                        this.status = SharedString::from(error.clone());
                        this.resolve_edit_toast(
                            toast::ToastKind::Error,
                            SharedString::from(error),
                            cx,
                        );
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn open_history_tab(&mut self, cx: &mut Context<Self>) {
        self.ui_state.bottom_panel_open = true;
        self.ui_state.active_bottom_tab = MapViewerBottomTab::History;
        self.refresh_history(cx);
        cx.notify();
    }

    pub(super) fn clear_history(&mut self, cx: &mut Context<Self>) {
        let history_dir = history_dir_for_world(&self.world_path);
        self.history.loading = true;
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    match std::fs::remove_dir_all(&history_dir) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(format!("清理历史失败: {error}")),
                    }
                })
                .await;
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            view.update(cx, move |this, cx| {
                this.history.loading = false;
                match result {
                    Ok(()) => {
                        this.history.entries = Arc::new(Vec::new());
                        this.history.selected_entry_id = None;
                        this.history.visualization = Arc::new(MapHistoryVisualization::default());
                        this.history.visualization_error = None;
                        this.professional.overlay_generation =
                            this.professional.overlay_generation.saturating_add(1);
                        this.last_synced_canvas_snapshot_key = None;
                        toast::success(cx, SharedString::from("历史已清理"));
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn render_history_panel(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let selected = self
            .history
            .selected_entry_id
            .as_ref()
            .and_then(|id| self.history.entries.iter().find(|entry| &entry.id == id));
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .gap(px(10.0))
            .p(px(10.0))
            .child(
                div()
                    .w(px(430.0))
                    .flex_none()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(self.render_history_toolbar(colors, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.0))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.24,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: 0.42,
                                ..colors.surface_hover
                            })
                            .overflow_y_scrollbar()
                            .when(self.history.entries.is_empty(), |this| {
                                this.child(
                                    div()
                                        .p(px(12.0))
                                        .text_size(px(12.0))
                                        .line_height(px(18.0))
                                        .text_color(colors.text_muted)
                                        .child(if self.history.loading {
                                            "正在加载历史..."
                                        } else {
                                            "还没有地图编辑历史。"
                                        }),
                                )
                            })
                            .children(self.history.entries.iter().map(|entry| {
                                self.render_history_entry(colors, entry, cx)
                                    .into_any_element()
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.38,
                        ..colors.surface_hover
                    })
                    .p(px(12.0))
                    .overflow_y_scrollbar()
                    .text_size(px(12.0))
                    .line_height(px(19.0))
                    .text_color(colors.text_secondary)
                    .child(history_visualization_legend(colors, &self.history, cx))
                    .child(history_detail_text(
                        selected,
                        self.history.error.as_ref(),
                        &self.history,
                    )),
            )
    }

    fn render_history_toolbar(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let selected = self
            .history
            .selected_entry_id
            .as_ref()
            .and_then(|id| self.history.entries.iter().find(|entry| &entry.id == id));
        let has_undo = self
            .history
            .entries
            .iter()
            .any(|entry| entry.status == MapHistoryEntryStatus::Success);
        let has_redo = self
            .history
            .entries
            .iter()
            .any(|entry| entry.status == MapHistoryEntryStatus::Undone);
        let can_restore =
            selected.is_some_and(|entry| entry.status != MapHistoryEntryStatus::Failed);
        let can_clear = !self.history.entries.is_empty();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(history_toolbar_action(
                colors,
                "刷新",
                !self.history.loading,
                cx,
                |this, cx| {
                    this.refresh_history(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                "撤回",
                !self.history.applying && has_undo,
                cx,
                |this, cx| {
                    this.undo_map_edit(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                "重做",
                !self.history.applying && has_redo,
                cx,
                |this, cx| {
                    this.redo_map_edit(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                "回档到此",
                !self.history.applying && can_restore,
                cx,
                |this, cx| {
                    this.restore_selected_history_entry(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                "备份",
                !self.history.applying,
                cx,
                |this, cx| {
                    this.create_map_backup(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                if self.history.visualization_enabled {
                    "隐藏差异"
                } else {
                    "地图差异"
                },
                selected.is_some(),
                cx,
                |this, cx| {
                    this.toggle_history_visualization(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                "清理",
                !self.history.loading && !self.history.applying && can_clear,
                cx,
                |this, cx| {
                    this.clear_history(cx);
                },
            ))
    }

    fn render_history_entry(
        &self,
        colors: &ThemeColors,
        entry: &MapHistoryEntry,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = self.history.selected_entry_id.as_ref() == Some(&entry.id);
        let id = entry.id.clone();
        div()
            .px(px(10.0))
            .py(px(8.0))
            .cursor(CursorStyle::PointingHand)
            .border_b_1()
            .border_color(Hsla {
                a: 0.16,
                ..colors.border
            })
            .bg(if selected {
                Hsla {
                    a: 0.22,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.surface
                }
            })
            .hover(move |style| {
                if selected {
                    style
                } else {
                    style.bg(Hsla {
                        a: 0.58,
                        ..colors.surface_hover
                    })
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.select_history_entry(id.clone(), cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(history_entry_timeline_color(entry)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(colors.text_primary)
                                    .child(entry.kind_label()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(colors.text_muted)
                                    .child(entry.short_status()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(entry.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child(format!(
                                "{} · chunk {} · raw {} · {} bytes · 存储 {} bytes",
                                format_history_time(entry.timestamp_secs),
                                entry.chunks.len(),
                                entry.raw_delta_count,
                                entry.raw_delta_bytes,
                                entry.stored_bytes
                            )),
                    ),
            )
    }

    fn apply_history_operation(
        &mut self,
        label: &'static str,
        phase: &'static str,
        operation: impl FnOnce(
            PathBuf,
            Box<dyn FnMut(MapHistoryApplyProgress) + Send>,
        ) -> Result<MapHistoryApplyOutcome, String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.history.applying {
            toast::error(cx, SharedString::from("已有历史操作正在执行"));
            return;
        }
        self.history.applying = true;
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase: SharedString::from(phase),
            completed: 0,
            total: 1,
        });
        self.begin_edit_toast(SharedString::from(format!("正在{label}...")), cx);
        let world_path = self.world_path.clone();
        let metadata_generation = self.metadata_generation;
        cx.notify();
        cx.spawn(async move |handle, cx| {
            enum HistoryApplyEvent {
                Progress(ChunkTransferProgress),
                Complete(Result<MapHistoryApplyOutcome, String>),
            }

            let (event_sender, mut event_receiver) = unbounded::<HistoryApplyEvent>();
            let progress_sender = event_sender.clone();
            let completion_sender = event_sender.clone();
            let world_path_for_task = world_path.clone();
            let task = cx.background_spawn(async move {
                let progress = Box::new(move |progress: MapHistoryApplyProgress| {
                    if progress_sender
                        .unbounded_send(HistoryApplyEvent::Progress(ChunkTransferProgress {
                            phase: progress.phase,
                            completed: progress.completed,
                            total: progress.total,
                        }))
                        .is_err()
                    {
                        tracing::debug!("history operation progress receiver dropped");
                    }
                });
                let result = operation(world_path_for_task, progress);
                if completion_sender
                    .unbounded_send(HistoryApplyEvent::Complete(result))
                    .is_err()
                {
                    tracing::debug!("history operation completion receiver dropped");
                }
            });
            task.detach();
            let Some(view) = handle.upgrade() else {
                return Ok(());
            };
            while let Some(event) = event_receiver.next().await {
                let is_complete = matches!(&event, HistoryApplyEvent::Complete(_));
                view.update(cx, move |this, cx| {
                    if this.metadata_generation != metadata_generation {
                        if is_complete {
                            this.history.applying = false;
                            this.finish_chunk_transfer_progress();
                        }
                        cx.notify();
                        return;
                    }
                    match event {
                        HistoryApplyEvent::Progress(progress) => {
                            this.set_chunk_transfer_progress(progress);
                        }
                        HistoryApplyEvent::Complete(result) => {
                            this.history.applying = false;
                            match result {
                                Ok(outcome) => {
                                    this.complete_chunk_transfer_progress();
                                    if outcome.refresh_all_tiles {
                                        this.invalidate_tiles(cx);
                                        this.refresh_metadata(cx);
                                        this.ensure_visible_tiles(cx);
                                    } else {
                                        let invalidation = MapEditInvalidation::chunks(
                                            outcome.affected_chunks.clone(),
                                        )
                                        .with_metadata();
                                        this.apply_map_edit_invalidation_with_tile_priority(
                                            &invalidation,
                                            TilePriority::EditRefresh,
                                            cx,
                                        );
                                    }
                                    if outcome.level_dat_changed && !outcome.refresh_all_tiles {
                                        this.refresh_metadata(cx);
                                    }
                                    this.status = SharedString::from(outcome.message.clone());
                                    this.resolve_edit_toast(
                                        toast::ToastKind::Success,
                                        SharedString::from(outcome.message),
                                        cx,
                                    );
                                    this.refresh_history(cx);
                                }
                                Err(error) => {
                                    this.finish_chunk_transfer_progress();
                                    this.status = SharedString::from(error.clone());
                                    this.resolve_edit_toast(
                                        toast::ToastKind::Error,
                                        SharedString::from(error),
                                        cx,
                                    );
                                    this.refresh_history(cx);
                                }
                            }
                        }
                    }
                    cx.notify();
                })?;
                if is_complete {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

fn history_toolbar_action(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    enabled: bool,
    cx: &mut Context<MapViewerWindowView>,
    action: impl Fn(&mut MapViewerWindowView, &mut Context<MapViewerWindowView>) + 'static,
) -> Div {
    let button = history_toolbar_button(colors, label, enabled);
    if enabled {
        button.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| action(this, cx)),
        )
    } else {
        button
    }
}

fn history_toolbar_button(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Div {
    toolbar_button(colors, label)
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::Arrow
        })
        .text_color(if enabled {
            colors.text_primary
        } else {
            colors.text_muted
        })
        .bg(Hsla {
            a: if enabled { 0.58 } else { 0.24 },
            ..colors.surface_hover
        })
        .border_color(Hsla {
            a: if enabled { 0.20 } else { 0.12 },
            ..colors.border
        })
}

fn history_entry_timeline_color(entry: &MapHistoryEntry) -> Rgba {
    if entry.status == MapHistoryEntryStatus::Failed {
        return rgb(0xef4444);
    }
    if entry.status == MapHistoryEntryStatus::Undone {
        return rgb(0x94a3b8);
    }
    match entry.kind {
        MapHistoryEntryKind::ChunkDelete | MapHistoryEntryKind::RecordDelete => rgb(0xef4444),
        MapHistoryEntryKind::ChunkPaste | MapHistoryEntryKind::RecordSave => rgb(0x3b82f6),
        _ => rgb(0x8b5cf6),
    }
}

fn history_visualization_legend(
    colors: &ThemeColors,
    history: &MapHistoryState,
    cx: &mut Context<MapViewerWindowView>,
) -> Div {
    let visualization = history.visualization.as_ref();
    div()
        .mb(px(10.0))
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.32,
            ..colors.surface
        })
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(history_filter_chip(
            "新增",
            MapHistoryVisualFilterKind::Added,
            visualization,
            history.visualization_filter,
            rgb(0x3b82f6),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            "删除",
            MapHistoryVisualFilterKind::Removed,
            visualization,
            history.visualization_filter,
            rgb(0xef4444),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            "修改",
            MapHistoryVisualFilterKind::Modified,
            visualization,
            history.visualization_filter,
            rgb(0x8b5cf6),
            colors,
            cx,
        ))
        .child(history_summary_badge(
            format!("混合 {} chunk", visualization.mixed_chunks),
            rgb(0xf59e0b),
            colors,
        ))
        .child(history_summary_badge(
            format!(
                "精确 {} · 部分 {} · 记录级 {}",
                visualization.precise_chunks,
                visualization.partial_chunks,
                visualization.record_only_chunks
            ),
            rgb(0x10b981),
            colors,
        ))
        .when(history.visualization_loading, |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child("正在解析块级差异..."),
            )
        })
        .when(!history.visualization_enabled, |this| {
            this.child(div().text_color(colors.text_muted).child("地图差异已隐藏"))
        })
        .when(!history.visualization_filter.any_enabled(), |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child("所有差异类型均已过滤"),
            )
        })
        .when_some(history.visualization_error.clone(), |this, error| {
            this.child(div().text_color(colors.danger).child(error))
        })
}

fn history_filter_chip(
    label: &'static str,
    kind: MapHistoryVisualFilterKind,
    visualization: &MapHistoryVisualization,
    filter: MapHistoryVisualFilter,
    color: Rgba,
    colors: &ThemeColors,
    cx: &mut Context<MapViewerWindowView>,
) -> Div {
    let active = filter.includes(kind);
    let blocks = visualization.kind_blocks(kind);
    let records = visualization.kind_records(kind);
    let chunks = visualization
        .chunks
        .iter()
        .filter(|chunk| chunk.has_kind(kind))
        .count();
    let metric = if blocks > 0 {
        format!("{} block", format_history_count(blocks))
    } else {
        format!("{records} record")
    };
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(if active {
            color.alpha(0.48)
        } else {
            Hsla {
                a: 0.14,
                ..colors.border
            }
            .into()
        })
        .bg(if active {
            color.alpha(0.12)
        } else {
            Hsla {
                a: 0.16,
                ..colors.surface_hover
            }
            .into()
        })
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.toggle_history_visualization_filter(kind, cx);
            }),
        )
        .child(
            div()
                .w(px(9.0))
                .h(px(9.0))
                .rounded(px(2.0))
                .bg(color.alpha(if active { 0.68 } else { 0.20 })),
        )
        .child(
            div()
                .text_color(if active {
                    colors.text_secondary
                } else {
                    colors.text_muted
                })
                .child(format!("{label} {metric} · {chunks} chunk")),
        )
}

fn history_summary_badge(label: String, color: Rgba, colors: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(7.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.16,
            ..colors.surface_hover
        })
        .child(
            div()
                .w(px(8.0))
                .h(px(8.0))
                .rounded_full()
                .bg(color.alpha(0.58)),
        )
        .child(div().text_color(colors.text_muted).child(label))
}

fn history_detail_text(
    entry: Option<&MapHistoryEntry>,
    error: Option<&SharedString>,
    history: &MapHistoryState,
) -> SharedString {
    if let Some(error) = error {
        return SharedString::from(format!("历史加载错误\n\n{error}"));
    }
    let Some(entry) = entry else {
        return SharedString::from("选择左侧历史项查看详情。");
    };
    let visualization = history.visualization.as_ref();
    let mut lines = Vec::new();
    lines.push(format!("变更集 ID: {}", entry.id));
    lines.push(format!("类型: {}", entry.kind_label()));
    lines.push(format!("状态: {}", entry.short_status()));
    lines.push(format!(
        "时间: {}",
        format_history_time(entry.timestamp_secs)
    ));
    lines.push(format!("标题: {}", entry.label));
    lines.push(format!("说明: {}", entry.message));
    lines.push(String::new());

    lines.push("空间范围".to_string());
    lines.extend(history_dimension_bounds_text(&visualization.chunks));
    lines.push(format!(
        "影响 chunk: {}（混合 {}）",
        visualization.chunks.len(),
        visualization.mixed_chunks
    ));
    lines.push(String::new());

    lines.push("块级差异".to_string());
    lines.push(format!(
        "新增 {} · 删除 {} · 修改 {} · 总计 {} block",
        format_history_count(visualization.added_blocks),
        format_history_count(visualization.removed_blocks),
        format_history_count(visualization.modified_blocks),
        format_history_count(visualization.total_blocks()),
    ));
    lines.push(format!(
        "精确 chunk {} · 部分解析 {} · 仅记录级 {}",
        visualization.precise_chunks,
        visualization.partial_chunks,
        visualization.record_only_chunks
    ));
    lines.push(format!("变化子区块: {}", visualization.changed_subchunks));
    lines.push(String::new());

    lines.push("数据库记录".to_string());
    lines.push(format!(
        "新增 {} · 删除 {} · 修改 {} · 总计 {} record",
        visualization.added_records,
        visualization.removed_records,
        visualization.modified_records,
        visualization.total_records(),
    ));
    lines.push(format!(
        "地形 {} · 方块实体 {} · 实体 {} · 元数据 {} · 未映射 {}",
        visualization.terrain_records,
        visualization.block_entity_records,
        visualization.entity_records,
        visualization.metadata_records,
        visualization.unmapped_records,
    ));
    lines.push(format!(
        "level.dat: {}",
        if visualization.level_dat_changed {
            "有变化"
        } else {
            "无变化"
        }
    ));
    lines.push(String::new());

    lines.push("存储".to_string());
    lines.push(format!("世界: {}", entry.world_path));
    lines.push(format!(
        "原始变化字节: {}",
        format_history_count(entry.raw_delta_bytes)
    ));
    lines.push(format!("存储格式: {}", history_storage_label(entry)));
    lines.push(format!(
        "实际新增存储: {} bytes{}",
        format_history_count(entry.stored_bytes),
        history_compression_ratio(entry)
    ));
    if entry.stored_object_count > 0 || entry.reused_object_count > 0 {
        lines.push(format!(
            "对象库: 新增 {} · 复用 {}",
            entry.stored_object_count, entry.reused_object_count
        ));
    }
    lines.push(format!(
        "当前地图筛选: {}",
        history_filter_text(history.visualization_filter)
    ));
    if let Some(error) = &entry.error {
        lines.push(format!("错误: {error}"));
    }
    SharedString::from(lines.join("\n"))
}

fn history_dimension_bounds_text(chunks: &[MapHistoryChunkVisual]) -> Vec<String> {
    let mut bounds = BTreeMap::<Dimension, (i32, i32, i32, i32, usize)>::new();
    for chunk in chunks {
        let entry = bounds.entry(chunk.pos.dimension).or_insert((
            chunk.pos.x,
            chunk.pos.z,
            chunk.pos.x,
            chunk.pos.z,
            0,
        ));
        entry.0 = entry.0.min(chunk.pos.x);
        entry.1 = entry.1.min(chunk.pos.z);
        entry.2 = entry.2.max(chunk.pos.x);
        entry.3 = entry.3.max(chunk.pos.z);
        entry.4 = entry.4.saturating_add(1);
    }
    if bounds.is_empty() {
        return vec!["无可映射的 chunk 范围".to_string()];
    }
    bounds
        .into_iter()
        .map(|(dimension, (min_x, min_z, max_x, max_z, count))| {
            format!(
                "{}: chunk ({min_x},{min_z}) → ({max_x},{max_z}) · block X {}..{} · Z {}..{} · {count} chunk",
                history_dimension_label(dimension),
                min_x.saturating_mul(16),
                max_x.saturating_add(1).saturating_mul(16).saturating_sub(1),
                min_z.saturating_mul(16),
                max_z.saturating_add(1).saturating_mul(16).saturating_sub(1),
            )
        })
        .collect()
}

fn history_dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "主世界".to_string(),
        Dimension::Nether => "下界".to_string(),
        Dimension::End => "末地".to_string(),
        Dimension::Unknown(id) => format!("维度 {id}"),
    }
}

fn history_filter_text(filter: MapHistoryVisualFilter) -> &'static str {
    match (filter.show_added, filter.show_removed, filter.show_modified) {
        (true, true, true) => "新增、删除、修改",
        (true, true, false) => "新增、删除",
        (true, false, true) => "新增、修改",
        (false, true, true) => "删除、修改",
        (true, false, false) => "仅新增",
        (false, true, false) => "仅删除",
        (false, false, true) => "仅修改",
        (false, false, false) => "全部隐藏",
    }
}

fn history_compression_ratio(entry: &MapHistoryEntry) -> String {
    if entry.raw_delta_bytes == 0 {
        return String::new();
    }
    let ratio = entry.stored_bytes as f64 / entry.raw_delta_bytes as f64 * 100.0;
    format!("（{ratio:.1}%）")
}

fn format_history_count(value: u64) -> String {
    let raw = value.to_string();
    let mut output = String::with_capacity(raw.len() + raw.len() / 3);
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    output
}

fn history_storage_label(entry: &MapHistoryEntry) -> &'static str {
    match entry.storage_format.as_str() {
        "objectStoreV1" => "对象库 delta",
        "inlineZstd" => "内联压缩 delta",
        _ => "未知",
    }
}

fn format_history_time(timestamp_secs: u64) -> String {
    let Some(utc) = chrono::DateTime::<chrono::Utc>::from_timestamp(
        i64::try_from(timestamp_secs).unwrap_or(0),
        0,
    ) else {
        return "-".to_string();
    };
    let datetime = utc.with_timezone(&chrono::Local);
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

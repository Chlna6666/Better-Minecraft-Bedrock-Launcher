use super::model::{
    ChunkTransferTaskProgressSync, View, is_map_operation_cancelled_error,
    sync_map_operation_task_progress, task_progress_units,
};
use super::panels::toolbar_button;
use super::prelude::*;
use super::tile_state::TilePriority;
use crate::tasks::TaskStage;

impl View {
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
            t!("MapViewer.history.history_panel.undo_operation_label"),
            TransferPhase::UNDO_HISTORY,
            |world_path, cancel, progress| apply_undo(&world_path, &cancel, progress),
            cx,
        );
    }

    pub(super) fn redo_map_edit(&mut self, cx: &mut Context<Self>) {
        self.apply_history_operation(
            t!("MapViewer.history.history_panel.redo_operation_label"),
            TransferPhase::REDO_HISTORY,
            |world_path, cancel, progress| apply_redo(&world_path, &cancel, progress),
            cx,
        );
    }

    pub(super) fn restore_selected_history_entry(&mut self, cx: &mut Context<Self>) {
        let Some(entry_id) = self.history.selected_entry_id.clone() else {
            toast::error(cx, t!("MapViewer.history.history_panel.select_entry_first"));
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
            t!("MapViewer.history.history_panel.restore_operation_label"),
            TransferPhase::RESTORE_HISTORY,
            move |world_path, cancel: CancelFlag, progress| {
                if cancel.is_cancelled() {
                    return Err(
                        t!("MapViewer.history.history_panel.operation_cancelled").to_string()
                    );
                }
                if !selected_chunks.is_empty() {
                    create_restore_protection_point(
                        &world_path,
                        selected_chunks.clone(),
                        t!("MapViewer.history.history_panel.restore_protection_point_label"),
                    )?;
                }
                restore_history_entry(&world_path, &entry_id, &cancel, progress)
            },
            cx,
        );
    }

    pub(super) fn create_map_backup(&mut self, cx: &mut Context<Self>) {
        let world_path = self.world_path.clone();
        let map_name = self.asset.display_name.to_string();
        self.begin_edit_toast(
            t!("MapViewer.history.history_panel.creating_full_map_backup"),
            cx,
        );
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
                        let message = t!(
                            "MapViewer.history.history_panel.map_backup_created",
                            path = path
                        )
                        .to_string();
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
        if self.map_write_busy() {
            toast::error(
                cx,
                t!("MapViewer.history.history_panel.clear_history_map_busy"),
            );
            return;
        }
        let world_path = self.world_path.clone();
        self.history.loading = true;
        cx.notify();
        cx.spawn(async move |handle, cx| {
            let result = cx
                .background_spawn(async move {
                    let history_dir = history_dir(&world_path);
                    match std::fs::remove_dir_all(&history_dir) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(t!(
                            "MapViewer.history.history_panel.clear_history_failed",
                            error = error
                        )
                        .to_string()),
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
                        toast::success(cx, t!("MapViewer.history.history_panel.history_cleared"));
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
                                            t!("MapViewer.history.history_panel.loading_history")
                                        } else {
                                            t!("MapViewer.history.history_panel.history_empty")
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
                t!("MapViewer.history.history_panel.refresh"),
                !self.history.loading,
                cx,
                |this, cx| {
                    this.refresh_history(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                t!("MapViewer.history.history_panel.undo"),
                !self.history.applying && has_undo,
                cx,
                |this, cx| {
                    this.undo_map_edit(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                t!("MapViewer.history.history_panel.redo"),
                !self.history.applying && has_redo,
                cx,
                |this, cx| {
                    this.redo_map_edit(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                t!("MapViewer.history.history_panel.restore_to_here"),
                !self.history.applying && can_restore,
                cx,
                |this, cx| {
                    this.restore_selected_history_entry(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                t!("MapViewer.history.history_panel.backup"),
                !self.history.applying,
                cx,
                |this, cx| {
                    this.create_map_backup(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                if self.history.visualization_enabled {
                    t!("MapViewer.history.history_panel.hide_diff")
                } else {
                    t!("MapViewer.history.history_panel.show_diff")
                },
                selected.is_some(),
                cx,
                |this, cx| {
                    this.toggle_history_visualization(cx);
                },
            ))
            .child(history_toolbar_action(
                colors,
                t!("MapViewer.history.history_panel.clear"),
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
                            .child(t!(
                                "MapViewer.history.history_panel.entry_meta_line",
                                time = format_history_time(entry.timestamp_secs),
                                chunks = entry.chunks.len(),
                                raw = entry.raw_delta_count,
                                rawBytes = entry.raw_delta_bytes,
                                storedBytes = entry.stored_bytes
                            )),
                    ),
            )
    }

    fn apply_history_operation(
        &mut self,
        label: SharedString,
        phase: TransferPhase,
        operation: impl FnOnce(
            PathBuf,
            CancelFlag,
            Box<dyn FnMut(MapHistoryApplyProgress) + Send>,
        ) -> Result<MapHistoryApplyOutcome, String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.history.applying {
            toast::error(
                cx,
                t!("MapViewer.history.history_panel.operation_already_running"),
            );
            return;
        }
        if self.map_write_busy() {
            toast::error(cx, t!("MapViewer.history.history_panel.map_busy_wait"));
            return;
        }
        self.history.applying = true;
        let task_id = task_manager::create_task(
            task_manager::TaskOptions::new(label.clone(), TaskStage::HISTORY_OPERATION)
                .detail(self.asset.display_name.to_string())
                .total(task_progress_units(1)),
        )
        .expect("task id must be unique");
        task_manager::register_cooperative_cancel(task_id.clone());
        let cancel = CancelFlag::new();
        task_manager::register_cancel_hook(task_id.clone(), {
            let cancel = cancel.clone();
            move || cancel.cancel()
        });
        self.set_chunk_transfer_progress(ChunkTransferProgress {
            phase,
            completed: 0,
            total: 1,
        });
        self.begin_edit_toast(
            t!(
                "MapViewer.history.history_panel.operation_in_progress",
                label = label
            ),
            cx,
        );
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
                let mut progress_sync = ChunkTransferTaskProgressSync::default();
                let task_id_for_progress = task_id.clone();
                let progress = Box::new(move |progress: MapHistoryApplyProgress| {
                    let snapshot = ChunkTransferProgress {
                        phase: progress.phase,
                        completed: progress.completed,
                        total: progress.total,
                    };
                    sync_map_operation_task_progress(
                        &task_id_for_progress,
                        &mut progress_sync,
                        &snapshot,
                        TaskStage::HISTORY_OPERATION,
                    );
                    if progress_sender
                        .unbounded_send(HistoryApplyEvent::Progress(snapshot))
                        .is_err()
                    {
                        tracing::debug!("history operation progress receiver dropped");
                    }
                });
                let result = operation(world_path_for_task, cancel.clone(), progress);
                let status = match &result {
                    Ok(_) => "completed",
                    Err(error) if is_map_operation_cancelled_error(error) => "cancelled",
                    Err(_) => "error",
                };
                task_manager::finish_task(&task_id, status, result.as_ref().err().cloned());
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
                                        let invalidation = EditInvalidation::chunks(
                                            outcome.affected_chunks.clone(),
                                        )
                                        .with_metadata();
                                        this.apply_map_edit_invalidation(
                                            &invalidation,
                                            TilePriority::EditRefresh,
                                            false,
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
    cx: &mut Context<View>,
    action: impl Fn(&mut View, &mut Context<View>) + 'static,
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
    cx: &mut Context<View>,
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
            t!("MapViewer.history.history_panel.filter_added"),
            MapHistoryVisualFilterKind::Added,
            visualization,
            history.visualization_filter,
            rgb(0x3b82f6),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            t!("MapViewer.history.history_panel.filter_removed"),
            MapHistoryVisualFilterKind::Removed,
            visualization,
            history.visualization_filter,
            rgb(0xef4444),
            colors,
            cx,
        ))
        .child(history_filter_chip(
            t!("MapViewer.history.history_panel.filter_modified"),
            MapHistoryVisualFilterKind::Modified,
            visualization,
            history.visualization_filter,
            rgb(0x8b5cf6),
            colors,
            cx,
        ))
        .child(history_summary_badge(
            t!(
                "MapViewer.history.history_panel.mixed_chunks",
                count = visualization.mixed_chunks
            ),
            rgb(0xf59e0b),
            colors,
        ))
        .child(history_summary_badge(
            t!(
                "MapViewer.history.history_panel.precision_summary",
                precise = visualization.precise_chunks,
                partial = visualization.partial_chunks,
                recordOnly = visualization.record_only_chunks
            ),
            rgb(0x10b981),
            colors,
        ))
        .when(history.visualization_loading, |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.history.history_panel.parsing_block_diff")),
            )
        })
        .when(!history.visualization_enabled, |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.history.history_panel.visualization_hidden")),
            )
        })
        .when(!history.visualization_filter.any_enabled(), |this| {
            this.child(
                div()
                    .text_color(colors.text_muted)
                    .child(t!("MapViewer.history.history_panel.all_filters_disabled")),
            )
        })
        .when_some(history.visualization_error.clone(), |this, error| {
            this.child(div().text_color(colors.danger).child(error))
        })
}

fn history_filter_chip(
    label: SharedString,
    kind: MapHistoryVisualFilterKind,
    visualization: &MapHistoryVisualization,
    filter: MapHistoryVisualFilter,
    color: Rgba,
    colors: &ThemeColors,
    cx: &mut Context<View>,
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

fn history_summary_badge(label: SharedString, color: Rgba, colors: &ThemeColors) -> Div {
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
        return SharedString::from(format!(
            "{}\n\n{error}",
            t!("MapViewer.history.history_panel.load_error")
        ));
    }
    let Some(entry) = entry else {
        return t!("MapViewer.history.history_panel.select_entry_hint");
    };
    let visualization = history.visualization.as_ref();
    let mut lines = Vec::new();
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_changeset_id",
            id = entry.id
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_kind",
            value = entry.kind_label()
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_status",
            value = entry.short_status()
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_time",
            value = format_history_time(entry.timestamp_secs)
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_title",
            value = entry.label
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_message",
            value = entry.message
        )
        .to_string(),
    );
    lines.push(String::new());

    lines.push(t!("MapViewer.history.history_panel.section_spatial_bounds").to_string());
    lines.extend(history_dimension_bounds_text(&visualization.chunks));
    lines.push(
        t!(
            "MapViewer.history.history_panel.affected_chunks",
            chunks = visualization.chunks.len(),
            mixed = visualization.mixed_chunks
        )
        .to_string(),
    );
    lines.push(String::new());

    lines.push(t!("MapViewer.history.history_panel.section_block_diff").to_string());
    lines.push(
        t!(
            "MapViewer.history.history_panel.block_diff_summary",
            added = format_history_count(visualization.added_blocks),
            removed = format_history_count(visualization.removed_blocks),
            modified = format_history_count(visualization.modified_blocks),
            total = format_history_count(visualization.total_blocks())
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_precision",
            precise = visualization.precise_chunks,
            partial = visualization.partial_chunks,
            recordOnly = visualization.record_only_chunks
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.changed_subchunks",
            count = visualization.changed_subchunks
        )
        .to_string(),
    );
    lines.push(String::new());

    lines.push(t!("MapViewer.history.history_panel.section_db_records").to_string());
    lines.push(
        t!(
            "MapViewer.history.history_panel.record_diff_summary",
            added = visualization.added_records,
            removed = visualization.removed_records,
            modified = visualization.modified_records,
            total = visualization.total_records()
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.record_categories",
            terrain = visualization.terrain_records,
            blockEntity = visualization.block_entity_records,
            entity = visualization.entity_records,
            metadata = visualization.metadata_records,
            unmapped = visualization.unmapped_records
        )
        .to_string(),
    );
    lines.push(format!(
        "level.dat: {}",
        if visualization.level_dat_changed {
            t!("MapViewer.history.history_panel.level_dat_changed")
        } else {
            t!("MapViewer.history.history_panel.level_dat_unchanged")
        }
    ));
    lines.push(String::new());

    lines.push(t!("MapViewer.history.history_panel.section_storage").to_string());
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_world",
            path = entry.world_path
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_raw_delta_bytes",
            bytes = format_history_count(entry.raw_delta_bytes)
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_storage_format",
            value = history_storage_label(entry)
        )
        .to_string(),
    );
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_stored_bytes",
            bytes = format_history_count(entry.stored_bytes),
            ratio = history_compression_ratio(entry)
        )
        .to_string(),
    );
    if entry.stored_object_count > 0 || entry.reused_object_count > 0 {
        lines.push(
            t!(
                "MapViewer.history.history_panel.object_store_counts",
                stored = entry.stored_object_count,
                reused = entry.reused_object_count
            )
            .to_string(),
        );
    }
    lines.push(
        t!(
            "MapViewer.history.history_panel.detail_filter",
            value = history_filter_text(history.visualization_filter)
        )
        .to_string(),
    );
    if let Some(error) = &entry.error {
        lines.push(
            t!(
                "MapViewer.history.history_panel.detail_error",
                error = error
            )
            .to_string(),
        );
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
        return vec![t!("MapViewer.history.history_panel.no_mappable_chunks").to_string()];
    }
    bounds
        .into_iter()
        .map(|(dimension, (min_x, min_z, max_x, max_z, count))| {
            t!(
                "MapViewer.history.history_panel.dimension_bounds",
                dimension = history_dimension_label(dimension),
                minX = min_x,
                minZ = min_z,
                maxX = max_x,
                maxZ = max_z,
                blockXMin = min_x.saturating_mul(16),
                blockXMax = max_x.saturating_add(1).saturating_mul(16).saturating_sub(1),
                blockZMin = min_z.saturating_mul(16),
                blockZMax = max_z.saturating_add(1).saturating_mul(16).saturating_sub(1),
                count = count
            )
            .to_string()
        })
        .collect()
}

fn history_dimension_label(dimension: Dimension) -> SharedString {
    match dimension {
        Dimension::Overworld => t!("MapViewer.history.history_panel.dimension_overworld"),
        Dimension::Nether => t!("MapViewer.history.history_panel.dimension_nether"),
        Dimension::End => t!("MapViewer.history.history_panel.dimension_end"),
        Dimension::Unknown(id) => {
            t!("MapViewer.history.history_panel.dimension_unknown", id = id)
        }
    }
}

fn history_filter_text(filter: MapHistoryVisualFilter) -> SharedString {
    match (filter.show_added, filter.show_removed, filter.show_modified) {
        (true, true, true) => t!("MapViewer.history.history_panel.filter_all"),
        (true, true, false) => t!("MapViewer.history.history_panel.filter_added_removed"),
        (true, false, true) => t!("MapViewer.history.history_panel.filter_added_modified"),
        (false, true, true) => t!("MapViewer.history.history_panel.filter_removed_modified"),
        (true, false, false) => t!("MapViewer.history.history_panel.filter_only_added"),
        (false, true, false) => t!("MapViewer.history.history_panel.filter_only_removed"),
        (false, false, true) => t!("MapViewer.history.history_panel.filter_only_modified"),
        (false, false, false) => t!("MapViewer.history.history_panel.filter_none"),
    }
}

fn history_compression_ratio(entry: &MapHistoryEntry) -> String {
    if entry.raw_delta_bytes == 0 {
        return String::new();
    }
    let ratio = entry.stored_bytes as f64 / entry.raw_delta_bytes as f64 * 100.0;
    t!(
        "MapViewer.history.history_panel.compression_ratio_suffix",
        ratio = format!("{ratio:.1}")
    )
    .to_string()
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

fn history_storage_label(entry: &MapHistoryEntry) -> SharedString {
    match entry.storage_format.as_str() {
        "objectStoreV1" => t!("MapViewer.history.history_panel.storage_object_store_delta"),
        "inlineZstd" => t!("MapViewer.history.history_panel.storage_inline_zstd_delta"),
        _ => t!("MapViewer.history.history_panel.storage_unknown"),
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

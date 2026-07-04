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
                            .rounded(px(6.0))
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
                    .rounded(px(6.0))
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
                    .child(history_detail_text(selected, self.history.error.as_ref())),
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

fn history_detail_text(
    entry: Option<&MapHistoryEntry>,
    error: Option<&SharedString>,
) -> SharedString {
    if let Some(error) = error {
        return SharedString::from(format!("历史加载错误\n\n{error}"));
    }
    let Some(entry) = entry else {
        return SharedString::from("选择左侧历史项查看详情。");
    };
    let mut lines = Vec::new();
    lines.push(format!("类型: {}", entry.kind_label()));
    lines.push(format!("状态: {}", entry.short_status()));
    lines.push(format!(
        "时间: {}",
        format_history_time(entry.timestamp_secs)
    ));
    lines.push(format!("说明: {}", entry.message));
    lines.push(format!("世界: {}", entry.world_path));
    lines.push(format!("影响 chunk: {}", entry.chunks.len()));
    if !entry.chunks.is_empty() {
        lines.push(format!(
            "chunk: {}",
            entry
                .chunks
                .iter()
                .take(12)
                .map(|chunk| format!("{}:{},{}", chunk.dimension.id(), chunk.x, chunk.z))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    lines.push(format!("raw 记录变化: {}", entry.raw_delta_count));
    lines.push(format!("raw 变化字节: {}", entry.raw_delta_bytes));
    lines.push(format!("存储格式: {}", history_storage_label(entry)));
    lines.push(format!("实际新增存储: {} bytes", entry.stored_bytes));
    if entry.stored_object_count > 0 || entry.reused_object_count > 0 {
        lines.push(format!(
            "对象库: 新增 {} · 复用 {}",
            entry.stored_object_count, entry.reused_object_count
        ));
    }
    lines.push(format!(
        "level.dat: {}",
        if entry.level_dat_changed {
            "有变化"
        } else {
            "无变化"
        }
    ));
    if let Some(error) = &entry.error {
        lines.push(format!("错误: {error}"));
    }
    SharedString::from(lines.join("\n"))
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

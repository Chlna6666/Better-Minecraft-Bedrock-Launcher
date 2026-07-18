use super::model::*;
use super::panels::{compact_activity_label, dimension_label, panel_title};
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn set_chunk_transfer_progress(&mut self, progress: ChunkTransferProgress) {
        self.professional.last_chunk_transfer_progress = Some(progress.clone());
        self.professional.last_chunk_transfer_finished_at = None;
        self.professional.chunk_transfer_progress = Some(progress);
    }

    pub(super) fn finish_chunk_transfer_progress(&mut self) {
        if let Some(progress) = self.professional.chunk_transfer_progress.take() {
            self.professional.last_chunk_transfer_progress = Some(progress);
        }
        self.professional.last_chunk_transfer_finished_at = Some(Instant::now());
    }

    pub(super) fn complete_chunk_transfer_progress(&mut self) {
        if let Some(mut progress) = self.professional.chunk_transfer_progress.clone() {
            progress.completed = progress.total;
            self.professional.last_chunk_transfer_progress = Some(progress);
        }
        self.professional.chunk_transfer_progress = None;
        self.professional.last_chunk_transfer_finished_at = Some(Instant::now());
    }

    fn visible_chunk_transfer_progress(&self) -> Option<(&ChunkTransferProgress, bool)> {
        if let Some(progress) = self.professional.chunk_transfer_progress.as_ref() {
            return Some((progress, true));
        }
        let finished_at = self.professional.last_chunk_transfer_finished_at?;
        if finished_at.elapsed() <= CHUNK_TRANSFER_FINISHED_RETENTION {
            return self
                .professional
                .last_chunk_transfer_progress
                .as_ref()
                .map(|progress| (progress, false));
        }
        None
    }

    pub(super) fn spawn_task_updates(&mut self, cx: &mut Context<Self>) {
        let mut updates = task_manager::subscribe_task_updates();
        let task = cx.spawn(async move |handle, cx| {
            loop {
                let first_snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Ok::<(), anyhow::Error>(());
                    }
                };

                let mut batch = HashMap::default();
                if is_map_window_task_snapshot(first_snapshot.as_ref()) {
                    batch.insert(first_snapshot.id.clone(), first_snapshot);
                }
                loop {
                    match updates.try_recv() {
                        Ok(snapshot) => {
                            if is_map_window_task_snapshot(snapshot.as_ref()) {
                                batch.insert(snapshot.id.clone(), snapshot);
                            }
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            return Ok::<(), anyhow::Error>(());
                        }
                    }
                }
                if batch.is_empty() {
                    continue;
                }
                let snapshots = batch.into_values().collect::<Vec<_>>();

                if handle
                    .update(cx, move |this, cx| {
                        this.task_snapshots
                            .retain(|_, snapshot| is_map_window_task_snapshot(snapshot.as_ref()));
                        for snapshot in snapshots {
                            this.task_snapshots.insert(snapshot.id.clone(), snapshot);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return Ok::<(), anyhow::Error>(());
                }
            }
        });
        self.task_updates_task = Some(task);
    }

    pub(super) fn render_map_status_bar(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let visible_local_progress = self.visible_chunk_transfer_progress();
        let local_progress = visible_local_progress.map(|(progress, _active)| progress);
        let local_progress_active =
            visible_local_progress.is_some_and(|(_progress, active)| active);
        let active_tasks = self.active_task_snapshots();
        let finished_tasks = self.finished_task_snapshots(1);
        let task_count = active_tasks.len();
        let has_running_work = local_progress_active || task_count > 0;
        let task_label = match (
            local_progress_active,
            local_progress.is_some(),
            task_count,
            finished_tasks.first(),
        ) {
            (true, _, 0, _) => "打开进度".to_string(),
            (true, _, 1, _) => format!("打开进度 · {}", active_tasks[0].stage),
            (true, _, count, _) => format!("打开进度 · {count} 个任务"),
            (false, true, 0, _) => "最近完成".to_string(),
            (false, true, 1, _) => format!("最近完成 · {}", active_tasks[0].stage),
            (false, true, count, _) => format!("最近完成 · {count} 个任务"),
            (false, false, 0, Some(snapshot)) => format!(
                "{} · {}",
                task_status_label(snapshot.status.as_ref()),
                snapshot.title
            ),
            (false, false, 0, None) => "无后台任务".to_string(),
            (false, false, 1, _) => format!("1 个后台任务 · {}", active_tasks[0].stage),
            (false, false, count, _) => format!("{count} 个后台任务"),
        };
        let primary_active_task = local_progress
            .is_none()
            .then(|| active_tasks.first().cloned())
            .flatten();
        let activity_label = if local_progress_active {
            "运行中".to_string()
        } else if local_progress.is_some() {
            "完成".to_string()
        } else {
            compact_activity_label(self)
        };
        let center = self.viewport.center_block(self.active_layout);
        let hover = self.hover_chunk_pos();

        div()
            .h(px(IDE_STATUS_BAR_HEIGHT))
            .flex_none()
            .border_t_1()
            .border_color(Hsla {
                a: CHROME_HAIRLINE_ALPHA,
                ..colors.border
            })
            .bg(Hsla {
                a: CHROME_SURFACE_ALPHA,
                ..colors.surface
            })
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .text_size(px(11.0))
            .text_color(colors.text_secondary)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(activity_label),
            )
            .child(status_bar_separator(colors))
            .child(div().flex_none().child(format!(
                "{} · Y {} · {:.0}%",
                dimension_label(self.dimension),
                self.y_layer,
                self.viewport.scale * 100.0
            )))
            .child(status_bar_separator(colors))
            .child(div().flex_none().child(format!(
                "Hover chunk {},{} · Center {},{}",
                hover.x,
                hover.z,
                center.0.div_euclid(16),
                center.1.div_euclid(16)
            )))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(colors.text_muted)
                    .child(self.status.clone()),
            )
            .when_some(visible_local_progress, |this, (progress, active)| {
                this.child(local_progress_inline(colors, progress, active))
            })
            .when_some(primary_active_task.as_ref(), |this, snapshot| {
                this.child(task_progress_inline(colors, snapshot))
            })
            .child(
                div()
                    .flex_none()
                    .cursor_pointer()
                    .text_color(if has_running_work {
                        colors.accent
                    } else {
                        colors.text_muted
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.set_bottom_tab(MapViewerBottomTab::ChunkTree, cx);
                        }),
                    )
                    .child(task_label),
            )
    }

    pub(super) fn render_operation_progress_panel(&self, colors: &ThemeColors) -> impl IntoElement {
        let active_tasks = self.active_task_snapshots();
        let finished_tasks = self.finished_task_snapshots(3);
        let width = (self.window_width * 0.28).clamp(300.0, 380.0);

        div()
            .w(px(width))
            .flex_none()
            .min_h(px(0.0))
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
            .p(px(10.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .overflow_y_scrollbar()
            .child(panel_title(colors, "当前进度"))
            .when_some(
                self.visible_chunk_transfer_progress(),
                |this, (progress, active)| {
                    this.child(local_progress_card(colors, progress, active))
                },
            )
            .when(
                self.visible_chunk_transfer_progress().is_none() && active_tasks.is_empty(),
                |this| {
                    this.child(
                        div()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.18,
                                ..colors.border
                            })
                            .p(px(10.0))
                            .text_size(px(12.0))
                            .line_height(px(18.0))
                            .text_color(colors.text_muted)
                            .child("没有正在运行的导入、导出、复制、粘贴或下载任务。"),
                    )
                },
            )
            .children(
                active_tasks
                    .iter()
                    .map(|snapshot| task_progress_card(colors, snapshot).into_any_element()),
            )
            .when(!finished_tasks.is_empty(), |this| {
                this.child(
                    div()
                        .pt(px(4.0))
                        .border_t_1()
                        .border_color(Hsla {
                            a: 0.18,
                            ..colors.border
                        })
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_muted)
                                .child("最近完成"),
                        )
                        .children(finished_tasks.iter().map(|snapshot| {
                            finished_task_row(colors, snapshot).into_any_element()
                        })),
                )
            })
    }

    fn active_task_snapshots(&self) -> Vec<Arc<TaskSnapshot>> {
        let mut snapshots = self
            .task_snapshots
            .values()
            .filter(|snapshot| {
                matches!(
                    snapshot.status.as_ref(),
                    "running" | "paused" | "cancelling"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            right
                .sequence
                .cmp(&left.sequence)
                .then_with(|| right.last_update_unix.cmp(&left.last_update_unix))
                .then_with(|| left.title.cmp(&right.title))
        });
        snapshots
    }

    fn finished_task_snapshots(&self, limit: usize) -> Vec<Arc<TaskSnapshot>> {
        let mut snapshots = self
            .task_snapshots
            .values()
            .filter(|snapshot| {
                matches!(
                    snapshot.status.as_ref(),
                    "completed" | "cancelled" | "error"
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| {
            right
                .last_update_unix
                .cmp(&left.last_update_unix)
                .then_with(|| right.sequence.cmp(&left.sequence))
        });
        snapshots.truncate(limit);
        snapshots
    }
}

fn is_map_window_task_snapshot(snapshot: &TaskSnapshot) -> bool {
    matches!(
        snapshot.stage.as_ref(),
        "打开地图"
            | "地图索引"
            | "探测瓦片"
            | "局部刷新"
            | "地图导出"
            | "地图导入"
            | "复制区块"
            | "粘贴区块"
            | "删除区块"
            | "写入地图"
    )
}

fn local_progress_inline(
    colors: &ThemeColors,
    progress: &ChunkTransferProgress,
    active: bool,
) -> Div {
    let progress_color = if active {
        colors.accent
    } else {
        colors.stat_green_text
    };
    div()
        .w(px(318.0))
        .flex_none()
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        })
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .child(
            div()
                .w(px(104.0))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if active {
                    colors.text_primary
                } else {
                    colors.stat_green_text
                })
                .child(progress.phase.clone()),
        )
        .child(inline_progress_bar(
            colors,
            progress.ratio(),
            progress_color,
        ))
        .child(
            div()
                .w(px(92.0))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(colors.text_secondary)
                .child(local_progress_numbers(progress)),
        )
}

fn task_progress_inline(colors: &ThemeColors, snapshot: &TaskSnapshot) -> Div {
    let ratio = snapshot
        .percent
        .map_or(0.0, |percent| percent as f32 / 100.0);
    let color = task_status_color(colors, snapshot.status.as_ref());
    div()
        .w(px(318.0))
        .flex_none()
        .h(px(22.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        })
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .child(
            div()
                .w(px(104.0))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(snapshot.stage.to_string()),
        )
        .child(inline_progress_bar(colors, ratio, color))
        .child(
            div()
                .w(px(92.0))
                .flex_none()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(colors.text_secondary)
                .child(task_progress_numbers(snapshot)),
        )
}

fn local_progress_card(
    colors: &ThemeColors,
    progress: &ChunkTransferProgress,
    active: bool,
) -> Div {
    let progress_color = if active {
        colors.accent
    } else {
        colors.stat_green_text
    };
    progress_card_shell(colors)
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if active {
                    colors.text_primary
                } else {
                    colors.stat_green_text
                })
                .child(progress.phase.clone()),
        )
        .child(progress_bar(colors, progress.ratio(), progress_color))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(colors.text_muted)
                .child(local_progress_numbers(progress)),
        )
}

fn task_progress_card(colors: &ThemeColors, snapshot: &TaskSnapshot) -> Div {
    let ratio = snapshot.percent.map(|percent| percent as f32 / 100.0);
    progress_card_shell(colors)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(snapshot.title.to_string()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(task_status_color(colors, snapshot.status.as_ref()))
                        .child(task_status_label(snapshot.status.as_ref())),
                ),
        )
        .child(progress_bar(
            colors,
            ratio.unwrap_or(0.0),
            task_status_color(colors, snapshot.status.as_ref()),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(11.0))
                .text_color(colors.text_muted)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(snapshot.detail.as_ref().map_or_else(
                            || snapshot.stage.to_string(),
                            |detail| format!("{} · {}", snapshot.stage, detail),
                        )),
                )
                .child(task_progress_numbers(snapshot)),
        )
}

fn finished_task_row(colors: &ThemeColors, snapshot: &TaskSnapshot) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_size(px(11.0))
        .text_color(colors.text_muted)
        .child(
            div()
                .w(px(50.0))
                .flex_none()
                .text_color(task_status_color(colors, snapshot.status.as_ref()))
                .child(task_status_label(snapshot.status.as_ref())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .child(snapshot.title.to_string()),
        )
}

fn progress_card_shell(colors: &ThemeColors) -> Div {
    div()
        .rounded(px(6.0))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.32,
            ..colors.surface
        })
        .p(px(9.0))
        .flex()
        .flex_col()
        .gap(px(7.0))
}

fn progress_bar(colors: &ThemeColors, ratio: f32, color: Hsla) -> Div {
    div()
        .w_full()
        .h(px(5.0))
        .rounded_full()
        .bg(Hsla {
            a: 0.18,
            ..colors.border
        })
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(ratio.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(color),
        )
}

fn inline_progress_bar(colors: &ThemeColors, ratio: f32, color: Hsla) -> Div {
    div()
        .flex_1()
        .min_w(px(72.0))
        .h(px(6.0))
        .rounded_full()
        .bg(colors.progress_track)
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(ratio.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(color),
        )
}

fn local_progress_numbers(progress: &ChunkTransferProgress) -> String {
    let completed = progress.completed.min(progress.total);
    format!(
        "{completed}/{} · {:.0}%",
        progress.total,
        progress.ratio() * 100.0
    )
}

fn task_progress_numbers(snapshot: &TaskSnapshot) -> String {
    match (snapshot.percent, snapshot.total) {
        (Some(percent), Some(total)) => {
            format!("{percent:.0}% · {}/{}", snapshot.done, total)
        }
        (Some(percent), None) => format!("{percent:.0}%"),
        (None, Some(total)) => format!("{}/{}", snapshot.done, total),
        (None, None) => snapshot.done.to_string(),
    }
}

fn task_status_label(status: &str) -> &'static str {
    match status {
        "running" => "运行中",
        "paused" => "暂停",
        "cancelling" => "取消中",
        "completed" => "完成",
        "cancelled" => "已取消",
        "error" => "错误",
        _ => "任务",
    }
}

fn task_status_color(colors: &ThemeColors, status: &str) -> Hsla {
    match status {
        "error" => colors.danger,
        "paused" | "cancelled" => colors.text_muted,
        "completed" => colors.stat_green_text,
        _ => colors.accent,
    }
}

fn status_bar_separator(colors: &ThemeColors) -> Div {
    div().w(px(1.0)).h(px(16.0)).flex_none().bg(Hsla {
        a: CHROME_HAIRLINE_ALPHA,
        ..colors.border
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn local_progress_numbers_include_count_and_percent() {
        let progress = ChunkTransferProgress {
            phase: SharedString::from("导出图片"),
            completed: 91,
            total: 182,
        };

        assert_eq!(local_progress_numbers(&progress), "91/182 · 50%");
    }

    #[::core::prelude::v1::test]
    fn local_progress_numbers_clamp_completed_count() {
        let progress = ChunkTransferProgress {
            phase: SharedString::from("导出图片"),
            completed: 200,
            total: 182,
        };

        assert_eq!(local_progress_numbers(&progress), "182/182 · 100%");
    }
}

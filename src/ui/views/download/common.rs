use crate::tasks::task_manager::{TaskSnapshot, get_snapshot_arc, subscribe_task_updates};
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};

pub(crate) fn status_card(colors: &ThemeColors, text: &str, accent: Option<Hsla>) -> Div {
    let border = accent.unwrap_or(colors.border);
    let bg = if accent.is_some() {
        Hsla { a: 0.10, ..border }
    } else {
        Hsla {
            a: 0.70,
            ..colors.surface
        }
    };
    let fg = accent.unwrap_or(colors.text_secondary);

    div()
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .p(px(16.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(fg)
                .child(text.to_string()),
        )
}

pub(crate) fn panel_shell(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
        .rounded(px(10.))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.40,
            ..colors.surface
        })
        .overflow_hidden()
}

pub(crate) fn page_shell(content: impl IntoElement, colors: &ThemeColors) -> Div {
    let _ = colors;
    crate::ui::components::page_shell::page_frame(content)
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn format_count(value: f64) -> SharedString {
    if value >= 1_000_000_000.0 {
        SharedString::from(format!("{:.1}B", value / 1_000_000_000.0))
    } else if value >= 1_000_000.0 {
        SharedString::from(format!("{:.1}M", value / 1_000_000.0))
    } else if value >= 1_000.0 {
        SharedString::from(format!("{:.1}K", value / 1_000.0))
    } else {
        SharedString::from(format!("{:.0}", value))
    }
}

pub(crate) fn format_date_ymd(raw: &str) -> SharedString {
    let trimmed = raw.trim();
    if trimmed.len() >= 10 {
        SharedString::from(trimmed[..10].replace('-', "/"))
    } else {
        SharedString::from(trimmed.to_string())
    }
}

pub(crate) fn sanitize_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        let ch = if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        };
        if ch.is_whitespace() {
            if last_space {
                continue;
            }
            out.push(' ');
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

pub(crate) fn truncate_with_ellipsis(text: &str, max_chars: usize) -> SharedString {
    if max_chars == 0 {
        return SharedString::from("");
    }
    let mut it = text.chars();
    let mut buf = String::new();
    for _ in 0..max_chars {
        let Some(ch) = it.next() else {
            return SharedString::from(text.to_string());
        };
        buf.push(ch);
    }

    if it.next().is_none() {
        SharedString::from(text.to_string())
    } else {
        buf.push_str("...");
        SharedString::from(buf)
    }
}

fn is_task_running_or_paused(snapshot: &TaskSnapshot) -> bool {
    matches!(snapshot.status.as_ref(), "running" | "paused")
}

fn check_task_finished(task_id: &str) -> Option<Arc<TaskSnapshot>> {
    get_snapshot_arc(task_id).filter(|snap| !is_task_running_or_paused(snap))
}

pub(crate) async fn wait_task_finished(task_id: &str) -> Result<Arc<TaskSnapshot>, String> {
    if let Some(snapshot) = check_task_finished(task_id) {
        info!("wait_task_finished: already done task_id={task_id} status={}", snapshot.status);
        return Ok(snapshot);
    }

    info!("wait_task_finished: waiting task_id={task_id}");
    let mut receiver = subscribe_task_updates();
    let mut poll_count: u64 = 0;

    loop {
        if let Some(snapshot) = check_task_finished(task_id) {
            info!("wait_task_finished: done via poll task_id={task_id} status={} poll_count={poll_count}", snapshot.status);
            return Ok(snapshot);
        }

        poll_count += 1;
        match tokio::time::timeout(Duration::from_millis(200), receiver.recv()).await {
            Ok(Ok(snapshot)) => {
                if snapshot.id.as_ref() == task_id && !is_task_running_or_paused(&snapshot) {
                    info!("wait_task_finished: done via broadcast task_id={task_id} status={} poll_count={poll_count}", snapshot.status);
                    return Ok(snapshot);
                }
            }
            Ok(Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) => {
                if let Some(snapshot) = check_task_finished(task_id) {
                    info!("wait_task_finished: done via closed channel check task_id={task_id} status={}", snapshot.status);
                    return Ok(snapshot);
                }
                warn!("wait_task_finished: channel closed task_id={task_id}");
                return Err("任务管理器已关闭".to_string());
            }
            Err(_elapsed) => {}
        }
    }
}

use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

pub(crate) fn status_card(colors: &ThemeColors, text: &str, accent: Option<Hsla>) -> Div {
    let fg = accent.unwrap_or(colors.text_secondary);
    let mut card = crate::ui::components::page_shell::glass_card(colors);
    if let Some(accent) = accent {
        card = card
            .border_color(Hsla { a: 0.30, ..accent })
            .bg(Hsla { a: 0.10, ..accent });
    }

    card.w_full().p(px(16.)).child(
        div()
            .text_size(px(13.))
            .text_color(fg)
            .child(text.to_string()),
    )
}

pub(crate) fn panel_shell(colors: &ThemeColors) -> Div {
    crate::ui::components::page_shell::inner_well(colors)
        .flex_1()
        .min_w(px(0.))
        .min_h(px(0.))
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

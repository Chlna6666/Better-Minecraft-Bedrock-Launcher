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

fn compact_result_tag(colors: &ThemeColors, label: SharedString, accent: bool) -> Div {
    let (background, border, text) = if accent {
        (
            Hsla {
                a: 0.08,
                ..colors.accent
            },
            Hsla {
                a: 0.14,
                ..colors.accent
            },
            colors.accent,
        )
    } else {
        (
            Hsla {
                a: 0.045,
                ..colors.text_primary
            },
            Hsla {
                a: 0.07,
                ..colors.border
            },
            colors.text_muted,
        )
    };

    div()
        .flex_none()
        .max_w(px(132.))
        .px(px(7.))
        .py(px(1.))
        .rounded_full()
        .bg(background)
        .border_1()
        .border_color(border)
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(text)
        .overflow_hidden()
        .text_ellipsis()
        .child(label)
}

pub(super) fn compact_result_header(
    colors: &ThemeColors,
    count_label: SharedString,
    source_label: SharedString,
    detail_label: Option<SharedString>,
) -> Div {
    let tags = div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.))
        .ml_auto()
        .child(compact_result_tag(colors, source_label, true))
        .when_some(detail_label, |tags, detail| {
            tags.child(compact_result_tag(colors, detail, false))
        });

    div()
        .w_full()
        .flex_none()
        .min_h(px(26.))
        .px(px(14.))
        .py(px(2.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.06,
            ..colors.border
        })
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .items_center()
                .gap(px(7.))
                .child(
                    div()
                        .flex_none()
                        .w(px(6.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(colors.accent),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_secondary)
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(count_label),
                ),
        )
        .child(tags)
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

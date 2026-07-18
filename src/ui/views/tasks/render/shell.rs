use super::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskVisualKind {
    Download,
    Install,
    Extract,
}

pub(crate) fn task_text_main(colors: &ThemeColors) -> Hsla {
    colors.text_primary
}

pub(crate) fn task_text_secondary(colors: &ThemeColors) -> Hsla {
    colors.text_secondary
}

pub(crate) fn task_text_tertiary(colors: &ThemeColors) -> Hsla {
    colors.text_muted
}

pub(crate) fn task_border_color(colors: &ThemeColors) -> Hsla {
    colors.border
}

pub(crate) fn task_card_bg(colors: &ThemeColors) -> Hsla {
    colors.surface
}

pub(crate) fn task_card_hover_bg(colors: &ThemeColors) -> Hsla {
    colors.surface_hover
}

pub(crate) fn task_warning_color(colors: &ThemeColors) -> Hsla {
    colors.danger
}

pub(crate) fn task_visual_kind(stage: &str, status: &str) -> TaskVisualKind {
    if status == "completed" {
        return TaskVisualKind::Extract;
    }

    let stage = stage.to_lowercase();
    if stage.contains("下载") || stage.contains("解析") || stage.contains("读取") {
        TaskVisualKind::Download
    } else if stage.contains("解压")
        || stage.contains("解密")
        || stage.contains("安装")
        || stage.contains("准备")
    {
        TaskVisualKind::Extract
    } else if stage.contains("处理") || stage.contains("整理") || stage.contains("校验") {
        TaskVisualKind::Install
    } else {
        TaskVisualKind::Download
    }
}

pub(crate) fn task_visual_icon(kind: TaskVisualKind) -> &'static str {
    match kind {
        TaskVisualKind::Download => lucide_icons::icon_download(),
        TaskVisualKind::Install => lucide_icons::icon_package(),
        TaskVisualKind::Extract => lucide_icons::icon_box(),
    }
}

pub(crate) fn task_visual_accent(kind: TaskVisualKind, colors: &ThemeColors) -> Hsla {
    match kind {
        TaskVisualKind::Download => colors.accent,
        TaskVisualKind::Install => colors.stat_orange_text,
        TaskVisualKind::Extract => colors.stat_green_text,
    }
}

pub(crate) fn task_status_accent(status: &str, kind: TaskVisualKind, colors: &ThemeColors) -> Hsla {
    match status {
        "paused" => colors.stat_orange_text,
        "cancelling" | "error" => colors.danger,
        "completed" => colors.stat_green_text,
        _ => task_visual_accent(kind, colors),
    }
}

pub(crate) fn page_shell(content: impl IntoElement, colors: &ThemeColors) -> Div {
    crate::ui::components::page_shell::page_frame(
        div()
            .size_full()
            .rounded(px(12.))
            .border_1()
            .border_color(Hsla {
                a: 0.16,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.78,
                ..colors.settings_panel_bg
            })
            .overflow_hidden()
            .child(content),
    )
}

pub(crate) fn task_icon_button(
    id: impl Into<ElementId>,
    icon_path: &'static str,
    danger: bool,
    enabled: bool,
    colors: &ThemeColors,
) -> Stateful<Div> {
    let mut button = div()
        .id(id)
        .w(px(32.))
        .h(px(32.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(Hsla {
            a: 0.10,
            ..colors.border
        })
        .bg(colors.surface)
        .child(themed_icon(
            icon_path,
            16.0,
            if danger {
                colors.danger
            } else {
                colors.text_secondary
            },
        ));

    if enabled {
        button = button
            .cursor_pointer()
            .hover(|this| this.bg(colors.surface_hover));
    } else {
        button = button.opacity(0.45);
    }

    button
}

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

    if matches!(
        stage,
        "downloading"
            | "resolving_url"
            | "reading_body"
            | "parsing"
            | "url_resolved"
            | "resolving_runner"
            | "resolving_proton_gdk"
    ) {
        TaskVisualKind::Download
    } else if matches!(
        stage,
        "extracting"
            | "decrypting"
            | "preparing_files"
            | "preparing_prefix"
            | "installing_linux_packages"
            | "extracting_proton_gdk"
    ) {
        TaskVisualKind::Extract
    } else if matches!(stage, "merging" | "verifying" | "renaming" | "patching") {
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
        crate::ui::components::page_shell::page_panel(colors)
            .size_full()
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
        .rounded(px(crate::ui::theme::tokens::radius::XS))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
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
            .hover(|this| this.bg(colors.surface_hover))
            .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE));
    } else {
        button = button.opacity(0.45);
    }

    button
}

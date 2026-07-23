use crate::ui::components::modal;
use crate::ui::hooks::use_launcher::{
    LauncherSnapshot, cancel_launcher, close_launcher, copy_launcher_error, minimize_launcher,
    retry_launcher,
};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

fn launch_icon_path(name: &str) -> &'static str {
    if name.contains("EducationPreview") {
        "images/minecraft/EducationEditionPreview.png"
    } else if name.contains("Education") {
        "images/minecraft/EducationEdition.png"
    } else if name.contains("Preview") || name.contains("Beta") {
        "images/minecraft/Preview.png"
    } else {
        "images/minecraft/Release.png"
    }
}

pub fn render_launcher_overlay(
    snapshot: &LauncherSnapshot,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if !snapshot.show_modal {
        return div().into_any_element();
    }

    let factor = snapshot.modal_factor.clamp(0.0, 1.0);
    let smooth = (factor * factor * (3.0 - 2.0 * factor)).clamp(0.0, 1.0);
    let now = std::time::Instant::now();
    let theme_state = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme_state.factor(now),
        theme_state.accent,
    );
    let icon_path = launch_icon_path(snapshot.version_name.as_ref());
    let percent = snapshot
        .last_snapshot
        .as_ref()
        .and_then(|value| value.percent)
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let status = snapshot
        .last_snapshot
        .as_ref()
        .map(|value| value.status.to_string())
        .unwrap_or_else(|| "running".to_string());
    let stage = snapshot
        .last_snapshot
        .as_ref()
        .map(|value| value.stage.to_string())
        .unwrap_or_else(|| "准备中".to_string());
    let is_error = status == "error";
    let is_completed = status == "completed";
    let is_cancelled = status == "cancelled";
    let is_terminal = matches!(status.as_str(), "completed" | "cancelled" | "error");
    let button_label = if is_error {
        "重试"
    } else if is_terminal {
        "关闭"
    } else {
        "取消"
    };
    let action_fg = if is_error {
        rgb(0x92400e)
    } else if is_terminal {
        rgb(0x334155)
    } else {
        rgb(0x475569)
    };
    let progress_fill = if status == "error" {
        rgb(0xef4444)
    } else if status == "completed" {
        rgb(0x22c55e)
    } else {
        rgb(0x4f46e5)
    };
    let status_text = snapshot
        .last_snapshot
        .as_ref()
        .filter(|value| matches!(value.status.as_ref(), "completed" | "cancelled" | "error"))
        .and_then(|value| value.message.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            snapshot
                .logs
                .last()
                .map(|line| line.as_ref().to_string())
                .filter(|line| !line.is_empty())
        })
        .unwrap_or_else(|| stage.clone());
    let status_color = line_color(&status_text);
    let visible_logs = snapshot
        .logs
        .iter()
        .rev()
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let log_scroll_handle = cx
        .global::<crate::ui::state::launcher::LauncherState>()
        .log_scroll_handle
        .clone();
    let scroll_to_bottom_handle = log_scroll_handle.clone();
    let last_log_scroll_version =
        window.use_keyed_state("launcher-log-scroll-version", cx, |_, _| 0_u64);
    if !snapshot.logs.is_empty() {
        let previous_log_scroll_version = *last_log_scroll_version.read(cx);
        let should_scroll = previous_log_scroll_version != snapshot.log_version;
        if should_scroll {
            last_log_scroll_version.update(cx, |version, _cx| {
                *version = snapshot.log_version;
            });
            window.on_next_frame(move |_window, _cx| {
                let max_offset = scroll_to_bottom_handle.max_offset().height;
                scroll_to_bottom_handle.set_offset(point(px(0.), -max_offset));
            });
        }
    }

    let card_radius = px(18.);
    let card_width = px(640.0);
    let card_height = px(392.0);

    let card = div()
        .w(card_width)
        .h(card_height)
        .max_w(relative(1.0))
        .max_h(relative(1.0))
        .rounded(card_radius)
        .shadow(vec![
            BoxShadow {
                color: hsla(0., 0., 0., 0.10 * smooth),
                blur_radius: px(40.),
                spread_radius: px(0.),
                offset: point(px(0.), px(20.)),
            },
            BoxShadow {
                color: hsla(0., 0., 0., 0.05 * smooth),
                blur_radius: px(0.),
                spread_radius: px(1.),
                offset: point(px(0.), px(0.)),
            },
        ])
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .rounded(card_radius)
                .occlude()
                .overflow_hidden()
                .bg(rgb(0xffffff))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(28.))
                        .py(px(16.))
                        .border_b_1()
                        .border_color(hsla(0., 0., 0., 0.05))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(14.))
                                .child(
                                    img(icon_path)
                                        .w(px(44.))
                                        .h(px(44.))
                                        .rounded(px(8.))
                                        .object_fit(ObjectFit::Cover),
                                )
                                .child(
                                    div()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_size(px(16.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(0x1e293b))
                                                .truncate()
                                                .child(format!(
                                                    "正在启动 {}",
                                                    snapshot.version_folder
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(0x64748b))
                                                .truncate()
                                                .child(format!(
                                                    "版本: {} | 构建类型: {}",
                                                    snapshot.version, snapshot.kind
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(0x64748b))
                                                .truncate()
                                                .child(format!(
                                                    "BLoader.dll: {}",
                                                    snapshot.loader_version
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|style| style.bg(rgb(0xf1f5f9)))
                                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                                            let _ = copy_launcher_error(cx);
                                            cx.stop_propagation();
                                        })
                                        .child(
                                            svg()
                                                .path(lucide_icons::icon_copy())
                                                .size(px(16.))
                                                .text_color(rgb(0x64748b)),
                                        ),
                                )
                                .child(
                                    div()
                                        .w(px(32.))
                                        .h(px(32.))
                                        .rounded(px(7.))
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(|style| style.bg(rgb(0xf1f5f9)))
                                        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                                            minimize_launcher(cx);
                                            cx.stop_propagation();
                                        })
                                        .child(
                                            svg()
                                                .path(lucide_icons::icon_minus())
                                                .size(px(16.))
                                                .text_color(rgb(0x64748b)),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .px(px(28.))
                        .pt(px(12.))
                        .pb(px(10.))
                        .bg(rgb(0xf8fafc))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("launcher-log-scroll")
                                .flex_1()
                                .min_h(px(0.))
                                .rounded(px(10.))
                                .border_1()
                                .border_color(hsla(0., 0., 0., 0.05))
                                .bg(rgb(0xffffff))
                                .overflow_y_scroll()
                                .scrollbar_width(px(0.))
                                .track_scroll(&log_scroll_handle)
                                .overflow_x_hidden()
                                .px(px(12.))
                                .py(px(10.))
                                .flex()
                                .flex_col()
                                .justify_start()
                                .gap(px(4.))
                                .children(visible_logs.into_iter().map(|line| {
                                    let line_ref = line.as_ref();
                                    let line_color = line_color(line_ref);
                                    let accent_color = if is_error_line(line_ref) {
                                        rgb(0xef4444)
                                    } else if is_warning_line(line_ref) {
                                        rgb(0xf59e0b)
                                    } else if is_success_line(line_ref) {
                                        rgb(0x22c55e)
                                    } else {
                                        rgb(0xe2e8f0)
                                    };
                                    div()
                                        .pl(px(8.))
                                        .border_l_2()
                                        .border_color(Hsla {
                                            a: 1.0,
                                            ..accent_color.into()
                                        })
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .text_color(line_color)
                                        .whitespace_normal()
                                        .child(line_ref.to_string())
                                })),
                        ),
                )
                .child(
                    div()
                        .px(px(28.))
                        .py(px(12.))
                        .bg(rgb(0xffffff))
                        .rounded_b(card_radius)
                        .when(!is_error, |this| {
                            this.flex()
                                .flex_col()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(12.))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .flex_1()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_color(status_color)
                                                .child(status_text.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(12.))
                                                .child(
                                                    div()
                                                        .text_size(px(12.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(0x64748b))
                                                        .child(format!("{percent:.0}%")),
                                                )
                                                .child(
                                                    div()
                                                        .w(px(220.))
                                                        .h(px(6.))
                                                        .rounded(px(10.))
                                                        .bg(rgb(0xf1f5f9))
                                                        .overflow_hidden()
                                                        .child(
                                                            div()
                                                                .h_full()
                                                                .w(relative(
                                                                    (percent as f32 / 100.0)
                                                                        .clamp(0.0, 1.0),
                                                                ))
                                                                .bg(progress_fill)
                                                                .rounded(px(10.)),
                                                        ),
                                                ),
                                        ),
                                )
                                .child(
                                    div().flex().justify_end().child(
                                        action_button(
                                            button_label,
                                            None,
                                            is_terminal,
                                            action_fg.into(),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            move |_ev, _window, cx| {
                                                if is_completed || is_cancelled {
                                                    close_launcher(cx);
                                                } else {
                                                    cancel_launcher(cx);
                                                }
                                            },
                                        ),
                                    ),
                                )
                        })
                        .when(is_error, |this| {
                            this.flex()
                                .flex_col()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(px(12.))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .flex_1()
                                                .min_w(px(0.))
                                                .text_color(status_color)
                                                .child(status_text.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .child(
                                                    ghost_button("复制错误", lucide_icons::icon_copy())
                                                        .on_mouse_down(
                                                            MouseButton::Left,
                                                            move |_ev, _window, cx| {
                                                                let _ = copy_launcher_error(cx);
                                                                cx.stop_propagation();
                                                            },
                                                        ),
                                                )
                                                .child(
                                                    primary_button(
                                                        &colors,
                                                        "重试",
                                                        lucide_icons::icon_rotate_ccw(),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_ev, _window, cx| {
                                                            let _ = retry_launcher(cx);
                                                            cx.stop_propagation();
                                                        },
                                                    ),
                                                ),
                                        ),
                                )
                        }),
                ),
        );

    modal::animated_modal_layer(
        card,
        hsla(0.0, 0.0, 1.0, 0.30),
        smooth,
        snapshot.modal_visible,
    )
    .into_any_element()
}

fn is_error_line(line: &str) -> bool {
    line.contains(" ERROR ")
        || line.contains("错误")
        || line.contains("失败")
        || line.contains("HRESULT")
}

fn is_warning_line(line: &str) -> bool {
    line.contains(" WARN ") || line.contains("警告") || line.contains("warning")
}

fn is_success_line(line: &str) -> bool {
    line.contains("成功") || line.contains("完成") || line.contains("已就绪")
}

fn line_color(line: &str) -> Hsla {
    if is_error_line(line) {
        rgb(0xdc2626).into()
    } else if is_warning_line(line) {
        rgb(0xd97706).into()
    } else if is_success_line(line) {
        rgb(0x16a34a).into()
    } else {
        rgb(0x475569).into()
    }
}

fn ghost_button(label: &'static str, icon_path: &'static str) -> Div {
    div()
        .h(px(36.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(10.))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0x475569))
        .hover(|style| style.text_color(rgb(0x1e293b)).bg(rgb(0xf8fafc)))
        .child(
            svg()
                .path(icon_path)
                .size(px(14.))
                .text_color(rgb(0x64748b)),
        )
        .child(label)
}

fn primary_button(colors: &ThemeColors, label: &'static str, icon_path: &'static str) -> Div {
    div()
        .h(px(36.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(10.))
        .bg(colors.accent)
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.btn_primary_text)
        .shadow(vec![BoxShadow {
            color: colors.accent_glow,
            blur_radius: px(16.),
            spread_radius: px(0.),
            offset: point(px(0.), px(8.)),
        }])
        .hover(|style| style.bg(colors.accent_hover))
        .child(
            svg()
                .path(icon_path)
                .size(px(14.))
                .text_color(colors.btn_primary_text),
        )
        .child(label)
}

fn action_button(
    label: &'static str,
    icon_path: Option<&'static str>,
    is_terminal: bool,
    action_fg: Hsla,
) -> Div {
    let border_color = if is_terminal {
        rgb(0xcbd5e1)
    } else {
        rgb(0xcbd5e1)
    };
    let background = if is_terminal {
        rgb(0xf8fafc)
    } else {
        rgb(0xf8fafc)
    };

    let button = div()
        .h(px(34.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(9.))
        .border_1()
        .border_color(border_color)
        .bg(background)
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .text_color(action_fg)
        .hover(|style| style.bg(rgb(0xf1f5f9)).text_color(rgb(0x1e293b)));

    if let Some(icon_path) = icon_path {
        button
            .child(svg().path(icon_path).size(px(14.)).text_color(action_fg))
            .child(label)
    } else {
        button.child(label)
    }
}

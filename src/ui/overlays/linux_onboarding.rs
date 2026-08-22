#![cfg(target_os = "linux")]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::components::modal;
use crate::ui::state::linux_onboarding::{
    LinuxOnboardingEnvironmentSummary, LinuxOnboardingState, LinuxOnboardingStep,
};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

pub fn render_linux_onboarding_overlay(
    state: &LinuxOnboardingState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    );
    let window_size = window.bounds().size;
    let card_width = (window_size.width - px(40.)).max(px(360.)).min(px(720.));
    let card_height = (window_size.height - px(48.)).max(px(420.)).min(px(660.));

    let step_index = match state.step {
        LinuxOnboardingStep::Welcome => 1,
        LinuxOnboardingStep::Environment => 2,
        LinuxOnboardingStep::AcquireGame => 3,
        LinuxOnboardingStep::Runtime => 4,
    };

    let body = match state.step {
        LinuxOnboardingStep::Welcome => render_welcome(&colors),
        LinuxOnboardingStep::Environment => render_environment(state, &colors),
        LinuxOnboardingStep::AcquireGame => render_acquire(state, &colors),
        LinuxOnboardingStep::Runtime => render_runtime(state, &colors),
    };

    let card = div()
        .w(card_width)
        .h(card_height)
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .bg(colors.bg)
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(26.))
                .pt(px(22.))
                .pb(px(16.))
                .border_b_1()
                .border_color(colors.border)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(icon_shell(&colors, lucide_icons::icon_blocks()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .text_size(px(20.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child("欢迎使用 BMCBL"),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child("Linux 首次运行设置向导"),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_muted)
                        .child(format!("{step_index} / 4")),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .px(px(26.))
                .py(px(20.))
                .child(body),
        )
        .child(render_footer(state, &colors));

    modal::modal_layer(card, hsla(0.0, 0.0, 0.0, 0.38)).into_any_element()
}

fn render_welcome(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(
            div()
                .text_size(px(15.))
                .line_height(relative(1.55))
                .text_color(colors.text_secondary)
                .child(
                    "Linux 下 BMCBL 直接管理解包后的 Minecraft Bedrock 版本，并通过 Proton/UMU 兼容环境运行。不会注册 Windows UWP，也不会执行 Store UWP 数据迁移。",
                ),
        )
        .child(feature_card(
            colors,
            lucide_icons::icon_package_plus(),
            "下载和导入版本",
            "支持 BMCBL 在线版本来源，以及 APPX、ZIP、MSIXVC 本地安装包导入。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_package(),
            "多版本目录",
            "游戏版本保存在 BMCBL/versions，各版本文件彼此独立，不需要 Windows PackageManager 注册。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_shield_check(),
            "Linux 原生引导",
            "本向导只检查 Linux 发行版、Proton-GDK/UMU 运行环境和本地版本，不执行 UWP/AppContainer 检查。",
        ))
        .into_any_element()
}

fn render_environment(state: &LinuxOnboardingState, colors: &ThemeColors) -> AnyElement {
    if state.scanning {
        return div()
            .h(px(260.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(
                svg()
                    .path(lucide_icons::icon_loader_circle())
                    .w(px(24.))
                    .h(px(24.))
                    .text_color(colors.accent),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .text_color(colors.text_secondary)
                    .child("正在检查 Linux 发行版、Proton-GDK 和本地游戏版本…"),
            )
            .into_any_element();
    }

    if let Some(error) = &state.error {
        return div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(status_card(colors, "Linux 环境扫描失败", error.clone(), true))
            .into_any_element();
    }

    let Some(environment) = &state.environment else {
        return div()
            .h(px(220.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(colors.text_secondary)
            .child("准备扫描 Linux 运行环境")
            .into_any_element();
    };

    let runtime_text = if environment.runtime_ready {
        format!(
            "运行环境可用 · {}",
            environment
                .runner_label
                .as_deref()
                .unwrap_or("已检测到 Proton/UMU runner")
        )
    } else {
        format!(
            "运行环境尚未就绪 · {}",
            environment
                .missing_reason
                .as_deref()
                .unwrap_or("未检测到可用 Proton-GDK/UMU runner")
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(colors.text_secondary)
                .child("Linux 不进行 UWP 注册检查；这里只检查实际运行 Bedrock 所需的本机兼容环境。"),
        )
        .child(status_card(
            colors,
            "Linux 发行版",
            environment.distribution_name.clone(),
            false,
        ))
        .child(status_card(
            colors,
            "Proton-GDK / UMU",
            SharedString::from(runtime_text),
            !environment.runtime_ready,
        ))
        .child(status_card(
            colors,
            "BMCBL 已有版本",
            SharedString::from(format!("检测到 {} 个本地版本目录", environment.bmcbl_versions)),
            false,
        ))
        .into_any_element()
}

fn render_acquire(state: &LinuxOnboardingState, colors: &ThemeColors) -> AnyElement {
    let existing = state
        .environment
        .as_ref()
        .map_or(0, |environment| environment.bmcbl_versions);
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(colors.text_secondary)
                .child("Linux 与 Windows 共用版本下载/导入资产，但安装完成后不会注册为 Windows UWP。"),
        )
        .child(feature_card(
            colors,
            lucide_icons::icon_package_plus(),
            "从 BMCBL 下载",
            "浏览 Minecraft Bedrock 版本并下载到本地 versions 目录。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_package_open(),
            "导入本地安装包",
            "支持 APPX、ZIP、MSIXVC；BMCBL 负责解包/转换为本地版本目录。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_circle_check(),
            "使用已有版本",
            if existing > 0 {
                "已经检测到本地版本，可完成向导后直接选择版本启动。"
            } else {
                "当前尚未检测到本地版本，建议完成向导后前往下载页。"
            },
        ))
        .into_any_element()
}

fn render_runtime(state: &LinuxOnboardingState, colors: &ThemeColors) -> AnyElement {
    let runtime_ready = state
        .environment
        .as_ref()
        .is_some_and(|environment| environment.runtime_ready);
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(status_card(
            colors,
            "Proton-GDK 运行环境",
            SharedString::from(if runtime_ready {
                "当前已检测到可用兼容环境。BMCBL 启动 Bedrock 时会使用 Proton/UMU runner，不需要 Windows 开发者模式或 UWP 散装注册。"
            } else {
                "当前兼容环境尚未就绪。可以从“设置 → Proton-GDK”安装/选择 runner；缺少系统 32 位 glibc 时，BMCBL 会通过独立 Linux runtime 提示处理。"
            }),
            !runtime_ready,
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_shield_check(),
            "没有 UWP 数据迁移",
            "Linux 不会卸载 Microsoft Store 包，也不会访问 Windows Packages/Microsoft.MinecraftUWP LocalState 迁移链路。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_package(),
            "历史版本仍建议隔离数据",
            "跨大版本打开同一世界仍可能产生不可逆升级。版本文件重定向/数据隔离与 UWP 注册无关，Linux 同样建议使用。",
        ))
        .into_any_element()
}

fn render_footer(state: &LinuxOnboardingState, colors: &ThemeColors) -> AnyElement {
    let mut left = secondary_button(
        colors,
        if state.step == LinuxOnboardingStep::Welcome {
            "跳过引导"
        } else {
            "上一步"
        },
    );
    if state.step == LinuxOnboardingStep::Welcome {
        left = left.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            complete_onboarding(cx, None);
        });
    } else {
        left = left.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.update_global(|state: &mut LinuxOnboardingState, _cx| state.back());
        });
    }

    let (right_label, right_enabled) = match state.step {
        LinuxOnboardingStep::Welcome => ("开始设置", true),
        LinuxOnboardingStep::Environment => (
            "继续",
            !state.scanning && state.environment.is_some(),
        ),
        LinuxOnboardingStep::AcquireGame => ("检查运行环境", true),
        LinuxOnboardingStep::Runtime => ("完成设置", true),
    };
    let mut right = primary_button(colors, right_label, right_enabled);
    if right_enabled {
        right = right.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            let step = cx.global::<LinuxOnboardingState>().step;
            match step {
                LinuxOnboardingStep::Welcome => start_environment_scan(cx),
                LinuxOnboardingStep::Environment | LinuxOnboardingStep::AcquireGame => {
                    cx.update_global(|state: &mut LinuxOnboardingState, _cx| state.next());
                }
                LinuxOnboardingStep::Runtime => complete_onboarding(cx, None),
            }
        });
    }

    let mut actions = div().flex().items_center().gap(px(10.));
    if state.step == LinuxOnboardingStep::AcquireGame {
        let mut download = secondary_button(colors, "打开下载页");
        download = download.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            complete_onboarding(cx, Some(crate::ui::navigation::AppRoute::Download));
        });
        actions = actions.child(download);
    }
    if state.step == LinuxOnboardingStep::Runtime {
        let mut proton = secondary_button(colors, "打开 Proton-GDK 设置");
        proton = proton.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            complete_onboarding(cx, Some(crate::ui::navigation::AppRoute::Settings));
            cx.update_global(
                |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                    state.tab = crate::ui::views::settings::state::SettingsTab::ProtonGdk;
                },
            );
        });
        actions = actions.child(proton);
    }
    actions = actions.child(right);

    div()
        .px(px(26.))
        .py(px(16.))
        .border_t_1()
        .border_color(colors.border)
        .flex()
        .items_center()
        .justify_between()
        .child(left)
        .child(actions)
        .into_any_element()
}

fn start_environment_scan(cx: &mut App) {
    let request_id = cx.update_global(|state: &mut LinuxOnboardingState, _cx| state.begin_scan());
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(move || {
            let check = crate::core::linux_runtime::check_linux_runtime();
            let versions = crate::utils::file_ops::bmcbl_subdir("versions");
            let bmcbl_versions = std::fs::read_dir(versions)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter(|entry| entry.path().is_dir())
                        .count() as u64
                })
                .unwrap_or(0);
            let runner_label = check.runner.as_ref().map(|runner| {
                SharedString::from(format!(
                    "{} · {}",
                    runner.kind.display_name(),
                    runner.executable.display()
                ))
            });
            LinuxOnboardingEnvironmentSummary {
                distribution_name: SharedString::from(check.distribution_name.to_string()),
                runtime_ready: check.is_ready(),
                runner_label,
                missing_reason: check
                    .missing_reason
                    .as_deref()
                    .map(|reason| SharedString::from(reason.to_string())),
                bmcbl_versions,
            }
        })
        .await;
        cx.update(|cx| match result {
            Ok(environment) => {
                cx.update_global(|state: &mut LinuxOnboardingState, _cx| {
                    state.apply_environment(request_id, environment);
                });
            }
            Err(error) => {
                cx.update_global(|state: &mut LinuxOnboardingState, _cx| {
                    state.set_error(request_id, format!("Linux 环境扫描任务失败: {error}"));
                });
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn complete_onboarding(cx: &mut App, route: Option<crate::ui::navigation::AppRoute>) {
    if let Err(error) = crate::config::onboarding::complete_current_onboarding() {
        let request_id = cx.global::<LinuxOnboardingState>().request_id_for_error();
        cx.update_global(|state: &mut LinuxOnboardingState, _cx| {
            state.set_error(request_id, format!("保存首次运行设置失败: {error}"));
        });
        return;
    }
    cx.update_global(|state: &mut LinuxOnboardingState, _cx| state.finish());
    if let Some(route) = route {
        crate::ui::navigation::set_route(cx, route);
    }
}

fn icon_shell(colors: &ThemeColors, path: &'static str) -> AnyElement {
    div()
        .w(px(40.))
        .h(px(40.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.14,
            ..colors.accent
        })
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(path)
                .w(px(20.))
                .h(px(20.))
                .text_color(colors.accent),
        )
        .into_any_element()
}

fn feature_card(
    colors: &ThemeColors,
    icon: &'static str,
    title: &'static str,
    description: &'static str,
) -> AnyElement {
    div()
        .p(px(16.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .flex()
        .items_start()
        .gap(px(13.))
        .child(icon_shell(colors, icon))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(relative(1.45))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
        .into_any_element()
}

fn status_card(
    colors: &ThemeColors,
    title: &'static str,
    description: SharedString,
    danger: bool,
) -> AnyElement {
    let accent = if danger { colors.danger } else { colors.accent };
    div()
        .p(px(15.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            svg()
                .path(if danger {
                    lucide_icons::icon_triangle_alert()
                } else {
                    lucide_icons::icon_circle_check()
                })
                .w(px(19.))
                .h(px(19.))
                .text_color(accent),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(relative(1.45))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
        .into_any_element()
}

fn primary_button(colors: &ThemeColors, label: &'static str, enabled: bool) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!("linux-onboarding-primary-{label}")))
        .h(px(38.))
        .px(px(16.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label);
    if enabled {
        button = button
            .bg(colors.accent)
            .text_color(colors.btn_primary_text)
            .cursor_pointer()
            .hover(|this| this.bg(colors.accent_hover));
    } else {
        button = button.bg(colors.surface).text_color(colors.text_muted);
    }
    button
}

fn secondary_button(colors: &ThemeColors, label: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("linux-onboarding-secondary-{label}")))
        .h(px(38.))
        .px(px(15.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .text_color(colors.text_primary)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
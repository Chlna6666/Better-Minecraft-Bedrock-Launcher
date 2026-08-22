#![cfg(target_os = "windows")]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::launch_prereq::{LaunchPrereqState, OnboardingStep};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

pub fn render_onboarding_overlay(
    state: &LaunchPrereqState,
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

    let step_index = match state.onboarding_step {
        OnboardingStep::Welcome => 1,
        OnboardingStep::Environment => 2,
        OnboardingStep::AcquireGame => 3,
        OnboardingStep::DataSafety => 4,
    };

    let body = match state.onboarding_step {
        OnboardingStep::Welcome => render_welcome(&colors),
        OnboardingStep::Environment => render_environment(state, &colors),
        OnboardingStep::AcquireGame => render_acquire(state, &colors),
        OnboardingStep::DataSafety => render_data_safety(state, &colors),
    };

    let footer = render_footer(state, &colors);
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
                                        .child("首次运行设置向导"),
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
        .child(footer);

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
                    "BMCBL 可以下载、导入并切换多个 Minecraft Bedrock 版本。旧版 UWP 使用散装 DevelopmentMode 注册；切换版本时 BMCBL 会重新指向对应版本目录。",
                ),
        )
        .child(feature_card(
            colors,
            lucide_icons::icon_package_plus(),
            "下载和导入版本",
            "支持在线版本列表，以及 APPX、ZIP、MSIXVC 本地安装包。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_package(),
            "多版本管理",
            "版本文件保存在 BMCBL/versions，下次切换时无需重新下载。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_shield_check(),
            "UWP 数据保护",
            "如果首次切换会替换 Microsoft Store UWP 注册，BMCBL 会先备份并校验原版游戏数据；备份失败时禁止卸载。",
        ))
        .into_any_element()
}

fn render_environment(state: &LaunchPrereqState, colors: &ThemeColors) -> AnyElement {
    if state.onboarding_scanning {
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
                    .child("正在扫描 Minecraft 环境和用户数据…"),
            )
            .into_any_element();
    }

    if let Some(error) = &state.onboarding_error {
        return div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(status_card(colors, "环境扫描失败", error.clone(), true))
            .into_any_element();
    }

    let Some(environment) = &state.onboarding_environment else {
        return div()
            .h(px(220.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(colors.text_secondary)
            .child("准备扫描 Minecraft 环境")
            .into_any_element();
    };

    let release_text = minecraft_environment_text(&environment.release);
    let preview_text = minecraft_environment_text(&environment.preview);

    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(14.))
                .text_color(colors.text_secondary)
                .child("已自动检查当前 Windows 用户的 Minecraft 注册来源、版本、用户数据和 BMCBL 版本目录。"),
        )
        .child(status_card(
            colors,
            "Microsoft Minecraft UWP",
            SharedString::from(release_text),
            false,
        ))
        .child(status_card(
            colors,
            "Minecraft Preview UWP",
            SharedString::from(preview_text),
            false,
        ))
        .child(status_card(
            colors,
            "BMCBL 已有版本",
            SharedString::from(format!("检测到 {} 个本地版本目录", environment.bmcbl_versions)),
            false,
        ))
        .into_any_element()
}

fn minecraft_environment_text(
    summary: &crate::core::minecraft::uwp_migration::MinecraftDataSummary,
) -> String {
    let registration = if !summary.registered {
        "未注册".to_string()
    } else {
        let version = summary.registered_version.as_deref().unwrap_or("未知版本");
        if summary.bmcbl_managed_registration {
            format!("BMCBL 散装 DevelopmentMode · 版本 {version}")
        } else if summary.development_mode {
            format!("外部 DevelopmentMode · 版本 {version}")
        } else {
            format!("Microsoft Store / 外部安装包 · 版本 {version}")
        }
    };

    if summary.data_present {
        format!(
            "{registration} · 数据：{} 个世界 · {} 个资源包 · {}",
            summary.worlds,
            summary.resource_packs,
            human_bytes(summary.total_size)
        )
    } else {
        format!("{registration} · 未发现 games/com.mojang 用户数据")
    }
}

fn render_acquire(state: &LaunchPrereqState, colors: &ThemeColors) -> AnyElement {
    let existing = state
        .onboarding_environment
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
                .child("之后可以随时从“下载”页面添加游戏版本。"),
        )
        .child(feature_card(
            colors,
            lucide_icons::icon_package_plus(),
            "从 BMCBL 下载",
            "浏览正式版和 Preview 版本，选择后由任务系统完成下载、校验和解包。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_package_open(),
            "导入本地安装包",
            "下载页右上角可以导入 APPX、ZIP 或 MSIXVC；BMCBL 会按包类型进入对应安装链路。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_circle_check(),
            "使用已有版本",
            if existing > 0 {
                "已经检测到本地版本，可以直接完成向导并从主页启动。"
            } else {
                "当前尚未检测到本地版本，建议完成向导后前往下载页。"
            },
        ))
        .into_any_element()
}

fn render_data_safety(state: &LaunchPrereqState, colors: &ThemeColors) -> AnyElement {
    let has_store_data = state.onboarding_environment.as_ref().is_some_and(|environment| {
        environment.release.data_present || environment.preview.data_present
    });
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(status_card(
            colors,
            "散装 UWP 多版本切换",
            SharedString::from(
                "BMCBL 会保留各版本目录，并在启动目标版本时切换 DevelopmentMode 注册。BMCBL 自己的散装版本之间切换会请求 Windows 保留应用数据。",
            ),
            false,
        ))
        .child(status_card(
            colors,
            "Microsoft Store → BMCBL",
            SharedString::from(if has_store_data {
                "检测到现有 Minecraft 数据。首次需要替换 Store/外部 UWP 注册时，会先复制 games/com.mojang 到 BMCBL/backups/migrations/uwp 并校验文件数量和字节数，随后才允许卸载；新散装版本注册成功后自动恢复。"
            } else {
                "当前没有检测到需要迁移的原版数据。以后若出现 Store/外部 UWP 数据，替换注册时仍会执行同样的强制备份安全门。"
            }),
            false,
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_shield_check(),
            "历史版本建议使用数据隔离",
            "跨大版本直接打开同一世界可能导致方块、区块或世界数据发生不可逆迁移。使用版本设置中的文件重定向/隔离可以降低降级风险。",
        ))
        .into_any_element()
}

fn render_footer(state: &LaunchPrereqState, colors: &ThemeColors) -> AnyElement {
    let mut left = secondary_button(
        colors,
        if state.onboarding_step == OnboardingStep::Welcome {
            "跳过引导"
        } else {
            "上一步"
        },
    );
    if state.onboarding_step == OnboardingStep::Welcome {
        left = left.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            complete_onboarding(cx, None);
        });
    } else {
        left = left.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.update_global(|state: &mut LaunchPrereqState, _cx| state.onboarding_back());
        });
    }

    let (right_label, right_enabled) = match state.onboarding_step {
        OnboardingStep::Welcome => ("开始设置", true),
        OnboardingStep::Environment => (
            "继续",
            !state.onboarding_scanning && state.onboarding_environment.is_some(),
        ),
        OnboardingStep::AcquireGame => ("前往数据安全", true),
        OnboardingStep::DataSafety => ("完成设置", true),
    };
    let mut right = primary_button(colors, right_label, right_enabled);
    if right_enabled {
        right = right.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            let step = cx.global::<LaunchPrereqState>().onboarding_step;
            match step {
                OnboardingStep::Welcome => start_environment_scan(cx),
                OnboardingStep::Environment | OnboardingStep::AcquireGame => {
                    cx.update_global(|state: &mut LaunchPrereqState, _cx| state.onboarding_next());
                }
                OnboardingStep::DataSafety => complete_onboarding(cx, None),
            }
        });
    }

    let mut actions = div().flex().items_center().gap(px(10.));
    if state.onboarding_step == OnboardingStep::AcquireGame {
        let mut download = secondary_button(colors, "打开下载页");
        download = download.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            complete_onboarding(cx, Some(crate::ui::navigation::AppRoute::Download));
        });
        actions = actions.child(download);
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
    cx.update_global(|state: &mut LaunchPrereqState, _cx| state.begin_onboarding_scan());
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(
            crate::core::minecraft::uwp_migration::scan_onboarding_environment,
        )
        .await;
        cx.update(|cx| match result {
            Ok(environment) => cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                state.set_onboarding_environment(environment);
            }),
            Err(error) => cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                state.set_onboarding_error(format!("环境扫描任务失败: {error}"));
            }),
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn complete_onboarding(cx: &mut App, route: Option<crate::ui::navigation::AppRoute>) {
    if let Err(error) = crate::config::onboarding::complete_current_onboarding() {
        cx.update_global(|state: &mut LaunchPrereqState, _cx| {
            state.set_onboarding_error(format!("保存首次运行设置失败: {error}"));
        });
        return;
    }
    cx.update_global(|state: &mut LaunchPrereqState, _cx| state.finish_onboarding());
    if let Some(route) = route {
        crate::ui::navigation::set_route(cx, route);
    }
}

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.2} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.1} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} B")
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
        .id(SharedString::from(format!("onboarding-primary-{label}")))
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
        .id(SharedString::from(format!("onboarding-secondary-{label}")))
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
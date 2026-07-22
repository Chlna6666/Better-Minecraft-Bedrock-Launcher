use crate::ui::components::toast;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render(colors: &ThemeColors) -> impl IntoElement {
    let runner_root = crate::utils::file_ops::runners_dir();
    let config = crate::config::config::read_config().unwrap_or_default();
    let mut runners = crate::core::linux_runtime::installed_proton_gdk_runners();
    let configured_path = std::path::PathBuf::from(&config.launcher.proton_gdk_runner);
    if configured_path.is_file() && !runners.contains(&configured_path) {
        runners.push(configured_path);
    }
    let selected_runner = if config.launcher.proton_gdk_runner.trim().is_empty() {
        crate::core::linux_runtime::resolve_proton_runner()
            .map(|runner| runner.executable.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        config.launcher.proton_gdk_runner
    };
    let source = crate::core::linux_runtime::ProtonGdkSource::from_config(
        &config.launcher.proton_gdk_source,
    );
    let is_ready = crate::core::linux_runtime::resolve_proton_runner().is_ok();
    let has_runners = !runners.is_empty();

    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(page_heading(colors))
        .child(environment_overview(colors, is_ready))
        .child(source_selector(colors, source))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .mt(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child("已安装版本"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(colors.text_muted)
                                .child("启动游戏时使用当前选中的 Proton-GDK"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            action_button(
                                colors,
                                "注册本地版本",
                                lucide_icons::icon_folder_open(),
                                false,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                |_event, window, cx| {
                                    register_local_runner(window, cx);
                                },
                            ),
                        )
                        .child(
                            action_button(
                                colors,
                                "安装新版本",
                                lucide_icons::icon_download(),
                                true,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                |_event, _window, cx| {
                                    start_latest_install(cx);
                                },
                            ),
                        ),
                ),
        )
        .when(has_runners, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(runners.into_iter().map(|runner| {
                        let selected = selected_runner == runner.to_string_lossy();
                        installed_runner_card(colors, runner, selected)
                    })),
            )
        })
        .when(!has_runners, |this| this.child(empty_runner_card(colors)))
        .child(storage_footer(colors, runner_root))
}

fn page_heading(colors: &ThemeColors) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(20.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("Proton-GDK"),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(colors.text_secondary)
                        .child("管理 Minecraft Bedrock 在 Linux 上使用的兼容运行环境"),
                ),
        )
}

fn source_selector(
    colors: &ThemeColors,
    selected: crate::core::linux_runtime::ProtonGdkSource,
) -> Div {
    div()
        .w_full()
        .p(px(14.))
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.52,
            ..colors.settings_card_bg
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("下载源"),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors.text_muted)
                        .child("安装新版本时从选中的仓库获取 Release"),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(source_option(
                    colors,
                    "LukasPAH Custom（推荐）",
                    crate::core::linux_runtime::ProtonGdkSource::LukasPah,
                    selected,
                ))
                .child(source_option(
                    colors,
                    "Weather-OS（旧版）",
                    crate::core::linux_runtime::ProtonGdkSource::WeatherOs,
                    selected,
                )),
        )
}

fn source_option(
    colors: &ThemeColors,
    label: &'static str,
    source: crate::core::linux_runtime::ProtonGdkSource,
    selected: crate::core::linux_runtime::ProtonGdkSource,
) -> Stateful<Div> {
    let active = source == selected;
    div()
        .id(SharedString::from(format!("proton-gdk-source-{label}")))
        .h(px(34.))
        .px(px(11.))
        .rounded(px(9.))
        .border_1()
        .border_color(if active {
            Hsla {
                a: 0.34,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.18,
                ..colors.border
            }
        })
        .bg(if active {
            Hsla {
                a: 0.16,
                ..colors.accent
            }
        } else {
            colors.surface
        })
        .text_color(if active {
            colors.accent
        } else {
            colors.text_secondary
        })
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .cursor_pointer()
        .when(active, |this| {
            this.child(
                svg()
                    .path(lucide_icons::icon_check())
                    .w(px(13.))
                    .h(px(13.))
                    .text_color(colors.accent),
            )
        })
        .child(label)
        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            if active {
                return;
            }
            match crate::config::config::update_config(|config| {
                config.launcher.proton_gdk_source = source.config_value().to_string();
            }) {
                Ok(()) => {
                    cx.update_global(
                        |_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {},
                    );
                    toast::success(cx, format!("下载源已切换为 {label}").into());
                }
                Err(error) => {
                    toast::error(cx, format!("保存下载源失败：{error}").into());
                }
            };
        })
}

fn environment_overview(colors: &ThemeColors, is_ready: bool) -> Div {
    let (status, description, tone) = if is_ready {
        (
            "已选择运行环境",
            "Proton-GDK 已安装，启动游戏前会继续检查 GDK API 兼容性",
            colors.accent,
        )
    } else {
        (
            "需要安装运行环境",
            "尚未检测到 Proton-GDK，安装后才能启动 Bedrock UWP/GDK 版本",
            colors.danger,
        )
    };

    div()
        .w_full()
        .p(px(18.))
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla { a: 0.22, ..tone })
        .bg(Hsla { a: 0.08, ..tone })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(13.))
                .child(
                    div()
                        .size(px(42.))
                        .rounded(px(12.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(Hsla { a: 0.16, ..tone })
                        .text_color(tone)
                        .child(
                            svg()
                                .path(if is_ready {
                                    lucide_icons::icon_shield_check()
                                } else {
                                    lucide_icons::icon_package_open()
                                })
                                .w(px(21.))
                                .h(px(21.))
                                .text_color(tone),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(status),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_secondary)
                                .child(description),
                        ),
                ),
        )
        .child(
            div()
                .id("proton-gdk-status-action")
                .px(px(10.))
                .py(px(5.))
                .rounded(px(999.))
                .bg(Hsla { a: 0.14, ..tone })
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone)
                .cursor_pointer()
                .child(if is_ready {
                    "可用"
                } else {
                    "未安装 · 点击安装"
                })
                .hover(move |this| this.bg(Hsla { a: 0.22, ..tone }))
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    if !is_ready {
                        start_latest_install(cx);
                    }
                }),
        )
}

fn start_latest_install(cx: &mut App) {
    let source = crate::config::config::read_config()
        .map(|config| {
            crate::core::linux_runtime::ProtonGdkSource::from_config(
                &config.launcher.proton_gdk_source,
            )
        })
        .unwrap_or(crate::core::linux_runtime::ProtonGdkSource::LukasPah);
    let task_id = crate::core::linux_runtime::start_proton_gdk_install_latest(source);
    toast::success(
        cx,
        SharedString::from(format!("已开始下载 Proton-GDK（任务 {task_id}）")),
    );
}

fn register_local_runner(window: &Window, cx: &mut App) {
    let Some(folder) = crate::utils::file_picker::pick_directory_path_for_window(window) else {
        return;
    };
    let root = std::path::PathBuf::from(folder);
    let executable = [root.join("proton"), root.join("bin").join("proton")]
        .into_iter()
        .find(|candidate| candidate.is_file());
    let Some(executable) = executable else {
        toast::error(cx, "所选目录中没有 proton 或 bin/proton".into());
        return;
    };
    let executable = executable.to_string_lossy().into_owned();
    match crate::config::config::update_config(|config| {
        config.launcher.proton_gdk_runner = executable.clone();
    }) {
        Ok(()) => {
            cx.update_global(
                |_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {},
            );
            toast::success(cx, "本地 Proton-GDK 已注册并设为默认".into());
        }
        Err(error) => {
            toast::error(cx, format!("保存本地 Proton-GDK 失败：{error}").into());
        }
    };
}

fn installed_runner_card(
    colors: &ThemeColors,
    executable: std::path::PathBuf,
    selected: bool,
) -> Stateful<Div> {
    let executable_for_action = executable.clone();
    let executable_for_delete = executable.clone();
    div()
        .id(SharedString::from(format!(
            "proton-gdk-runner-{}",
            executable.display()
        )))
        .w_full()
        .p(px(16.))
        .rounded(px(14.))
        .border_1()
        .border_color(if selected {
            Hsla {
                a: 0.38,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.20,
                ..colors.border
            }
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.settings_card_bg
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(18.))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_w(px(0.))
                .flex_1()
                .child(
                    div()
                        .size(px(38.))
                        .rounded(px(10.))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.accent
                        })
                        .text_color(colors.accent)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(lucide_icons::icon_box())
                                .w(px(19.))
                                .h(px(19.))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child("Proton-GDK"),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(colors.text_muted)
                                .overflow_hidden()
                                .child(executable.to_string_lossy().into_owned()),
                        ),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(7.))
                .child(
                    div()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.14,
                            ..colors.accent
                        })
                        .text_size(px(11.))
                        .text_color(colors.accent)
                        .child(if selected {
                            "当前使用"
                        } else {
                            "设为默认"
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "proton-gdk-delete-{}",
                            executable.display()
                        )))
                        .size(px(30.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            svg()
                                .path(lucide_icons::icon_trash_2())
                                .w(px(14.))
                                .h(px(14.))
                                .text_color(colors.danger),
                        )
                        .hover(|this| {
                            this.bg(Hsla {
                                a: 0.10,
                                ..colors.danger
                            })
                        })
                        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                            cx.stop_propagation();
                            remove_runner(&executable_for_delete, selected, cx);
                        }),
                ),
        )
        .hover(|this| this.bg(colors.surface_hover))
        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            if selected {
                return;
            }
            let path = executable_for_action.to_string_lossy().into_owned();
            match crate::config::config::update_config(|config| {
                config.launcher.proton_gdk_runner = path.clone();
            }) {
                Ok(()) => {
                    cx.update_global(
                        |_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {},
                    );
                    toast::success(cx, "已设为默认 Proton-GDK".into());
                }
                Err(error) => {
                    toast::error(cx, format!("保存默认版本失败：{error}").into());
                }
            };
        })
}

fn empty_runner_card(colors: &ThemeColors) -> Div {
    div()
        .w_full()
        .min_h(px(150.))
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.44,
            ..colors.settings_card_bg
        })
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(
            div()
                .size(px(42.))
                .rounded(px(12.))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.text_muted
                })
                .text_color(colors.text_muted)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_package_open())
                        .w(px(20.))
                        .h(px(20.))
                        .text_color(colors.text_muted),
                ),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_primary)
                .child("还没有安装 Proton-GDK"),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors.text_muted)
                .child("安装后，版本会显示在这里"),
        )
}

fn remove_runner(executable: &std::path::Path, selected: bool, cx: &mut App) {
    let runners_root = crate::utils::file_ops::runners_dir();
    let managed_root = executable
        .strip_prefix(&runners_root)
        .ok()
        .and_then(|relative| {
            relative
                .components()
                .next()
                .map(|version| runners_root.join(version.as_os_str()))
        });
    let is_managed = managed_root.is_some();
    if let Some(managed_root) = managed_root
        && let Err(error) = std::fs::remove_dir_all(&managed_root)
    {
        toast::error(cx, format!("删除 Proton-GDK 失败：{error}").into());
        return;
    }
    if selected
        && let Err(error) = crate::config::config::update_config(|config| {
            config.launcher.proton_gdk_runner.clear();
        })
    {
        toast::error(cx, format!("清除默认版本失败：{error}").into());
        return;
    }
    cx.update_global(|_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {});
    toast::success(
        cx,
        if is_managed {
            "Proton-GDK 版本已删除".into()
        } else {
            "本地 Proton-GDK 已取消注册".into()
        },
    );
}

fn storage_footer(colors: &ThemeColors, runner_root: std::path::PathBuf) -> Div {
    div()
        .mt(px(2.))
        .px(px(4.))
        .flex()
        .flex_col()
        .items_start()
        .gap(px(16.))
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .flex()
                .items_center()
                .gap(px(7.))
                .text_size(px(11.5))
                .text_color(colors.text_muted)
                .child(
                    svg()
                        .path(lucide_icons::icon_folder())
                        .w(px(14.))
                        .h(px(14.))
                        .text_color(colors.text_muted),
                )
                .child(format!("存储位置：{}", runner_root.display())),
        )
        .child(
            action_button(colors, "清理版本", lucide_icons::icon_trash_2(), false).on_mouse_down(
                MouseButton::Left,
                move |_event, _window, cx| {
                    toast::error(cx, "请先选择要删除的 Proton-GDK 版本".into());
                },
            ),
        )
}

fn action_button(
    colors: &ThemeColors,
    label: &'static str,
    icon: &'static str,
    primary: bool,
) -> Stateful<Div> {
    let background = if primary {
        colors.accent
    } else {
        Hsla {
            a: 0.58,
            ..colors.surface
        }
    };
    let foreground = if primary {
        colors.btn_primary_text
    } else {
        colors.text_secondary
    };

    div()
        .id(SharedString::from(format!("proton-gdk-action-{label}")))
        .h(px(36.))
        .px(px(12.))
        .rounded(px(10.))
        .border_1()
        .border_color(if primary {
            Hsla {
                a: 0.28,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.18,
                ..colors.border
            }
        })
        .bg(background)
        .text_color(foreground)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(
            svg()
                .path(icon)
                .w(px(15.))
                .h(px(15.))
                .text_color(foreground),
        )
        .child(label)
        .hover(move |this| {
            this.bg(if primary {
                colors.accent_hover
            } else {
                colors.surface_hover
            })
        })
}

use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render(colors: &ThemeColors, i18n: &I18n) -> impl IntoElement {
    let runner_root = crate::utils::file_ops::runners_dir();
    let config = crate::config::config::read_config().unwrap_or_default();
    let mut runners = crate::core::linux_runtime::installed_proton_gdk_runners();
    let configured_path = std::path::PathBuf::from(&config.launcher.proton_gdk_runner);
    if configured_path.is_file()
        && !runners
            .iter()
            .any(|runner| runner.executable() == configured_path)
    {
        runners.push(crate::core::linux_runtime::installed_proton_gdk_runner(
            configured_path,
        ));
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
        .child(page_heading(colors, i18n))
        .child(environment_overview(colors, i18n, is_ready))
        .child(source_selector(colors, i18n, source))
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
                                .child(t!("Settings.proton_gdk.installed_title")),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(colors.text_muted)
                                .child(t!("Settings.proton_gdk.use_selected_description")),
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
                                t!("Settings.proton_gdk.register_local"),
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
                                t!("Settings.proton_gdk.install_latest"),
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
                        let selected = selected_runner == runner.executable().to_string_lossy();
                        installed_runner_card(colors, i18n, runner, selected)
                    })),
            )
        })
        .when(!has_runners, |this| {
            this.child(empty_runner_card(colors, i18n))
        })
        .child(storage_footer(colors, i18n, runner_root))
}

fn page_heading(colors: &ThemeColors, i18n: &I18n) -> Div {
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
                        .child(t!("Settings.proton_gdk.description")),
                ),
        )
}

fn source_selector(
    colors: &ThemeColors,
    i18n: &I18n,
    selected: crate::core::linux_runtime::ProtonGdkSource,
) -> Div {
    div()
        .w_full()
        .p(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                        .child(t!("Settings.proton_gdk.source")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(colors.text_muted)
                        .child(t!("Settings.proton_gdk.source_description")),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(source_option(
                    colors,
                    t!("Settings.proton_gdk.source_roundmcdev"),
                    crate::core::linux_runtime::ProtonGdkSource::RoundMcDev,
                    selected,
                ))
                .child(source_option(
                    colors,
                    t!("Settings.proton_gdk.source_lukaspah"),
                    crate::core::linux_runtime::ProtonGdkSource::LukasPah,
                    selected,
                ))
                .child(source_option(
                    colors,
                    t!("Settings.proton_gdk.source_weather_os"),
                    crate::core::linux_runtime::ProtonGdkSource::WeatherOs,
                    selected,
                )),
        )
}

fn source_option(
    colors: &ThemeColors,
    label: SharedString,
    source: crate::core::linux_runtime::ProtonGdkSource,
    selected: crate::core::linux_runtime::ProtonGdkSource,
) -> Stateful<Div> {
    let active = source == selected;
    let label_for_id = label.clone();
    let label_for_toast = label.clone();
    div()
        .id(SharedString::from(format!(
            "proton-gdk-source-{label_for_id}"
        )))
        .h(px(34.))
        .px(px(11.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                    toast::success(
                        cx,
                        t!(
                            "Settings.proton_gdk.source_changed",
                            source = label_for_toast
                        ),
                    );
                }
                Err(error) => {
                    toast::error(
                        cx,
                        t!("Settings.proton_gdk.source_save_failed", error = error),
                    );
                }
            };
        })
}

fn environment_overview(colors: &ThemeColors, i18n: &I18n, is_ready: bool) -> Div {
    let (status, description, tone) = if is_ready {
        (
            t!("Settings.proton_gdk.ready"),
            t!("Settings.proton_gdk.ready_description"),
            colors.accent,
        )
    } else {
        (
            t!("Settings.proton_gdk.not_ready"),
            t!("Settings.proton_gdk.not_ready_description"),
            colors.danger,
        )
    };

    div()
        .w_full()
        .p(px(18.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla { a: 0.14, ..tone })
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(tone)
                .cursor_pointer()
                .child(if is_ready {
                    t!("Settings.proton_gdk.available")
                } else {
                    t!("Settings.proton_gdk.install_action")
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
        .unwrap_or(crate::core::linux_runtime::ProtonGdkSource::RoundMcDev);
    let task_id = crate::core::linux_runtime::start_proton_gdk_install_latest(source);
    toast::success(
        cx,
        t!("Settings.proton_gdk.install_started", task = &task_id),
    );
    let terminal_task_id = task_id.clone();
    let terminal_task = gpui_tokio::Tokio::spawn_result(cx, async move {
        crate::tasks::task_manager::wait_for_task_terminal(&terminal_task_id)
            .await
            .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        let Ok(snapshot) = terminal_task.await else {
            return anyhow::Ok(());
        };
        if snapshot.status.as_ref() == "completed" {
            cx.update_global(
                |_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {},
            )?;
        }
        anyhow::Ok(())
    })
    .detach();
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
        toast::error(cx, t!("Settings.proton_gdk.runner_not_found"));
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
            toast::success(cx, t!("Settings.proton_gdk.runner_registered"));
        }
        Err(error) => {
            toast::error(
                cx,
                t!("Settings.proton_gdk.runner_save_failed", error = error),
            );
        }
    };
}

fn installed_runner_card(
    colors: &ThemeColors,
    i18n: &I18n,
    runner: crate::core::linux_runtime::InstalledProtonGdkRunner,
    selected: bool,
) -> Stateful<Div> {
    let executable = runner.executable().to_path_buf();
    let executable_for_action = executable.clone();
    let executable_for_delete = executable.clone();
    let display_name = runner.display_name().to_string();
    let source_summary = format!(
        "{} · {} · {}",
        runner.source_label(),
        runner.identity_label(),
        runner.login_capability()
    );
    let release_tag = runner.release_tag().map(str::to_string);
    let asset_count = runner.bundle_asset_count();
    div()
        .id(SharedString::from(format!(
            "proton-gdk-runner-{}",
            executable.display()
        )))
        .w_full()
        .p(px(16.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(display_name),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(11.5))
                                .text_color(colors.text_secondary)
                                .child(source_summary)
                                .when_some(release_tag, |this, release_tag| {
                                    this.child("·").child(release_tag)
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .text_color(colors.text_muted)
                                .overflow_hidden()
                                .child(executable.to_string_lossy().into_owned()),
                        )
                        .when(asset_count > 0, |this| {
                            this.child(div().text_size(px(10.5)).text_color(colors.accent).child(
                                t!(
                                    "Settings.proton_gdk.assets_integrated",
                                    count = &asset_count
                                ),
                            ))
                        }),
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
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.14,
                            ..colors.accent
                        })
                        .text_size(px(11.))
                        .text_color(colors.accent)
                        .child(if selected {
                            t!("Settings.proton_gdk.current")
                        } else {
                            t!("Settings.proton_gdk.set_default")
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "proton-gdk-delete-{}",
                            executable.display()
                        )))
                        .size(px(30.))
                        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                    toast::success(cx, t!("Settings.proton_gdk.default_set"));
                }
                Err(error) => {
                    toast::error(
                        cx,
                        t!("Settings.proton_gdk.default_save_failed", error = error),
                    );
                }
            };
        })
}

fn empty_runner_card(colors: &ThemeColors, i18n: &I18n) -> Div {
    div()
        .w_full()
        .min_h(px(150.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                .child(t!("Settings.proton_gdk.empty_title")),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(colors.text_muted)
                .child(t!("Settings.proton_gdk.empty_description")),
        )
}

fn remove_runner(executable: &std::path::Path, selected: bool, cx: &mut App) {
    let executable = executable.to_path_buf();
    let configured_runner = crate::config::config::read_config()
        .ok()
        .map(|config| std::path::PathBuf::from(config.launcher.proton_gdk_runner));
    let removal_task = gpui_tokio::Tokio::spawn_result(cx, async move {
        crate::tasks::runtime::run_io_blocking(move || {
            crate::core::linux_runtime::remove_managed_proton_gdk_runner(&executable)
        })
        .await
        .map_err(anyhow::Error::msg)?
        .map_err(anyhow::Error::msg)
    });
    cx.spawn(async move |cx| {
        match removal_task.await {
            Ok(Some(removed_root)) => {
                let should_clear_config = selected
                    || configured_runner
                        .as_deref()
                        .is_some_and(|configured| configured.starts_with(&removed_root));
                if should_clear_config
                    && let Err(error) = crate::config::config::update_config(|config| {
                        config.launcher.proton_gdk_runner.clear();
                    })
                {
                    toast::push_async(
                        cx,
                        toast::ToastKind::Error,
                        t!("Settings.proton_gdk.clear_default_failed", error = error),
                    );
                    return anyhow::Ok(());
                }
                cx.update_global(
                    |_state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {},
                )?;
                toast::push_async(
                    cx,
                    toast::ToastKind::Success,
                    t!("Settings.proton_gdk.removed"),
                );
            }
            Ok(None) => {
                if selected
                    && let Err(error) = crate::config::config::update_config(|config| {
                        config.launcher.proton_gdk_runner.clear();
                    })
                {
                    toast::push_async(
                        cx,
                        toast::ToastKind::Error,
                        t!("Settings.proton_gdk.clear_default_failed", error = error),
                    );
                    return anyhow::Ok(());
                }
                toast::push_async(
                    cx,
                    toast::ToastKind::Success,
                    t!("Settings.proton_gdk.unregistered"),
                );
            }
            Err(error) => {
                toast::push_async(
                    cx,
                    toast::ToastKind::Error,
                    t!("Settings.proton_gdk.remove_failed", error = error),
                );
            }
        }
        anyhow::Ok(())
    })
    .detach();
}

fn storage_footer(colors: &ThemeColors, i18n: &I18n, runner_root: std::path::PathBuf) -> Div {
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
                .child(t!(
                    "Settings.proton_gdk.storage_path",
                    path = runner_root.display()
                )),
        )
        .child(
            action_button(
                colors,
                t!("Settings.proton_gdk.cleanup"),
                lucide_icons::icon_trash_2(),
                false,
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                toast::error(cx, t!("Settings.proton_gdk.select_to_remove"));
            }),
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
        .rounded(px(crate::ui::theme::tokens::radius::SM))
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

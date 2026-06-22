use crate::config::config::ProxyType;
use crate::ui::components::input::{Input, InputState};
use crate::ui::components::toast::{self, ToastKind};
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{LauncherDisplayMode, SettingsPageState};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use tracing::warn;

#[derive(Clone)]
pub(super) struct SettingsSnapshot {
    pub(super) launcher_display_mode: LauncherDisplayMode,
    pub(super) fix_uwp_minimize: bool,
    pub(super) keep_downloaded_packages: bool,
    pub(super) modify_appx_manifest: bool,
    pub(super) debug: bool,
    pub(super) renderer_backend: String,
    pub(super) gpu_adapter_name: String,
    pub(super) stats_upload: bool,
    pub(super) error_report_sentry_enabled: bool,
    pub(super) error_report_sentry_auto: bool,
    pub(super) update_channel_nightly: bool,
    pub(super) auto_check_updates: bool,
    pub(super) music_auto_play_on_startup: bool,
    pub(super) download_multi_thread: bool,
    pub(super) download_auto_thread_count: bool,
    pub(super) download_max_threads: u32,
    pub(super) download_proxy_type: String,
    pub(super) download_curseforge_api_source: String,
    pub(super) download_curseforge_api_base: String,
    pub(super) download_http_proxy_url: String,
    pub(super) download_socks_proxy_url: String,
    pub(super) theme_color: String,
    pub(super) background_option: String,
    pub(super) local_image_path: String,
    pub(super) network_image_url: String,
    pub(super) background_blur: f32,
    pub(super) show_launch_animation: bool,
    pub(super) font_source: String,
    pub(super) local_font_path: String,
    pub(super) local_font_family: String,
    pub(super) system_font_family: String,
}

pub(super) fn snapshot_from_state(state: &SettingsPageState) -> SettingsSnapshot {
    SettingsSnapshot {
        launcher_display_mode: state.launcher_display_mode,
        fix_uwp_minimize: state.fix_uwp_minimize,
        keep_downloaded_packages: state.keep_downloaded_packages,
        modify_appx_manifest: state.modify_appx_manifest,
        debug: state.debug,
        renderer_backend: state.renderer_backend.to_string(),
        gpu_adapter_name: state.gpu_adapter_name.to_string(),
        stats_upload: state.stats_upload,
        error_report_sentry_enabled: state.error_report_sentry_enabled,
        error_report_sentry_auto: state.error_report_sentry_auto,
        update_channel_nightly: state.update_channel_nightly,
        auto_check_updates: state.auto_check_updates,
        music_auto_play_on_startup: state.music_auto_play_on_startup,
        download_multi_thread: state.download_multi_thread,
        download_auto_thread_count: state.download_auto_thread_count,
        download_max_threads: state.download_max_threads.clamp(1, 256),
        download_proxy_type: state.download_proxy_type.to_string(),
        download_curseforge_api_source: state.download_curseforge_api_source.to_string(),
        download_curseforge_api_base: state.download_curseforge_api_base.to_string(),
        download_http_proxy_url: state.download_http_proxy_url.to_string(),
        download_socks_proxy_url: state.download_socks_proxy_url.to_string(),
        theme_color: state.theme_color.to_string(),
        background_option: state.background_option.to_string(),
        local_image_path: state.local_image_path.to_string(),
        network_image_url: state.network_image_url.to_string(),
        background_blur: crate::config::config::clamp_background_blur(state.background_blur),
        show_launch_animation: state.show_launch_animation,
        font_source: state.font_source.to_string(),
        local_font_path: state.local_font_path.to_string(),
        local_font_family: state.local_font_family.to_string(),
        system_font_family: state.system_font_family.to_string(),
    }
}

pub(super) fn spawn_persist_settings(snapshot: SettingsSnapshot, cx: &mut App) {
    spawn_persist_settings_with_success(snapshot, None, cx);
}

pub(super) fn spawn_persist_settings_with_success(
    snapshot: SettingsSnapshot,
    on_success: Option<Rc<dyn Fn(&mut App)>>,
    cx: &mut App,
) {
    let settings_loaded = cx.read_global(|state: &SettingsPageState, _cx| state.loaded);
    if !settings_loaded {
        warn!("skip persisting settings before settings page finishes loading");
        toast::error(cx, SharedString::from("设置仍在加载，请稍后再试"));
        return;
    }

    let toast_id = toast::pending(cx, SharedString::from("保存设置中..."));

    cx.spawn(async move |cx| {
        let res = tokio::task::spawn_blocking(move || {
            crate::config::config::update_config(|cfg| {
                cfg.game.launcher_visibility = match snapshot.launcher_display_mode {
                    LauncherDisplayMode::MinimizeOnLaunch => "minimize".to_string(),
                    LauncherDisplayMode::CloseOnLaunch => "close".to_string(),
                    LauncherDisplayMode::KeepVisible => "keep".to_string(),
                };
                cfg.game.uwp_minimize_fix = snapshot.fix_uwp_minimize;
                cfg.game.keep_downloaded_game_package = snapshot.keep_downloaded_packages;
                cfg.game.modify_appx_manifest = snapshot.modify_appx_manifest;
                cfg.launcher.debug = snapshot.debug;
                cfg.launcher.renderer_backend =
                    crate::config::config::normalize_renderer_backend(&snapshot.renderer_backend);
                cfg.launcher.gpu_adapter_name =
                    crate::config::config::normalize_gpu_adapter_name(&snapshot.gpu_adapter_name);
                cfg.launcher.stats_upload = snapshot.stats_upload;
                cfg.launcher.error_report_sentry_enabled = snapshot.error_report_sentry_enabled;
                if cfg.launcher.error_report_sentry_dsn.trim().is_empty() {
                    cfg.launcher.error_report_sentry_dsn =
                        crate::config::config::default_error_report_sentry_dsn();
                }
                cfg.launcher.error_report_sentry_auto =
                    snapshot.error_report_sentry_enabled && snapshot.error_report_sentry_auto;
                cfg.launcher.update_channel = if snapshot.update_channel_nightly {
                    crate::config::config::UpdateChannel::Nightly
                } else {
                    crate::config::config::UpdateChannel::Stable
                };
                cfg.launcher.auto_check_updates = snapshot.auto_check_updates;
                cfg.music.auto_play_on_startup = snapshot.music_auto_play_on_startup;
                cfg.launcher.download.multi_thread = snapshot.download_multi_thread;
                cfg.launcher.download.auto_thread_count = snapshot.download_auto_thread_count;
                cfg.launcher.download.max_threads = snapshot.download_max_threads.clamp(1, 256);
                cfg.launcher.download.proxy.proxy_type =
                    match snapshot.download_proxy_type.to_lowercase().as_str() {
                        "system" => ProxyType::System,
                        "http" => ProxyType::Http,
                        "socks5" => ProxyType::Socks5,
                        _ => ProxyType::None,
                    };
                cfg.launcher.download.curseforge_api_source = match snapshot
                    .download_curseforge_api_source
                    .to_lowercase()
                    .as_str()
                {
                    "official" => "official".to_string(),
                    "custom" => "custom".to_string(),
                    _ => "mirror".to_string(),
                };
                cfg.launcher.download.curseforge_api_base = snapshot.download_curseforge_api_base;
                cfg.launcher.download.proxy.http_proxy_url = snapshot.download_http_proxy_url;
                cfg.launcher.download.proxy.socks_proxy_url = snapshot.download_socks_proxy_url;
                cfg.custom_style.theme_color = snapshot.theme_color;
                cfg.custom_style.background_option = snapshot.background_option;
                cfg.custom_style.local_image_path = snapshot.local_image_path;
                cfg.custom_style.network_image_url = snapshot.network_image_url;
                cfg.custom_style.background_blur =
                    crate::config::config::clamp_background_blur(snapshot.background_blur);
                cfg.custom_style.show_launch_animation = snapshot.show_launch_animation;
                cfg.custom_style.font_source =
                    crate::config::config::normalize_font_source(&snapshot.font_source);
                cfg.custom_style.local_font_path = snapshot.local_font_path;
                cfg.custom_style.local_font_family = snapshot.local_font_family;
                cfg.custom_style.system_font_family = snapshot.system_font_family;
            })?;
            Ok::<(), std::io::Error>(())
        })
        .await;

        if let Err(join_err) = res {
            warn!("persist settings join error: {join_err}");
            toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from("保存设置失败"),
            );
        } else if let Ok(Err(io_err)) = res {
            warn!("persist settings failed: {io_err}");
            toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Error,
                SharedString::from(format!("保存设置失败: {io_err}")),
            );
        } else {
            toast::resolve_async(
                cx,
                toast_id,
                ToastKind::Success,
                SharedString::from("设置已保存"),
            );
            if let Some(on_success) = on_success {
                if let Err(error) = cx.update(move |cx| {
                    on_success(cx);
                }) {
                    warn!("persist settings success callback failed: {error:?}");
                }
            }
        }
    })
    .detach();
}

pub(super) fn spawn_persist_background_blur(blur: f32, cx: &mut App) {
    let blur = crate::config::config::clamp_background_blur(blur);
    cx.spawn(async move |_cx| {
        let result = tokio::task::spawn_blocking(move || {
            crate::config::config::update_config(|cfg| {
                cfg.custom_style.background_blur = blur;
            })?;
            Ok::<(), std::io::Error>(())
        })
        .await;

        match result {
            Err(error) => warn!("persist background blur join error: {error}"),
            Ok(Err(error)) => warn!("persist background blur failed: {error}"),
            Ok(Ok(())) => {}
        }
    })
    .detach();
}

pub(super) fn settings_card(colors: &ThemeColors, id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .w_full()
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.12,
                ..rgb(0x000000).into()
            },
            blur_radius: px(16.),
            spread_radius: px(-6.),
            offset: point(px(0.), px(6.)),
        }])
        .child(
            div()
                .absolute()
                .top_0()
                .left(px(14.))
                .right(px(14.))
                .h(px(1.))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.border
                }),
        )
}

pub(super) fn settings_inline_card(colors: &ThemeColors, id: &'static str) -> Stateful<Div> {
    settings_card(colors, id)
        .px(px(16.))
        .py(px(16.))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(22.))
        .hover(|this| {
            this.bg(Hsla {
                a: 0.84,
                ..colors.surface_hover
            })
            .border_color(Hsla {
                a: 0.30,
                ..colors.border
            })
        })
}

pub(super) fn settings_card_header(
    colors: &ThemeColors,
    title: SharedString,
    desc: SharedString,
) -> Div {
    div()
        .w_full()
        .px(px(14.))
        .py(px(13.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(settings_card_text(colors, title, desc))
}

pub(super) fn settings_card_text(
    colors: &ThemeColors,
    title: SharedString,
    desc: SharedString,
) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(relative(1.45))
                .text_color(Hsla {
                    a: 0.92,
                    ..colors.text_secondary
                })
                .child(desc),
        )
}

pub(super) fn settings_badge(colors: &ThemeColors, label: SharedString) -> Div {
    div()
        .max_w(px(260.))
        .h(px(28.))
        .px(px(10.))
        .rounded(px(999.))
        .bg(Hsla {
            a: 0.14,
            ..colors.accent
        })
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.accent
        })
        .flex()
        .items_center()
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_primary)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(label),
        )
}

pub(super) fn settings_option_row_shell(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    active: bool,
) -> Div {
    div()
        .w_full()
        .px(px(14.))
        .py(px(10.))
        .border_t_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .bg(if active {
            Hsla {
                a: 0.08,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.0,
                ..colors.surface
            }
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if active {
                            colors.text_primary
                        } else {
                            Hsla {
                                a: 0.92,
                                ..colors.text_secondary
                            }
                        })
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(Hsla {
                            a: 0.76,
                            ..colors.text_muted
                        })
                        .child(desc),
                ),
        )
}

pub(super) fn settings_control_box(
    colors: &ThemeColors,
    active: bool,
    width: Pixels,
    control: impl IntoElement,
) -> Div {
    div()
        .w(width)
        .min_w(px(0.))
        .h(px(30.))
        .rounded(px(10.))
        .text_color(colors.text_primary)
        .bg(Hsla {
            a: 0.84,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: if active { 0.34 } else { 0.24 },
            ..if active { colors.accent } else { colors.border }
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.10,
                ..rgb(0x000000).into()
            },
            blur_radius: px(12.0),
            spread_radius: px(-6.0),
            offset: point(px(0.), px(4.)),
        }])
        .px(px(10.))
        .flex()
        .items_center()
        .child(control)
}

pub(super) fn settings_value_box(
    colors: &ThemeColors,
    display: SharedString,
    active: bool,
    width: Pixels,
) -> Div {
    settings_control_box(
        colors,
        active,
        width,
        div()
            .w_full()
            .min_w(px(0.))
            .text_size(px(12.))
            .text_color(colors.text_secondary)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
            .child(display),
    )
}

pub(super) fn settings_action_button(
    colors: &ThemeColors,
    label: SharedString,
    enabled: bool,
) -> Div {
    div()
        .w(px(92.))
        .flex_shrink_0()
        .h(px(30.))
        .px(px(8.))
        .rounded(px(10.))
        .bg(Hsla {
            a: 0.84,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.10,
                ..rgb(0x000000).into()
            },
            blur_radius: px(12.0),
            spread_radius: px(-6.0),
            offset: point(px(0.), px(4.)),
        }])
        .when(enabled, |this| this.cursor_pointer())
        .when(!enabled, |this| this.opacity(0.72))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_primary)
                .child(label),
        )
}

pub(super) fn settings_sub_row(
    colors: &ThemeColors,
    label: SharedString,
    control: impl IntoElement,
) -> Div {
    div()
        .w_full()
        .px(px(14.))
        .py(px(10.))
        .border_t_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(Hsla {
                    a: 0.92,
                    ..colors.text_secondary
                })
                .child(label),
        )
        .child(control)
}

pub(super) fn settings_sub_input_row(
    colors: &ThemeColors,
    label: SharedString,
    input: Option<&Entity<InputState>>,
    placeholder: &'static str,
) -> Div {
    let control: AnyElement = if let Some(input_state) = input {
        Input::new(input_state)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(true)
            .w_full()
            .h(px(30.))
            .px(px(4.))
            .text_size(px(13.))
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(30.))
            .px(px(10.))
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child(placeholder),
            )
            .into_any_element()
    };

    settings_sub_row(
        colors,
        label,
        settings_control_box(colors, false, px(280.), control),
    )
}

pub(super) fn page_shell(content: impl IntoElement, colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .left(px(18.))
        .right(px(18.))
        .top(px(88.))
        .bottom(px(18.))
        .flex()
        .justify_center()
        .child(
            div()
                .relative()
                .w_full()
                .h_full()
                .max_w(px(960.))
                .rounded(px(18.))
                .overflow_hidden()
                .border_1()
                .border_color(Hsla {
                    a: 0.22,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.90,
                    ..colors.settings_panel_bg
                })
                .shadow(vec![BoxShadow {
                    color: Hsla {
                        a: 0.16,
                        ..rgb(0x000000).into()
                    },
                    blur_radius: px(36.),
                    spread_radius: px(-8.),
                    offset: point(px(0.), px(18.)),
                }])
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(18.))
                        .bg(linear_gradient(
                            180.0,
                            linear_color_stop(
                                Hsla {
                                    a: 0.14,
                                    ..colors.accent
                                },
                                0.0,
                            ),
                            linear_color_stop(
                                Hsla {
                                    a: 0.02,
                                    ..colors.surface
                                },
                                1.0,
                            ),
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(18.))
                        .right(px(18.))
                        .h(px(1.))
                        .bg(Hsla {
                            a: 0.18,
                            ..colors.border
                        }),
                )
                .p(px(14.))
                .child(content),
        )
}

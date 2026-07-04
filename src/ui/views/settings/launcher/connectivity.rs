use crate::ui::animation::{ease_out_cubic_motion, repeating_linear_motion};
use crate::ui::components::icon::themed_icon;
use crate::ui::components::modal;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{
    LauncherConnectivityItem, LauncherConnectivityStatus, SettingsPageState,
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;
use std::time::Duration;
use tracing::warn;

use super::super::common::{settings_card, settings_card_header, settings_sub_row};

#[derive(Clone, Copy)]
struct ConnectivityService {
    name: &'static str,
    url: &'static str,
}

#[derive(Clone, Copy)]
struct ConnectivityGroup {
    services: &'static [ConnectivityService],
}

const CONNECTIVITY_GROUPS: &[ConnectivityGroup] = &[
    ConnectivityGroup {
        services: &[
            ConnectivityService {
                name: "BMCBL API",
                url: "https://api.chlna6666.com/",
            },
            ConnectivityService {
                name: "Update Proxy",
                url: "https://dl-proxy.bmcbl.com/",
            },
            ConnectivityService {
                name: "Update Check",
                url: "https://updater.bmcbl.com/",
            },
        ],
    },
    ConnectivityGroup {
        services: &[
            ConnectivityService {
                name: "Xbox Live Auth",
                url: "https://user.auth.xboxlive.com",
            },
            ConnectivityService {
                name: "Xbox XSTS Auth",
                url: "https://xsts.auth.xboxlive.com",
            },
            ConnectivityService {
                name: "GDK Download",
                url: "http://assets1.xboxlive.cn/",
            },
            ConnectivityService {
                name: "UWP Download",
                url: "http://tlu.dl.delivery.mp.microsoft.com/",
            },
            ConnectivityService {
                name: "UWP URL Parse",
                url: "https://fe3.delivery.mp.microsoft.com/",
            },
        ],
    },
    ConnectivityGroup {
        services: &[
            ConnectivityService {
                name: "CurseForge",
                url: "https://www.curseforge.com/minecraft-bedrock/",
            },
            ConnectivityService {
                name: "GitHub",
                url: "https://github.com",
            },
        ],
    },
];

pub(super) fn launcher_connectivity_row(
    colors: &ThemeColors,
    i18n: &I18n,
    busy: bool,
) -> impl IntoElement {
    let can_open = !busy;

    let mut row = settings_card(colors, "settings-launcher-connectivity")
        .overflow_hidden()
        .rounded(px(16.))
        .p(px(16.))
        .flex()
        .items_center()
        .justify_between();

    if can_open {
        row = row
            .cursor_pointer()
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
            .on_click(move |_, _, cx| {
                cx.update_global(|settings: &mut SettingsPageState, cx| {
                    settings.launcher_connectivity_open = true;
                });
                spawn_run_connectivity_tests(cx);
            });
    } else {
        row = row.opacity(0.84);
    }

    row.child(
        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(i18n.t("LauncherSettings.connectivity_test")),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child(i18n.t("LauncherSettings.connectivity_test_desc")),
            ),
    )
    .child(
        div()
            .px(px(10.))
            .py(px(6.))
            .rounded(px(999.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(Hsla {
                a: 0.62,
                ..colors.surface_hover
            })
            .child(themed_icon(
                lucide_icons::icon_chevron_right(),
                16.0,
                colors.text_secondary,
            )),
    )
}

pub(super) fn render_connectivity_modal(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    window_width: Pixels,
    window_height: Pixels,
) -> Option<AnyElement> {
    if !state.launcher_connectivity_open {
        return None;
    }

    let release = Rc::new(release_launcher_connectivity_state);
    let dismiss = Rc::new(close_launcher_connectivity_window);
    let close_button = release.clone();

    let refresh = Rc::new(|cx: &mut App| {
        spawn_run_connectivity_tests(cx);
    });
    let busy = state.launcher_connectivity_running;

    let total_count = state.launcher_connectivity_items.len();
    let loading_count = state
        .launcher_connectivity_items
        .iter()
        .filter(|item| item.status == LauncherConnectivityStatus::Loading)
        .count();
    let success_count = state
        .launcher_connectivity_items
        .iter()
        .filter(|item| item.status == LauncherConnectivityStatus::Success)
        .count();
    let error_count = state
        .launcher_connectivity_items
        .iter()
        .filter(|item| item.status == LauncherConnectivityStatus::Error)
        .count();
    let pending_count = state
        .launcher_connectivity_items
        .iter()
        .filter(|item| item.status == LauncherConnectivityStatus::Pending)
        .count();
    let card_width = if window_width <= px(720.) {
        window_width - px(36.)
    } else if window_width <= px(1120.) {
        px(760.)
    } else {
        px(860.)
    };
    let card_height = if window_height <= px(720.) {
        window_height - px(36.)
    } else {
        px(760.)
    };

    let header = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(20.))
        .pt(px(18.))
        .pb(px(16.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .w(px(34.))
                                .h(px(34.))
                                .rounded(px(12.))
                                .bg(Hsla {
                                    a: 0.16,
                                    ..colors.accent
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_globe())
                                        .w(px(18.))
                                        .h(px(18.))
                                        .text_color(colors.accent),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(24. / 1.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .child(i18n.t("Connectivity.title")),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(i18n.t("LauncherSettings.connectivity_test_desc")),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(icon_button(
                            colors,
                            "launcher-connectivity-refresh",
                            lucide_icons::icon_refresh_cw(),
                            !busy,
                            refresh,
                        ))
                        .child(icon_button(
                            colors,
                            "launcher-connectivity-close",
                            lucide_icons::icon_x(),
                            true,
                            close_button.clone(),
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(8.))
                .child(connectivity_stat_chip(
                    colors,
                    i18n.t("Connectivity.stats.total"),
                    total_count,
                    Hsla {
                        a: 0.14,
                        ..colors.text_secondary
                    },
                ))
                .child(connectivity_stat_chip(
                    colors,
                    i18n.t("Connectivity.stats.running"),
                    loading_count,
                    colors.accent,
                ))
                .child(connectivity_stat_chip(
                    colors,
                    i18n.t("Connectivity.stats.success"),
                    success_count,
                    colors.stat_green_text,
                ))
                .child(connectivity_stat_chip(
                    colors,
                    i18n.t("Connectivity.stats.error"),
                    error_count,
                    colors.danger,
                ))
                .child(connectivity_stat_chip(
                    colors,
                    i18n.t("Connectivity.stats.pending"),
                    pending_count,
                    colors.text_secondary,
                )),
        );

    let card = div()
        .id("launcher-connectivity-modal")
        .w(card_width)
        .h(card_height)
        .max_w(px(860.))
        .min_w(px(0.))
        .max_h(card_height)
        .flex()
        .flex_col()
        .rounded(px(20.))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.98,
            ..colors.surface
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.28,
                ..rgb(0x000000).into()
            },
            blur_radius: px(42.),
            spread_radius: px(0.),
            offset: point(px(0.), px(18.)),
        }])
        .child(header)
        .child(render_connectivity_list(colors, i18n, state));

    let animated_card = card.with_animation(
        "launcher-connectivity-modal-card",
        ease_out_cubic_motion(Duration::from_millis(260)),
        |card, progress| {
            card.opacity(progress)
                .relative()
                .left(px((1.0 - progress) * 28.0))
                .top(px((1.0 - progress) * 6.0))
        },
    );

    Some(
        modal::modal_layer_dismissible_with_cleanup(
            div()
                .w_full()
                .h_full()
                .p(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .child(animated_card),
            hsla(0., 0., 0., 0.32),
            release,
            dismiss,
        )
        .into_any_element(),
    )
}

fn connectivity_initial_items() -> Vec<LauncherConnectivityItem> {
    let mut items = Vec::new();
    for (group_index, group) in CONNECTIVITY_GROUPS.iter().enumerate() {
        for (item_index, service) in group.services.iter().enumerate() {
            items.push(LauncherConnectivityItem {
                group_index,
                item_index,
                name: SharedString::from(service.name),
                url: SharedString::from(service.url),
                status: LauncherConnectivityStatus::Pending,
                latency_ms: None,
                error: None,
            });
        }
    }
    items
}

fn spawn_run_connectivity_tests(cx: &mut App) {
    let req_id = match cx.update_global(|settings: &mut SettingsPageState, cx| -> Option<u64> {
        if settings.launcher_connectivity_running || settings.launcher_connectivity_task.is_some() {
            return None;
        }
        settings.launcher_connectivity_req_id =
            settings.launcher_connectivity_req_id.saturating_add(1);
        settings.launcher_connectivity_running = true;
        settings.launcher_connectivity_items = connectivity_initial_items();
        Some(settings.launcher_connectivity_req_id)
    }) {
        Some(req_id) => req_id,
        None => return,
    };

    let mut test_probes: Vec<(usize, usize, String)> = Vec::new();
    for (group_index, group) in CONNECTIVITY_GROUPS.iter().enumerate() {
        for (item_index, service) in group.services.iter().enumerate() {
            test_probes.push((group_index, item_index, service.url.to_string()));
        }
    }

    let clear_task = |cx: &mut AsyncApp| {
        if let Err(err) = cx.update_global(|_settings: &mut SettingsPageState, cx| {}) {
            warn!("connectivity clear task failed: {err:?}");
        }
    };

    let task = cx.spawn(async move |cx| {
        if let Err(err) = cx.update_global(|settings: &mut SettingsPageState, cx| {
            if settings.launcher_connectivity_req_id != req_id {
                return;
            }
            for item in &mut settings.launcher_connectivity_items {
                item.status = LauncherConnectivityStatus::Loading;
                item.latency_ms = None;
                item.error = None;
            }
        }) {
            warn!("connectivity set loading failed: {err:?}");
            clear_task(cx);
            return;
        }

        let mut probe_results = FuturesUnordered::new();
        for (group_index, item_index, url) in test_probes {
            probe_results.push(cx.background_spawn(async move {
                let result = crate::utils::network::test_network_connectivity_blocking(url);
                (group_index, item_index, Some(result))
            }));
        }

        while let Some((group_index, item_index, result)) = probe_results.next().await {
            let Some(result) = result else {
                break;
            };
            if let Err(err) = cx.update_global(|settings: &mut SettingsPageState, cx| {
                if settings.launcher_connectivity_req_id != req_id {
                    return;
                }
                if let Some(item) = settings
                    .launcher_connectivity_items
                    .iter_mut()
                    .find(|it| it.group_index == group_index && it.item_index == item_index)
                {
                    match result {
                        Ok(latency) => {
                            item.status = LauncherConnectivityStatus::Success;
                            item.latency_ms = Some(latency);
                            item.error = None;
                        }
                        Err(error) => {
                            item.status = LauncherConnectivityStatus::Error;
                            item.latency_ms = None;
                            item.error = Some(SharedString::from(error));
                        }
                    }
                }
            }) {
                warn!("connectivity set result failed: {err:?}");
                clear_task(cx);
                return;
            }
        }

        if let Err(err) = cx.update_global(|settings: &mut SettingsPageState, cx| {
            if settings.launcher_connectivity_req_id != req_id {
                return;
            }
            settings.launcher_connectivity_running = false;
        }) {
            warn!("connectivity finalize failed: {err:?}");
        }

        clear_task(cx);
    });
    task.detach();

    cx.update_global(|settings: &mut SettingsPageState, cx| {
        settings.launcher_connectivity_task = None;
    });
}

fn release_launcher_connectivity_state(cx: &mut App) {
    cx.update_global(|settings: &mut SettingsPageState, cx| {
        settings.release_launcher_connectivity_state();
    });
}

fn close_launcher_connectivity_window(cx: &mut App) {
    cx.update_global(|settings: &mut SettingsPageState, cx| {
        settings.launcher_connectivity_open = false;
    });
}

fn render_connectivity_list(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let mut list = div()
        .id("launcher-connectivity-list-scroll")
        .w_full()
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(12.));

    for (group_index, group) in CONNECTIVITY_GROUPS.iter().enumerate() {
        list = list.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .px(px(4.))
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .child(i18n.t(&format!("Connectivity.groups.{group_index}"))),
                )
                .children(
                    group
                        .services
                        .iter()
                        .enumerate()
                        .map(|(item_index, service)| {
                            let item = state.launcher_connectivity_items.iter().find(|it| {
                                it.group_index == group_index && it.item_index == item_index
                            });
                            let row_id =
                                ("launcher-connectivity-row", group_index * 100 + item_index);
                            match item.map(|it| it.status) {
                                Some(LauncherConnectivityStatus::Loading) => {
                                    connectivity_item_row(colors, i18n, service, item)
                                        .with_animation(
                                            row_id,
                                            ease_out_cubic_motion(Duration::from_millis(220)),
                                            |row, progress| {
                                                row.opacity(0.72 + progress * 0.28)
                                                    .relative()
                                                    .left(px((1.0 - progress) * 12.0))
                                            },
                                        )
                                }
                                _ => connectivity_item_row(colors, i18n, service, item)
                                    .with_animation(
                                        row_id,
                                        ease_out_cubic_motion(Duration::from_millis(220)),
                                        |row, progress| {
                                            row.opacity(0.58 + progress * 0.42)
                                                .relative()
                                                .left(px((1.0 - progress) * 14.0))
                                        },
                                    ),
                            }
                        }),
                ),
        );
    }

    list
}

fn connectivity_item_row(
    colors: &ThemeColors,
    i18n: &I18n,
    service: &ConnectivityService,
    item: Option<&LauncherConnectivityItem>,
) -> Div {
    let status = item
        .map(|it| it.status)
        .unwrap_or(LauncherConnectivityStatus::Pending);
    let latency = item.and_then(|it| it.latency_ms);

    let badge = match status {
        LauncherConnectivityStatus::Pending => pending_badge(
            colors.text_secondary,
            Hsla {
                a: 0.10,
                ..colors.text_secondary
            },
        ),
        LauncherConnectivityStatus::Loading => loading_badge(colors),
        LauncherConnectivityStatus::Success => {
            let milliseconds = latency.unwrap_or(0);
            let (background, foreground) = if milliseconds < 200 {
                (
                    Hsla {
                        a: 0.16,
                        ..colors.stat_green_text
                    },
                    colors.stat_green_text,
                )
            } else if milliseconds < 600 {
                (
                    Hsla {
                        a: 0.18,
                        ..colors.stat_orange_text
                    },
                    colors.stat_orange_text,
                )
            } else {
                (
                    Hsla {
                        a: 0.15,
                        ..colors.danger
                    },
                    colors.danger,
                )
            };

            status_badge(
                foreground,
                background,
                lucide_icons::icon_check(),
                format!("{milliseconds} ms"),
            )
        }
        LauncherConnectivityStatus::Error => status_badge(
            colors.danger,
            Hsla {
                a: 0.15,
                ..colors.danger
            },
            lucide_icons::icon_x(),
            i18n.t("Connectivity.status.error"),
        ),
    };

    div()
        .w_full()
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .px(px(12.))
        .py(px(12.))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(service.name),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .truncate()
                        .child(service.url),
                ),
        )
        .child(badge)
}

fn connectivity_stat_chip(
    colors: &ThemeColors,
    label: SharedString,
    value: usize,
    accent: Hsla,
) -> impl IntoElement {
    div()
        .flex_grow()
        .flex_basis(px(116.))
        .min_w(px(100.))
        .min_h(px(34.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(999.))
        .border_1()
        .border_color(Hsla { a: 0.12, ..accent })
        .bg(Hsla { a: 0.12, ..accent })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .truncate()
                .child(label),
        )
        .child(
            div()
                .flex_none()
                .min_w(px(18.))
                .text_align(TextAlign::Right)
                .text_size(px(11.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(accent)
                .child(value.to_string()),
        )
}

fn status_badge(
    foreground: Hsla,
    background: Hsla,
    icon: &'static str,
    body: impl IntoElement,
) -> AnyElement {
    div()
        .min_w(px(92.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(9.))
        .bg(background)
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(16.))
                .h(px(16.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(icon)
                        .w(px(13.))
                        .h(px(13.))
                        .text_color(foreground),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .truncate()
                .child(body),
        )
        .into_any_element()
}

fn pending_badge(foreground: Hsla, background: Hsla) -> AnyElement {
    div()
        .min_w(px(30.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(999.))
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child("-"),
        )
        .into_any_element()
}

fn loading_badge(colors: &ThemeColors) -> AnyElement {
    div()
        .relative()
        .w(px(48.))
        .h(px(14.))
        .rounded(px(999.))
        .bg(Hsla {
            a: 0.09,
            ..colors.accent
        })
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .child(
                    div()
                        .w(px(3.5))
                        .h(px(3.5))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.85,
                            ..colors.accent
                        })
                        .with_animation(
                            "launcher-connectivity-loading-dot-1",
                            repeating_linear_motion(Duration::from_millis(720)),
                            |dot, progress| {
                                let pulse = if progress < 0.5 {
                                    progress * 2.0
                                } else {
                                    (1.0 - progress) * 2.0
                                };
                                dot.opacity(0.28 + pulse * 0.72)
                            },
                        ),
                )
                .child(
                    div()
                        .w(px(4.))
                        .h(px(4.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.85,
                            ..colors.accent
                        })
                        .with_animation(
                            "launcher-connectivity-loading-dot-2",
                            repeating_linear_motion(Duration::from_millis(900)),
                            |dot, progress| {
                                let pulse = if progress < 0.5 {
                                    progress * 2.0
                                } else {
                                    (1.0 - progress) * 2.0
                                };
                                dot.opacity(0.28 + pulse * 0.72)
                            },
                        ),
                )
                .child(
                    div()
                        .w(px(3.5))
                        .h(px(3.5))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.85,
                            ..colors.accent
                        })
                        .with_animation(
                            "launcher-connectivity-loading-dot-3",
                            repeating_linear_motion(Duration::from_millis(1080)),
                            |dot, progress| {
                                let pulse = if progress < 0.5 {
                                    progress * 2.0
                                } else {
                                    (1.0 - progress) * 2.0
                                };
                                dot.opacity(0.28 + pulse * 0.72)
                            },
                        ),
                ),
        )
        .into_any_element()
}

fn icon_button(
    colors: &ThemeColors,
    id: &'static str,
    icon: &'static str,
    enabled: bool,
    on_click: Rc<dyn Fn(&mut App)>,
) -> Stateful<Div> {
    let mut button = div()
        .id(id)
        .w(px(32.))
        .h(px(32.))
        .rounded(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(colors.settings_card_bg)
        .child(
            svg()
                .path(icon)
                .w(px(16.))
                .h(px(16.))
                .text_color(colors.text_primary),
        );

    if enabled {
        button = button
            .cursor_pointer()
            .on_click(move |_, _, cx| on_click(cx));
    } else {
        button = button.opacity(0.45);
    }

    button
}

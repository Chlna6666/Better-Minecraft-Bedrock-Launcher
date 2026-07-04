use crate::ui::components::button::IconButton;
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::Input;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadChannelFilter, DownloadPageState, DownloadTab};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Instant;

pub(super) fn render_toolbar(colors: &ThemeColors, state: &DownloadPageState, now: Instant) -> Div {
    let search = render_toolbar_search(colors, state);

    div()
        .w_full()
        .h(px(68.))
        .bg(Hsla {
            a: 0.0,
            ..colors.surface
        })
        .px(px(20.))
        .py(px(14.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(render_tabs(colors, state, now))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .px(px(8.))
                .flex()
                .items_center()
                .child(div().w(px(200.)).min_w(px(0.)).child(search)),
        )
        .child(render_toolbar_controls(colors, state))
}

fn render_toolbar_search(colors: &ThemeColors, state: &DownloadPageState) -> AnyElement {
    let placeholder = match state.tab {
        DownloadTab::Game => "搜索游戏版本...",
        DownloadTab::ResourcePack => "搜索资源包...",
        DownloadTab::Mod => "搜索模组...",
    };

    let dark_mode = colors.bg.l < 0.5;
    let shell_background = if dark_mode {
        Hsla {
            a: 0.85,
            ..colors.settings_card_bg
        }
    } else {
        Hsla {
            a: 1.0,
            ..colors.settings_card_bg
        }
    };
    let shell_border = Hsla {
        a: 0.25,
        ..colors.border
    };

    if let Some(input_state) = state.search_input.as_ref() {
        div()
            .id("download-search-input-wrapper")
            .w_full()
            .flex()
            .items_center()
            .child(
                Input::new(input_state)
                    .cleanable(true)
                    .prefix(themed_icon(
                        lucide_icons::icon_search(),
                        16.0,
                        colors.text_secondary,
                    ))
                    .w_full()
                    .with_size(crate::ui::components::input::InputSize::Small),
            )
            .into_any_element()
    } else {
        div()
            .id("download-search-placeholder-wrapper")
            .w_full()
            .h(px(32.))
            .px(px(8.))
            .rounded(px(6.))
            .bg(shell_background)
            .border_1()
            .border_color(shell_border)
            .flex()
            .items_center()
            .gap(px(8.))
            .child(themed_icon(
                lucide_icons::icon_search(),
                16.0,
                colors.text_secondary,
            ))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_muted)
                    .child(placeholder),
            )
            .into_any_element()
    }
}

fn render_tabs(colors: &ThemeColors, state: &DownloadPageState, now: Instant) -> Div {
    let active = state.tab;
    let (t, animating) = state.tab_anim_factor(now);
    let from = state.tab_anim_from;
    // ease-out-back for a subtle elastic overshoot on the sliding pill
    let t_eased = {
        let tc = t.clamp(0.0, 1.0);
        let p = tc - 1.0;
        (1.0 + 1.35 * p.powi(3) + 0.35 * p.powi(2)).clamp(0.0, 1.05)
    };

    let idx = |tab: DownloadTab| match tab {
        DownloadTab::Game => 0f32,
        DownloadTab::ResourcePack => 1f32,
        DownloadTab::Mod => 2f32,
    };

    let item_w = 104.0f32;
    let from_x = idx(from) * item_w;
    let to_x = idx(active) * item_w;
    let x = from_x + (to_x - from_x) * t_eased;

    // Pill width stretches during transition for a dynamic feel
    let stretch = {
        let mid = (t * 2.0 - 1.0).abs();
        let stretch_factor = 1.0 - mid * 0.5;
        let distance = (idx(active) - idx(from)).abs();
        if animating && distance > 0.0 {
            item_w * stretch_factor.max(0.7)
        } else {
            item_w
        }
    };

    let tab = |id: &'static str,
               icon_path: &'static str,
               label: &'static str,
               tab: DownloadTab,
               active: DownloadTab| {
        let is_active = tab == active;
        let fg = if is_active {
            colors.text_primary
        } else {
            Hsla {
                a: 0.65,
                ..colors.text_primary
            }
        };

        div()
            .id(id)
            .w(px(item_w))
            .h(px(32.))
            .rounded(px(7.))
            .cursor_pointer()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .child(svg().path(icon_path).size(px(15.)).text_color(fg))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(fg)
                    .child(label),
            )
            .hover(move |s| {
                if is_active {
                    s
                } else {
                    s.bg(Hsla {
                        a: 0.08,
                        ..colors.text_primary
                    })
                }
            })
            .active(|s| s.top(px(1.0)))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                cx.update_global(|s: &mut DownloadPageState, cx| {
                    if s.tab != tab {
                        s.tab_anim_from = s.tab;
                        s.tab_anim_at = Some(Instant::now());
                    }
                    s.tab = tab;

                    if let Some(ref search_input) = s.search_input {
                        let placeholder = match tab {
                            DownloadTab::Game => "搜索游戏版本...",
                            DownloadTab::ResourcePack => "搜索资源包...",
                            DownloadTab::Mod => "搜索模组...",
                        };
                        let _ = search_input.update(cx, |st, cx| {
                            st.set_placeholder(SharedString::from(placeholder), window, cx);
                        });
                    }

                    match tab {
                        DownloadTab::Game => {
                            if !s.loaded && !s.loading {
                                s.force_refresh_next = true;
                            }
                            s.game_rows_scroll.set_offset(point(px(0.), px(0.)));
                        }
                        DownloadTab::ResourcePack => {
                            if !s.curseforge_loaded && !s.curseforge_loading {
                                s.curseforge_invalidate_seq =
                                    s.curseforge_invalidate_seq.wrapping_add(1);
                            }
                            s.curseforge_results_scroll
                                .set_offset(point(px(0.), px(0.)));
                            s.curseforge_sidebar_scroll
                                .set_offset(point(px(0.), px(0.)));
                        }
                        DownloadTab::Mod => {}
                    }
                });
            })
    };

    // Pill indicator with no shadow for flat depth
    let indicator = div()
        .absolute()
        .left(px(x + (item_w - stretch) * 0.5 + 3.0))
        .top(px(3.))
        .w(px(stretch))
        .h(px(32.))
        .rounded(px(7.))
        .bg(colors.surface);

    div()
        .relative()
        .flex()
        .items_center()
        .bg(Hsla {
            a: if colors.bg.l < 0.5 { 0.65 } else { 0.85 },
            ..colors.settings_card_bg
        })
        .p(px(3.))
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .child(indicator)
        .child(tab(
            "download-tab-game",
            lucide_icons::icon_box(),
            "游戏",
            DownloadTab::Game,
            active,
        ))
        .child(tab(
            "download-tab-resource",
            lucide_icons::icon_package(),
            "资源包",
            DownloadTab::ResourcePack,
            active,
        ))
        .child(tab(
            "download-tab-mod",
            lucide_icons::icon_layers(),
            "模组",
            DownloadTab::Mod,
            active,
        ))
        .when(animating, |this| this)
}

fn render_toolbar_controls(colors: &ThemeColors, state: &DownloadPageState) -> Div {
    let tab = state.tab;
    let refresh_disabled = match tab {
        DownloadTab::Game => state.loading || state.force_refresh_next,
        DownloadTab::ResourcePack => {
            state.curseforge_invalidate_task.is_some()
                || state.curseforge_search_commit_task.is_some()
                || state.curseforge_loading
                || state.curseforge_results_loading
                || state.curseforge_pending_page_index.is_some()
                || state.curseforge_page_commit_task.is_some()
        }
        DownloadTab::Mod => false,
    };

    let icon_btn = |id: &'static str, icon_path: &'static str, disabled: bool| {
        IconButton::new(id, icon_path)
            .icon_color(colors.text_secondary)
            .w(px(32.))
            .h(px(32.))
            .rounded(px(6.))
            .bg(Hsla {
                a: 0.06,
                ..colors.text_secondary
            })
            .disabled(disabled)
            .into_any_element()
    };

    let channel_filter: AnyElement = if tab == DownloadTab::Game {
        let (label, selected_index) = match state.channel_filter {
            DownloadChannelFilter::All => (SharedString::from("全部"), 0usize),
            DownloadChannelFilter::Release => (SharedString::from("正式"), 1usize),
            DownloadChannelFilter::Beta => (SharedString::from("测试版"), 2usize),
            DownloadChannelFilter::Preview => (SharedString::from("预览"), 3usize),
        };
        let options = vec![
            DropdownOption::from("全部"),
            DropdownOption::from("正式"),
            DropdownOption::from("测试版"),
            DropdownOption::from("预览"),
        ];
        Dropdown::new(
            "download-channel-filter",
            colors,
            px(112.),
            label,
            options,
            selected_index,
            true,
            move |ix, _window, cx| {
                let filter = match ix {
                    1 => DownloadChannelFilter::Release,
                    2 => DownloadChannelFilter::Beta,
                    3 => DownloadChannelFilter::Preview,
                    _ => DownloadChannelFilter::All,
                };
                cx.update_global(|s: &mut DownloadPageState, cx| {
                    if s.channel_filter == filter {
                        return;
                    }
                    s.channel_filter = filter;
                    s.page_index = 0;
                    s.game_rows_scroll.set_offset(point(px(0.), px(0.)));
                });
            },
        )
        .with_height(px(32.))
        .rounded(px(6.))
        .into_any_element()
    } else {
        div().into_any_element()
    };

    let refresh = IconButton::new("download-refresh", lucide_icons::icon_refresh_cw())
        .icon_color(colors.text_secondary)
        .w(px(32.))
        .h(px(32.))
        .rounded(px(6.))
        .bg(Hsla {
            a: 0.05,
            ..colors.text_secondary
        })
        .disabled(refresh_disabled)
        .on_click(|_ev, _window, cx: &mut App| {
            cx.update_global(|s: &mut DownloadPageState, cx| match s.tab {
                DownloadTab::Game => {
                    s.force_refresh_next = true;
                    s.loaded = false;
                    s.loading = false;
                    s.error = None;
                    s.versions.clear();
                }
                DownloadTab::ResourcePack => {
                    crate::ui::views::download::curseforge::schedule_invalidate_results_in_state(
                        s, cx,
                    );
                    s.curseforge_loaded = false;
                    s.curseforge_loading = false;
                    s.curseforge_error = None;
                    s.curseforge_categories.clear();
                    s.curseforge_versions.clear();
                    s.curseforge_page_index = 0;
                    s.curseforge_last_query_key = SharedString::from("");
                    s.curseforge_results_error = None;
                    s.curseforge_results_loading = true;
                    s.curseforge_results_scroll
                        .set_offset(point(px(0.), px(0.)));
                    s.curseforge_sidebar_scroll
                        .set_offset(point(px(0.), px(0.)));
                }
                DownloadTab::Mod => {}
            });
        })
        .into_any_element();

    let mut row = div().flex().items_center().gap(px(12.)).justify_end();

    match tab {
        DownloadTab::Game => {
            row = row
                .child(channel_filter)
                .child(icon_btn(
                    "download-import",
                    lucide_icons::icon_upload(),
                    false,
                ))
                .child(refresh);
        }
        DownloadTab::ResourcePack => {
            let enabled = state.curseforge_loaded;

            let mut version_options: Vec<DropdownOption> =
                Vec::with_capacity(1 + state.curseforge_versions.len());
            version_options.push(DropdownOption::from("全部版本"));
            for v in &state.curseforge_versions {
                version_options.push(DropdownOption::from(v.clone()));
            }

            let version_label = if !enabled {
                SharedString::from("加载中...")
            } else if state.curseforge_selected_game_version.trim().is_empty() {
                SharedString::from("全部版本")
            } else {
                state.curseforge_selected_game_version.clone()
            };

            let selected_version_index = if state.curseforge_selected_game_version.trim().is_empty()
            {
                0usize
            } else {
                state
                    .curseforge_versions
                    .iter()
                    .position(|v| v.as_ref() == state.curseforge_selected_game_version.as_ref())
                    .map(|ix| ix + 1)
                    .unwrap_or(0)
            };

            let version_select = Dropdown::new(
                "download-cf-version",
                colors,
                px(148.),
                version_label,
                version_options,
                selected_version_index,
                enabled,
                move |ix, _window, cx| {
                    let version = if ix == 0 {
                        SharedString::from("")
                    } else {
                        cx.read_global(|s: &DownloadPageState, _cx| {
                            s.curseforge_versions
                                .get(ix.saturating_sub(1))
                                .cloned()
                                .unwrap_or_else(|| SharedString::from(""))
                        })
                    };
                    cx.update_global(|s: &mut DownloadPageState, cx| {
                        crate::ui::views::download::curseforge::apply_results_query_change_in_state(
                            s,
                            cx,
                            |s| {
                                if s.curseforge_selected_game_version.as_ref() == version.as_ref() {
                                    return false;
                                }
                                s.curseforge_selected_game_version = version;
                                s.curseforge_page_index = 0;
                                s.curseforge_results_scroll
                                    .set_offset(point(px(0.), px(0.)));
                                true
                            },
                        );
                    });
                    crate::ui::views::download::curseforge::ensure_results_loaded(false, cx);
                },
            )
            .with_height(px(32.))
            .rounded(px(6.))
            .into_any_element();

            let sort_options = vec![
                DropdownOption::from("精选"),
                DropdownOption::from("热门"),
                DropdownOption::from("更新"),
                DropdownOption::from("名称"),
                DropdownOption::from("下载"),
            ];

            let (sort_label, sort_selected_index) = match state.curseforge_sort_field {
                2 => (SharedString::from("热门"), 1usize),
                3 => (SharedString::from("更新"), 2usize),
                4 => (SharedString::from("名称"), 3usize),
                6 => (SharedString::from("下载"), 4usize),
                _ => (SharedString::from("精选"), 0usize),
            };

            let sort_select = Dropdown::new(
                "download-cf-sort",
                colors,
                px(112.),
                if enabled {
                    sort_label
                } else {
                    SharedString::from("加载中...")
                },
                sort_options,
                sort_selected_index,
                enabled,
                move |ix, _window, cx| {
                    let sort_field = match ix {
                        1 => 2,
                        2 => 3,
                        3 => 4,
                        4 => 6,
                        _ => 1,
                    };
                    cx.update_global(|s: &mut DownloadPageState, cx| {
                        crate::ui::views::download::curseforge::apply_results_query_change_in_state(
                            s,
                            cx,
                            |s| {
                                if s.curseforge_sort_field == sort_field {
                                    return false;
                                }
                                s.curseforge_sort_field = sort_field;
                                s.curseforge_page_index = 0;
                                s.curseforge_results_scroll
                                    .set_offset(point(px(0.), px(0.)));
                                true
                            },
                        );
                    });
                    crate::ui::views::download::curseforge::ensure_results_loaded(false, cx);
                },
            )
            .with_height(px(32.))
            .rounded(px(6.))
            .into_any_element();

            row = row.child(version_select).child(sort_select).child(refresh);
        }
        DownloadTab::Mod => {
            row = row.child(refresh);
        }
    }

    row
}

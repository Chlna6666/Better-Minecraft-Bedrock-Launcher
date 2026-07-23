use crate::core::levilamina::{LeviLaminaModEntry, mod_matches_loader_version};
use crate::ui::components::button::{Button, IconButton};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render_mod_panel(colors: &ThemeColors, state: &DownloadPageState) -> Div {
    if state.levilauncher_loading && !state.levilauncher_loaded {
        return render_loading_state(colors);
    }

    if let Some(ref err) = state.levilauncher_error {
        return render_error_state(colors, err);
    }

    let query = state.search_query.trim().to_lowercase();
    let loader_type = state.levilauncher_selected_loader.as_ref();
    let loader_ver = state.levilauncher_selected_loader_version.as_ref();

    let filtered_mods: Vec<&LeviLaminaModEntry> = state
        .levilauncher_all_mods
        .iter()
        .filter(|m| {
            if !query.is_empty() {
                let name_match = m.name.to_lowercase().contains(&query);
                let desc_match = m.description.to_lowercase().contains(&query);
                let pkg_match = m.package_id.to_lowercase().contains(&query);
                if !name_match && !desc_match && !pkg_match {
                    return false;
                }
            }
            mod_matches_loader_version(m, loader_type, loader_ver)
        })
        .collect();

    let total_mods = filtered_mods.len();
    let page_size = state.levilauncher_page_size.max(1);
    let total_pages = (total_mods + page_size - 1) / page_size;
    let page_index = state
        .levilauncher_page_index
        .min(total_pages.saturating_sub(1));

    let start_idx = page_index * page_size;
    let end_idx = (start_idx + page_size).min(total_mods);
    let page_mods = if start_idx < total_mods {
        &filtered_mods[start_idx..end_idx]
    } else {
        &[][..]
    };

    let main_content = if filtered_mods.is_empty() {
        render_empty_state(colors)
    } else {
        render_mod_grid(colors, page_mods, state)
    };

    let stats_bar = render_stats_bar(colors, total_mods, loader_type, loader_ver);
    let pagination = render_pagination(colors, page_index, total_pages, total_mods);

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(colors.bg)
        .child(stats_bar)
        .child(
            div()
                .flex_1()
                .overflow_y_scrollbar()
                .track_scroll(&state.levilauncher_scroll)
                .p(px(20.))
                .child(main_content),
        )
        .child(pagination)
}

fn render_loading_state(colors: &ThemeColors) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(16.))
        .child(
            div()
                .w(px(64.))
                .h(px(64.))
                .rounded(px(16.))
                .bg(Hsla {
                    a: 0.08,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_refresh_cw())
                        .size(px(32.))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_primary)
                .child("正在加载 LeviLamina 客户端模组信息..."),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child("调用 https://lipr.levimc.org/levilauncher.json"),
        )
}

fn render_error_state(colors: &ThemeColors, err: &SharedString) -> Div {
    let err_str = err.to_string();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(16.))
        .child(
            div()
                .w(px(64.))
                .h(px(64.))
                .rounded(px(16.))
                .bg(Hsla {
                    a: 0.1,
                    ..colors.danger
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_info())
                        .size(px(32.))
                        .text_color(colors.danger),
                ),
        )
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child("加载 LeviLamina 模组信息失败"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(err_str),
        )
        .child(
            Button::new("retry-fetch-mods")
                .label("重新加载")
                .bg(colors.accent)
                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                .on_click(|_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_loaded = false;
                        s.levilauncher_loading = false;
                        s.levilauncher_error = None;
                    });
                }),
        )
}

fn render_empty_state(colors: &ThemeColors) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .py(px(64.))
        .gap(px(12.))
        .child(
            svg()
                .path(lucide_icons::icon_search_x())
                .size(px(48.))
                .text_color(colors.text_muted),
        )
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_primary)
                .child("未找到符合条件的 LeviLamina 客户端模组信息"),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child("请尝试调整搜索关键词或加载器版本筛选条件"),
        )
}

fn render_stats_bar(
    colors: &ThemeColors,
    total_mods: usize,
    loader_type: &str,
    loader_ver: &str,
) -> Div {
    let loader_filter_text =
        if loader_type.is_empty() || loader_type == "全部" || loader_type == "全部加载器" {
            if loader_ver.is_empty() || loader_ver == "全部版本" || loader_ver == "全部" {
                "全部加载器 & 全部版本".to_string()
            } else {
                format!("全部加载器 ({})", loader_ver)
            }
        } else {
            if loader_ver.is_empty() || loader_ver == "全部版本" || loader_ver == "全部" {
                loader_type.to_string()
            } else {
                format!("{} {}", loader_type, loader_ver)
            }
        };

    div()
        .w_full()
        .px(px(20.))
        .py(px(10.))
        .bg(Hsla {
            a: 0.03,
            ..colors.text_primary
        })
        .border_b_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(6.))
                        .bg(Hsla {
                            a: 0.1,
                            ..colors.accent
                        })
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.accent)
                        .child(format!("{} 个客户端模组", total_mods)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child(format!("(当前筛选: {})", loader_filter_text)),
                ),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child("仅显示 Client 客户端模组信息 | 来源于 LeviLauncher 注册表"),
        )
}

fn render_mod_grid(
    colors: &ThemeColors,
    mods: &[&LeviLaminaModEntry],
    _state: &DownloadPageState,
) -> Div {
    let mut grid = div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(16.))
        .items_stretch();

    for (idx, mod_entry) in mods.iter().enumerate() {
        grid = grid.child(render_mod_card(colors, mod_entry, idx));
    }

    grid
}

fn render_mod_card(colors: &ThemeColors, mod_entry: &LeviLaminaModEntry, idx: usize) -> AnyElement {
    let mod_clone = (*mod_entry).clone();
    let mod_clone_for_detail = mod_clone.clone();

    let avatar_element = if !mod_entry.avatar_url.trim().is_empty() {
        img(mod_entry.avatar_url.clone())
            .w(px(48.))
            .h(px(48.))
            .rounded(px(10.))
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .w(px(48.))
            .h(px(48.))
            .rounded(px(10.))
            .bg(Hsla {
                a: 0.08,
                ..colors.accent
            })
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(lucide_icons::icon_layers())
                    .size(px(24.))
                    .text_color(colors.accent),
            )
            .into_any_element()
    };

    let latest_ver = mod_entry
        .client_versions
        .first()
        .or_else(|| mod_entry.all_versions.first())
        .map(|s| s.as_str())
        .unwrap_or("1.0.0");

    let card_bg = Hsla {
        a: if colors.bg.l < 0.5 { 0.5 } else { 0.8 },
        ..colors.settings_card_bg
    };

    div()
        .id(ElementId::NamedInteger("mod-card".into(), idx as u64))
        .w(px(320.))
        .flex_grow()
        .min_h(px(160.))
        .bg(card_bg)
        .border_1()
        .border_color(Hsla {
            a: 0.12,
            ..colors.border
        })
        .rounded(px(12.))
        .p(px(14.))
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(12.))
        .hover(move |s| {
            s.border_color(Hsla {
                a: 0.35,
                ..colors.accent
            })
            .bg(Hsla {
                a: 0.02,
                ..colors.accent
            })
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(12.))
                        .child(avatar_element)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_primary)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(mod_entry.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.text_muted)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(mod_entry.package_id.clone()),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .line_height(px(17.))
                        .max_h(px(34.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(if mod_entry.description.trim().is_empty() {
                            "暂无描述".to_string()
                        } else {
                            mod_entry.description.clone()
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pt(px(8.))
                .border_t_1()
                .border_color(Hsla {
                    a: 0.06,
                    ..colors.border
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(Hsla {
                                    a: 0.08,
                                    ..colors.accent
                                })
                                .text_size(px(10.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.accent)
                                .child(format!("v{}", latest_ver)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_star())
                                        .size(px(12.))
                                        .text_color(colors.text_muted),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.text_muted)
                                        .child(mod_entry.stargazer_count.to_string()),
                                ),
                        ),
                )
                .child(
                    Button::new(ElementId::NamedInteger("btn-mod-detail".into(), idx as u64))
                        .label("详情")
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.accent
                        })
                        .text_color(colors.accent)
                        .on_click(move |_ev, _window, cx| {
                            let m = mod_clone_for_detail.clone();
                            let first_ver = m
                                .client_versions
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "".to_string());
                            cx.update_global(|s: &mut DownloadPageState, _cx| {
                                s.levilauncher_selected_mod = Some(m);
                                s.levilauncher_selected_version = SharedString::from(first_ver);
                                s.levilauncher_modal_open = true;
                            });
                        }),
                ),
        )
        .into_any_element()
}

fn render_pagination(
    colors: &ThemeColors,
    page_index: usize,
    total_pages: usize,
    _total_mods: usize,
) -> Div {
    if total_pages <= 1 {
        return div();
    }

    let prev_disabled = page_index == 0;
    let next_disabled = page_index + 1 >= total_pages;

    div()
        .w_full()
        .px(px(20.))
        .py(px(10.))
        .bg(colors.surface)
        .border_t_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(
            IconButton::new("mod-prev-page", lucide_icons::icon_chevron_left())
                .icon_color(colors.text_secondary)
                .w(px(32.))
                .h(px(32.))
                .disabled(prev_disabled)
                .on_click(move |_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        if s.levilauncher_page_index > 0 {
                            s.levilauncher_page_index -= 1;
                            s.levilauncher_scroll.set_offset(point(px(0.), px(0.)));
                        }
                    });
                }),
        )
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.text_primary)
                .child(format!("第 {} / {} 页", page_index + 1, total_pages)),
        )
        .child(
            IconButton::new("mod-next-page", lucide_icons::icon_chevron_right())
                .icon_color(colors.text_secondary)
                .w(px(32.))
                .h(px(32.))
                .disabled(next_disabled)
                .on_click(move |_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        if s.levilauncher_page_index + 1 < total_pages {
                            s.levilauncher_page_index += 1;
                            s.levilauncher_scroll.set_offset(point(px(0.), px(0.)));
                        }
                    });
                }),
        )
}

pub(super) fn render_detail_modal_content(
    colors: &ThemeColors,
    cx: &App,
    mod_entry: &LeviLaminaModEntry,
) -> Div {
    let mod_id = mod_entry.package_id.clone();
    let mod_name = mod_entry.name.clone();
    let mod_desc = mod_entry.description.clone();
    let mod_updated = mod_entry.updated_at.clone();
    let mod_stars = mod_entry.stargazer_count;

    let versions = mod_entry.client_versions.clone();
    let selected_ver = cx.read_global(|state: &DownloadPageState, _cx| {
        state.levilauncher_selected_version.to_string()
    });

    let current_ver = if selected_ver.is_empty() {
        versions.first().cloned().unwrap_or_default()
    } else {
        selected_ver.clone()
    };

    let deps = mod_entry
        .version_dependencies
        .get(&current_ver)
        .cloned()
        .unwrap_or_default();

    let mut version_options: Vec<DropdownOption> = Vec::with_capacity(versions.len());
    for v in &versions {
        version_options.push(DropdownOption::from(SharedString::from(v.clone())));
    }

    let selected_version_index = versions.iter().position(|v| v == &current_ver).unwrap_or(0);

    let versions_for_closure = versions.clone();

    let version_select = Dropdown::new(
        "mod-modal-version-dropdown",
        colors,
        px(180.),
        SharedString::from(current_ver.clone()),
        version_options,
        selected_version_index,
        true,
        move |ix, _window, cx| {
            if let Some(ver) = versions_for_closure.get(ix) {
                let ver_str = ver.clone();
                cx.update_global(|s: &mut DownloadPageState, _cx| {
                    s.levilauncher_selected_version = SharedString::from(ver_str);
                });
            }
        },
    )
    .with_height(px(32.))
    .rounded(px(6.))
    .into_any_element();

    let lip_cmd = format!("lip install {}@{}", mod_id, current_ver);

    let header = div()
        .w_full()
        .p(px(20.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .flex()
        .items_start()
        .justify_between()
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(16.))
                .child(if !mod_entry.avatar_url.trim().is_empty() {
                    img(mod_entry.avatar_url.clone())
                        .w(px(52.))
                        .h(px(52.))
                        .rounded(px(12.))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element()
                } else {
                    div()
                        .w(px(52.))
                        .h(px(52.))
                        .rounded(px(12.))
                        .bg(Hsla {
                            a: 0.1,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(lucide_icons::icon_layers())
                                .size(px(26.))
                                .text_color(colors.accent),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(mod_name),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child(mod_id.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .pt(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .child(
                                            svg()
                                                .path(lucide_icons::icon_star())
                                                .size(px(13.))
                                                .text_color(colors.accent),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(colors.text_secondary)
                                                .child(format!("{} stars", mod_stars)),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_muted)
                                        .child(format!("更新: {}", mod_updated)),
                                ),
                        ),
                ),
        )
        .child(
            IconButton::new("mod-modal-header-close", lucide_icons::icon_x())
                .icon_color(colors.text_muted)
                .w(px(28.))
                .h(px(28.))
                .on_click(|_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_modal_open = false;
                        s.levilauncher_selected_mod = None;
                    });
                }),
        );

    let body = div()
        .flex_1()
        .overflow_y_scrollbar()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .p(px(12.))
                .rounded(px(8.))
                .bg(Hsla {
                    a: 0.04,
                    ..colors.text_primary
                })
                .text_size(px(13.))
                .text_color(colors.text_primary)
                .line_height(px(19.))
                .child(if mod_desc.trim().is_empty() {
                    "无详细描述".to_string()
                } else {
                    mod_desc
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("版本与依赖"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child("选择版本:"),
                        )
                        .child(version_select),
                ),
        )
        .child(
            div()
                .p(px(12.))
                .rounded(px(8.))
                .bg(Hsla {
                    a: 0.08,
                    ..colors.warning
                })
                .text_size(px(12.))
                .text_color(colors.warning)
                .child("当前仅支持显示 LeviLamina 模组信息，暂不支持在启动器内安装。"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_muted)
                        .child("该版本依赖项:"),
                )
                .child(render_dependencies_list(colors, &deps)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.text_muted)
                        .child("LIP 参考指令（需使用 LIP 工具执行）:"),
                )
                .child(
                    div()
                        .p(px(10.))
                        .rounded(px(6.))
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.settings_card_bg
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.1,
                            ..colors.border
                        })
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.accent)
                        .child(lip_cmd.clone()),
                ),
        );

    let lip_cmd_copy = lip_cmd;
    let footer = div()
        .px(px(20.))
        .py(px(14.))
        .border_t_1()
        .border_color(Hsla {
            a: 0.08,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.02,
            ..colors.surface
        })
        .flex()
        .items_center()
        .justify_end()
        .gap(px(10.))
        .child(
            Button::new("mod-modal-close")
                .label("关闭")
                .bg(Hsla {
                    a: 0.08,
                    ..colors.text_primary
                })
                .text_color(colors.text_primary)
                .on_click(|_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_modal_open = false;
                        s.levilauncher_selected_mod = None;
                    });
                }),
        )
        .child(
            Button::new("mod-modal-copy-lip")
                .label("复制 LIP 参考指令")
                .bg(colors.accent)
                .text_color(hsla(0.0, 0.0, 1.0, 1.0))
                .on_click(move |_ev, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(lip_cmd_copy.clone()));
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_modal_open = false;
                        s.levilauncher_selected_mod = None;
                    });
                }),
        );

    div()
        .w(px(540.))
        .max_h(px(560.))
        .bg(colors.surface)
        .rounded(px(12.))
        .border_1()
        .border_color(colors.border)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(body)
        .child(footer)
}

fn render_dependencies_list(
    colors: &ThemeColors,
    deps: &std::collections::HashMap<String, String>,
) -> AnyElement {
    if deps.is_empty() {
        return div()
            .text_size(px(12.))
            .text_color(colors.text_muted)
            .child("无依赖项")
            .into_any_element();
    }

    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .max_h(px(180.))
        .overflow_y_scrollbar();

    for (dep_name, dep_req) in deps {
        list = list.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.))
                .py(px(4.))
                .rounded(px(4.))
                .bg(Hsla {
                    a: 0.04,
                    ..colors.text_primary
                })
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_primary)
                        .child(dep_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(colors.accent)
                        .child(dep_req.clone()),
                ),
        );
    }

    list.into_any_element()
}

use crate::core::levilamina::{LeviLaminaModEntry, mod_matches_loader_version};
use crate::ui::components::button::{Button, IconButton};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;

type ModPanelRenderSignature = (
    usize,
    String,
    String,
    SharedString,
    SharedString,
    SharedString,
    usize,
    usize,
);

#[derive(Default)]
struct ModPanelRenderCache {
    last_signature: Option<ModPanelRenderSignature>,
    total_mods: usize,
    total_pages: usize,
    page_index: usize,
    page_mods: Vec<LeviLaminaModEntry>,
}

fn build_mod_panel_render_signature(state: &DownloadPageState) -> ModPanelRenderSignature {
    (
        state.levilauncher_all_mods.len(),
        state
            .levilauncher_all_mods
            .first()
            .map(|m| m.package_id.clone())
            .unwrap_or_default(),
        state
            .levilauncher_all_mods
            .last()
            .map(|m| m.package_id.clone())
            .unwrap_or_default(),
        state.search_query.clone(),
        state.levilauncher_selected_loader.clone(),
        state.levilauncher_selected_loader_version.clone(),
        state.levilauncher_page_index,
        state.levilauncher_page_size,
    )
}

fn contains_ignore_ascii_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    let needle = needle_lower.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

fn rebuild_mod_panel_render_cache(
    state: &DownloadPageState,
    signature: ModPanelRenderSignature,
) -> ModPanelRenderCache {
    let query = state.search_query.trim().to_ascii_lowercase();
    let loader_type = state.levilauncher_selected_loader.as_ref();
    let loader_ver = state.levilauncher_selected_loader_version.as_ref();

    let filtered_mods: Vec<&LeviLaminaModEntry> = state
        .levilauncher_all_mods
        .iter()
        .filter(|m| {
            if !query.is_empty()
                && !contains_ignore_ascii_case(&m.name, &query)
                && !contains_ignore_ascii_case(&m.description, &query)
                && !contains_ignore_ascii_case(&m.package_id, &query)
            {
                return false;
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
        filtered_mods[start_idx..end_idx]
            .iter()
            .map(|m| (*m).clone())
            .collect()
    } else {
        Vec::new()
    };

    ModPanelRenderCache {
        last_signature: Some(signature),
        total_mods,
        total_pages,
        page_index,
        page_mods,
    }
}

pub(super) fn render_mod_panel(window: &mut Window, cx: &mut App, colors: &ThemeColors) -> Div {
    let i18n = cx.global::<I18n>().clone();
    {
        let state = cx.global::<DownloadPageState>();
        if state.levilauncher_loading && !state.levilauncher_loaded {
            return render_loading_state(colors, &i18n);
        }

        if let Some(err) = state.levilauncher_error.clone() {
            return render_error_state(colors, &err, &i18n);
        }
    }

    let cache = window.use_keyed_state("download-mod-panel-cache", cx, |_, _| {
        ModPanelRenderCache::default()
    });
    let render_signature =
        cx.read_global(|state: &DownloadPageState, _cx| build_mod_panel_render_signature(state));
    let cache_needs_rebuild = cache.read(cx).last_signature.as_ref() != Some(&render_signature);
    if cache_needs_rebuild {
        let rebuilt_cache = cx.read_global(|state: &DownloadPageState, _cx| {
            rebuild_mod_panel_render_cache(state, render_signature.clone())
        });
        cache.update(cx, |cached, _| {
            *cached = rebuilt_cache;
        });
    }

    let state = cx.global::<DownloadPageState>();
    let cached = cache.read(cx);
    let total_mods = cached.total_mods;
    let total_pages = cached.total_pages;
    let page_index = cached.page_index;
    let loader_type = state.levilauncher_selected_loader.as_ref();
    let loader_ver = state.levilauncher_selected_loader_version.as_ref();

    let main_content = if total_mods == 0 {
        render_empty_state(colors, &i18n)
    } else {
        render_mod_grid(colors, &cached.page_mods, state, &i18n)
    };

    let stats_bar = render_stats_bar(colors, total_mods, loader_type, loader_ver, &i18n);
    let pagination = render_pagination(colors, page_index, total_pages, total_mods, &i18n);

    div()
        .size_full()
        .flex()
        .flex_col()
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

fn render_loading_state(colors: &ThemeColors, i18n: &I18n) -> Div {
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
                .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                .child(t!("LeviLaminaMods.loading")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(t!("LeviLaminaMods.source")),
        )
}

fn render_error_state(colors: &ThemeColors, err: &SharedString, i18n: &I18n) -> Div {
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
                .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                .child(t!("LeviLaminaMods.load_failed")),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_muted)
                .child(err_str),
        )
        .child(
            Button::new("retry-fetch-mods")
                .label(t!("LeviLaminaMods.reload"))
                .bg(colors.accent)
                .text_color(colors.btn_primary_text)
                .on_click(|_ev, _window, cx| {
                    cx.update_global(|s: &mut DownloadPageState, _cx| {
                        s.levilauncher_loaded = false;
                        s.levilauncher_loading = false;
                        s.levilauncher_error = None;
                    });
                }),
        )
}

fn render_empty_state(colors: &ThemeColors, i18n: &I18n) -> Div {
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
                .child(t!("LeviLaminaMods.empty")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(t!("LeviLaminaMods.empty_hint")),
        )
}

fn render_stats_bar(
    colors: &ThemeColors,
    total_mods: usize,
    loader_type: &str,
    loader_ver: &str,
    i18n: &I18n,
) -> Div {
    let all_filter = loader_type.is_empty() || loader_type == "全部" || loader_type == "全部加载器";
    let all_versions = loader_ver.is_empty() || loader_ver == "全部版本" || loader_ver == "全部";
    let loader_filter_text = if all_filter {
        if all_versions {
            t!("LeviLaminaMods.filter_all")
        } else {
            t!("LeviLaminaMods.filter_all_loader", version = loader_ver)
        }
    } else if all_versions {
        SharedString::from(loader_type.to_owned())
    } else {
        t!(
            "LeviLaminaMods.filter_loader_version",
            loader = loader_type,
            version = loader_ver
        )
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
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.1,
                            ..colors.accent
                        })
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.accent)
                        .child(t!("LeviLaminaMods.count", count = total_mods)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child(t!(
                            "LeviLaminaMods.filter_current",
                            filter = loader_filter_text
                        )),
                ),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .child(t!("LeviLaminaMods.client_source")),
        )
}

fn render_mod_grid(
    colors: &ThemeColors,
    mods: &[LeviLaminaModEntry],
    _state: &DownloadPageState,
    i18n: &I18n,
) -> Div {
    let mut grid = div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(16.))
        .items_stretch();

    for (idx, mod_entry) in mods.iter().enumerate() {
        grid = grid.child(render_mod_card(colors, mod_entry, idx, i18n));
    }

    grid
}

fn render_mod_card(
    colors: &ThemeColors,
    mod_entry: &LeviLaminaModEntry,
    idx: usize,
    i18n: &I18n,
) -> AnyElement {
    let mod_clone = (*mod_entry).clone();
    let mod_clone_for_detail = mod_clone.clone();

    let avatar_element = if !mod_entry.avatar_url.trim().is_empty() {
        img(mod_entry.avatar_url.clone())
            .w(px(48.))
            .h(px(48.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    } else {
        div()
            .w(px(48.))
            .h(px(48.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
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

    div()
        .id(ElementId::NamedInteger("mod-card".into(), idx as u64))
        .w(px(320.))
        .flex_grow()
        .min_h(px(160.))
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .shadow(crate::ui::components::page_shell::card_shadow())
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
                a: 0.85,
                ..colors.surface
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
                            t!("LeviLaminaMods.no_description").to_string()
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
                                .rounded(px(crate::ui::theme::tokens::radius::XS))
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
                        .label(t!("LeviLaminaMods.details"))
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.accent
                        })
                        .text_color(colors.accent)
                        .on_click(move |_ev, _window, cx| {
                            let m = mod_clone_for_detail.clone();
                            super::mod_install::open_modal(m, cx);
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
    i18n: &I18n,
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
                .child(t!(
                    "LeviLaminaMods.page_info",
                    current = page_index + 1,
                    total = total_pages
                )),
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
    let i18n = cx.global::<I18n>();
    let mod_id = mod_entry.package_id.clone();
    let mod_name = mod_entry.name.clone();
    let mod_desc = mod_entry.description.clone();
    let mod_updated = mod_entry.updated_at.clone();
    let mod_stars = mod_entry.stargazer_count;

    let (targets, targets_loading, selected_target_path, selected_ver, install_busy, install_error) =
        cx.read_global(|state: &DownloadPageState, _cx| {
            (
                state.levilauncher_install_targets.clone(),
                state.levilauncher_install_targets_loading,
                state.levilauncher_install_target_path.clone(),
                state.levilauncher_selected_version.to_string(),
                state.levilauncher_install_busy,
                state.levilauncher_install_error.clone(),
            )
        });
    let selected_target_index = selected_target_path
        .as_ref()
        .and_then(|path| targets.iter().position(|target| &target.path == path))
        .unwrap_or(0);
    let selected_target = targets.get(selected_target_index);
    let versions = selected_target.map_or_else(Vec::new, |target| {
        super::mod_install::compatible_releases(mod_entry, target.loader_version.as_ref())
    });
    let current_ver = versions
        .iter()
        .find(|version| **version == selected_ver)
        .cloned()
        .or_else(|| versions.first().cloned())
        .unwrap_or_default();

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
        if targets_loading {
            t!("LeviLaminaMods.checking")
        } else if current_ver.is_empty() {
            t!("LeviLaminaMods.no_compatible_version")
        } else {
            SharedString::from(current_ver.clone())
        },
        version_options,
        selected_version_index,
        !versions.is_empty() && !install_busy,
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
    .rounded(px(crate::ui::theme::tokens::radius::SM))
    .into_any_element();

    let target_label = selected_target.map_or_else(
        || {
            SharedString::from(if targets_loading {
                t!("LeviLaminaMods.checking_instances")
            } else {
                t!("LeviLaminaMods.no_compatible_instances")
            })
        },
        |target| target.label.clone(),
    );
    let target_options = targets
        .iter()
        .map(|target| DropdownOption::from(target.label.clone()))
        .collect::<Vec<_>>();
    let targets_for_dropdown = targets.clone();
    let mod_for_target = mod_entry.clone();
    let target_select = Dropdown::new(
        "mod-modal-target-dropdown",
        colors,
        px(260.),
        target_label,
        target_options,
        selected_target_index,
        !targets_loading && !targets.is_empty() && !install_busy,
        move |index, _window, cx| {
            if let Some(target) = targets_for_dropdown.get(index) {
                let selected_version = super::mod_install::compatible_releases(
                    &mod_for_target,
                    target.loader_version.as_ref(),
                )
                .into_iter()
                .next()
                .unwrap_or_default();
                cx.update_global(|state: &mut DownloadPageState, _cx| {
                    state.levilauncher_install_target_path = Some(target.path.clone());
                    state.levilauncher_install_target_version = target.game_version.clone();
                    state.levilauncher_selected_version = SharedString::from(selected_version);
                    state.levilauncher_install_error = None;
                });
            }
        },
    )
    .with_height(px(32.))
    .rounded(px(crate::ui::theme::tokens::radius::SM))
    .into_any_element();

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
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element()
                } else {
                    div()
                        .w(px(52.))
                        .h(px(52.))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                                                .child(t!(
                                                    "LeviLaminaMods.stars",
                                                    count = &mod_stars
                                                )),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_muted)
                                        .child(t!("LeviLaminaMods.updated", date = &mod_updated)),
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
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(Hsla {
                    a: 0.04,
                    ..colors.text_primary
                })
                .text_size(px(13.))
                .text_color(colors.text_primary)
                .line_height(px(19.))
                .child(if mod_desc.trim().is_empty() {
                    t!("LeviLaminaMods.no_description").to_string()
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
                        .child(t!("LeviLaminaMods.version_dependencies")),
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
                                .child(t!("LeviLaminaMods.select_version")),
                        )
                        .child(version_select),
                ),
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
                        .child(t!("LeviLaminaMods.install_to_game")),
                )
                .child(target_select)
                .children(install_error.map(|error| {
                    div()
                        .text_size(px(12.))
                        .text_color(colors.danger)
                        .child(error)
                        .into_any_element()
                })),
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
                        .child(t!("LeviLaminaMods.dependencies")),
                )
                .child(render_dependencies_list(colors, &deps, i18n)),
        );

    let install_mod_id = mod_id;
    let install_mod_version = current_ver.clone();
    let can_install =
        !install_busy && !targets_loading && !targets.is_empty() && !current_ver.is_empty();
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
                .label(t!("common.close"))
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
            Button::new("mod-modal-install")
                .label(if install_busy {
                    t!("LeviLaminaMods.installing")
                } else {
                    t!("common.install")
                })
                .bg(colors.accent)
                .text_color(colors.btn_primary_text)
                .opacity(if can_install { 1.0 } else { 0.5 })
                .on_click(move |_ev, _window, cx| {
                    start_mod_install(cx, install_mod_id.clone(), install_mod_version.clone());
                }),
        );

    div()
        .w(px(540.))
        .max_h(px(560.))
        .bg(colors.surface)
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(header)
        .child(body)
        .child(footer)
}

fn start_mod_install(cx: &mut App, package_id: String, version: String) {
    let i18n = cx.global::<I18n>().clone();
    if version.trim().is_empty() {
        toast::error(cx, t!("LeviLaminaMods.no_version"));
        return;
    }
    let target = cx.read_global(|state: &DownloadPageState, _cx| {
        state
            .levilauncher_install_target_path
            .clone()
            .map(|path| (path, state.levilauncher_install_target_version.to_string()))
    });
    let Some((game_directory, game_version)) = target else {
        toast::error(cx, t!("LeviLaminaMods.select_game_version"));
        return;
    };
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.levilauncher_install_busy = true;
        state.levilauncher_install_error = None;
    });
    let request = crate::core::levilamina::LeviLaminaInstallRequest::Mod {
        game_directory: std::path::PathBuf::from(game_directory.as_ref()),
        game_version,
        package_id,
        version,
    };
    let handle = match crate::core::levilamina::start_install(request) {
        Ok(handle) => handle,
        Err(error) => {
            cx.update_global(|state: &mut DownloadPageState, _cx| {
                state.levilauncher_install_busy = false;
                state.levilauncher_install_error = Some(SharedString::from(error));
            });
            return;
        }
    };
    let mut updates = handle.updates;
    cx.spawn(async move |cx| {
        loop {
            let stage = updates.borrow_and_update().stage.clone();
            match stage {
                crate::core::levilamina::LeviLaminaInstallStage::Completed { message } => {
                    cx.update_global(|state: &mut DownloadPageState, cx| {
                        state.levilauncher_install_busy = false;
                        state.levilauncher_modal_open = false;
                        state.levilauncher_selected_mod = None;
                        toast::success(cx, SharedString::from(message.to_string()));
                    })?;
                    return Ok::<(), anyhow::Error>(());
                }
                crate::core::levilamina::LeviLaminaInstallStage::Failed { message } => {
                    cx.update_global(|state: &mut DownloadPageState, _cx| {
                        state.levilauncher_install_busy = false;
                        state.levilauncher_install_error =
                            Some(SharedString::from(message.to_string()));
                    })?;
                    return Ok(());
                }
                _ => {}
            }
            if updates.changed().await.is_err() {
                return Ok(());
            }
        }
    })
    .detach_and_log_err(cx);
}

fn render_dependencies_list(
    colors: &ThemeColors,
    deps: &std::collections::HashMap<String, String>,
    i18n: &I18n,
) -> AnyElement {
    if deps.is_empty() {
        return div()
            .text_size(px(12.))
            .text_color(colors.text_muted)
            .child(t!("LeviLaminaMods.no_dependencies"))
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
                .rounded(px(crate::ui::theme::tokens::radius::XS))
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

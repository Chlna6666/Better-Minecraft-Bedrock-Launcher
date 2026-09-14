use crate::core::native_mods::{NativeModEntry, NativeModInstallRequest};
use crate::ui::components::button::{Button, IconButton};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::local_versions::LocalVersionsState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::DownloadPageState;
use gpui::*;

fn contains_ignore_ascii_case(haystack: &str, needle_lower: &str) -> bool {
    needle_lower.is_empty()
        || haystack
            .as_bytes()
            .windows(needle_lower.len())
            .any(|window| window.eq_ignore_ascii_case(needle_lower.as_bytes()))
}

pub(super) fn render_panel(cx: &mut App, colors: &ThemeColors) -> Div {
    let state = cx.global::<DownloadPageState>();
    if state.native_mods_loading && !state.native_mods_loaded {
        return render_loading(colors);
    }
    if let Some(error) = state.native_mods_error.clone() {
        return render_error(colors, &error);
    }

    let query = state.search_query.trim().to_ascii_lowercase();
    let filtered = state
        .native_mods
        .iter()
        .filter(|entry| {
            query.is_empty()
                || contains_ignore_ascii_case(&entry.name, &query)
                || contains_ignore_ascii_case(&entry.description, &query)
                || contains_ignore_ascii_case(&entry.id, &query)
                || entry
                    .tags
                    .iter()
                    .any(|tag| contains_ignore_ascii_case(tag, &query))
        })
        .collect::<Vec<_>>();
    let page_size = state.levilauncher_page_size.max(1);
    let total_pages = (filtered.len() + page_size - 1) / page_size;
    let page_index = state
        .levilauncher_page_index
        .min(total_pages.saturating_sub(1));
    let start = page_index * page_size;
    let end = (start + page_size).min(filtered.len());
    let page = if start < end {
        &filtered[start..end]
    } else {
        &[]
    };

    let mut grid = div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(16.))
        .items_stretch();
    for (index, entry) in page.iter().enumerate() {
        grid = grid.child(render_card(colors, entry, index));
    }
    let content = if filtered.is_empty() {
        render_empty(colors)
    } else {
        grid
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(render_stats(colors, filtered.len()))
        .child(
            div()
                .flex_1()
                .overflow_y_scrollbar()
                .track_scroll(&state.levilauncher_scroll)
                .p(px(20.))
                .child(content),
        )
        .child(render_pagination(colors, page_index, total_pages))
}

fn render_loading(colors: &ThemeColors) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(
            svg()
                .path(lucide_gpui::icon!(refresh_cw))
                .size(px(32.))
                .text_color(colors.accent),
        )
        .child(
            div()
                .text_size(px(14.))
                .text_color(colors.text_muted)
                .child(t!("NativeMods.loading")),
        )
}

fn render_error(colors: &ThemeColors, error: &SharedString) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(15.))
                .text_color(colors.danger)
                .child(t!("NativeMods.load_failed")),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(error.clone()),
        )
        .child(
            Button::new("retry-native-mods")
                .label(t!("NativeMods.reload"))
                .bg(colors.accent)
                .text_color(colors.btn_primary_text)
                .on_click(|_, _, cx| {
                    cx.update_global(|state: &mut DownloadPageState, _| {
                        state.native_mods_loaded = false;
                        state.native_mods_loading = false;
                        state.native_mods_error = None;
                    });
                }),
        )
}

fn render_empty(colors: &ThemeColors) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(15.))
                .text_color(colors.text_primary)
                .child(t!("NativeMods.empty")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(t!("NativeMods.empty_hint")),
        )
}

fn render_stats(colors: &ThemeColors, count: usize) -> Div {
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
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child(t!("NativeMods.count", count = count)),
        )
}

fn render_card(colors: &ThemeColors, entry: &NativeModEntry, index: usize) -> AnyElement {
    let entry_for_click = entry.clone();
    let file_names = entry.files.keys().cloned().collect::<Vec<_>>();
    let file_label = file_names.join(", ");
    div()
        .id(ElementId::NamedInteger(
            "native-mod-card".into(),
            index as u64,
        ))
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
        .p(px(14.))
        .flex()
        .flex_col()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(entry.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(entry.repository.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .line_height(px(17.))
                        .max_h(px(34.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(if entry.description.trim().is_empty() {
                            t!("NativeMods.no_description").to_string()
                        } else {
                            entry.description.clone()
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
                        .text_size(px(10.))
                        .text_color(colors.text_muted)
                        .child(file_label),
                )
                .child(
                    Button::new(ElementId::NamedInteger(
                        "native-mod-detail".into(),
                        index as u64,
                    ))
                    .label(t!("NativeMods.details"))
                    .bg(Hsla {
                        a: 0.08,
                        ..colors.accent
                    })
                    .text_color(colors.accent)
                    .on_click(move |_, _, cx| open_modal(entry_for_click.clone(), cx)),
                ),
        )
        .into_any_element()
}

fn render_pagination(colors: &ThemeColors, page_index: usize, total_pages: usize) -> Div {
    if total_pages <= 1 {
        return div();
    }
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
            IconButton::new("native-mod-prev-page", lucide_gpui::icon!(chevron_left))
                .icon_color(colors.text_secondary)
                .disabled(page_index == 0)
                .on_click(|_, _, cx| {
                    cx.update_global(|state: &mut DownloadPageState, _| {
                        state.levilauncher_page_index =
                            state.levilauncher_page_index.saturating_sub(1);
                    });
                }),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(colors.text_primary)
                .child(t!(
                    "NativeMods.page_info",
                    current = page_index + 1,
                    total = total_pages
                )),
        )
        .child(
            IconButton::new("native-mod-next-page", lucide_gpui::icon!(chevron_right))
                .icon_color(colors.text_secondary)
                .disabled(page_index + 1 >= total_pages)
                .on_click(move |_, _, cx| {
                    cx.update_global(|state: &mut DownloadPageState, _| {
                        if state.levilauncher_page_index + 1 < total_pages {
                            state.levilauncher_page_index += 1;
                        }
                    });
                }),
        )
}

pub(super) fn open_modal(entry: NativeModEntry, cx: &mut App) {
    let first_file = entry.files.keys().next().cloned().unwrap_or_default();
    let target = install_targets(cx).first().cloned();
    cx.update_global(|state: &mut DownloadPageState, _| {
        state.native_mod_modal_open = true;
        state.native_mod_selected = Some(entry);
        state.native_mod_selected_file = SharedString::from(first_file);
        state.native_mod_target_path = target.as_ref().map(|(path, _, _)| path.clone());
        state.native_mod_target_version =
            target.map_or_else(|| SharedString::from(""), |(_, version, _)| version);
        state.native_mod_install_busy = false;
        state.native_mod_install_error = None;
    });
}

fn install_targets(cx: &App) -> Vec<(SharedString, SharedString, SharedString)> {
    cx.read_global(|state: &LocalVersionsState, _| {
        state
            .versions
            .iter()
            .map(|version| {
                let folder = version.folder.to_string();
                let game_version = version.version.to_string();
                let label = if folder.is_empty() {
                    game_version.clone()
                } else if game_version.is_empty() {
                    folder.clone()
                } else {
                    format!("{folder} ({game_version})")
                };
                (
                    SharedString::from(version.path.to_string()),
                    SharedString::from(game_version),
                    SharedString::from(label),
                )
            })
            .collect()
    })
}

pub(super) fn dismiss_modal(cx: &mut App) {
    cx.update_global(|state: &mut DownloadPageState, _| {
        state.native_mod_modal_open = false;
        state.native_mod_selected = None;
        state.native_mod_install_error = None;
    });
}

pub(super) fn render_detail_modal_content(
    colors: &ThemeColors,
    cx: &App,
    entry: &NativeModEntry,
) -> Div {
    let (selected_file, target_path, install_busy, install_error) =
        cx.read_global(|state: &DownloadPageState, _| {
            (
                state.native_mod_selected_file.clone(),
                state.native_mod_target_path.clone(),
                state.native_mod_install_busy,
                state.native_mod_install_error.clone(),
            )
        });
    let targets = install_targets(cx);
    let selected_target_index = target_path
        .as_ref()
        .and_then(|path| {
            targets
                .iter()
                .position(|(candidate, _, _)| candidate == path)
        })
        .unwrap_or(0);
    let target_label = targets
        .get(selected_target_index)
        .map(|(_, _, label)| label.clone())
        .unwrap_or_else(|| t!("NativeMods.no_game_version"));
    let target_options = targets
        .iter()
        .map(|(_, _, label)| DropdownOption::from(label.clone()))
        .collect::<Vec<_>>();
    let targets_for_dropdown = targets.clone();
    let target_select = Dropdown::new(
        "native-mod-target-dropdown",
        colors,
        px(260.),
        target_label,
        target_options,
        selected_target_index,
        !install_busy && !targets.is_empty(),
        move |index, _, cx| {
            if let Some((path, version, _)) = targets_for_dropdown.get(index) {
                cx.update_global(|state: &mut DownloadPageState, _| {
                    state.native_mod_target_path = Some(path.clone());
                    state.native_mod_target_version = version.clone();
                    state.native_mod_install_error = None;
                });
            }
        },
    )
    .with_height(px(32.))
    .rounded(px(crate::ui::theme::tokens::radius::SM));

    let files = entry.files.keys().cloned().collect::<Vec<_>>();
    let file_index = files
        .iter()
        .position(|file| file == &selected_file)
        .unwrap_or(0);
    let file_label = files
        .get(file_index)
        .cloned()
        .unwrap_or_else(|| t!("NativeMods.no_file").to_string());
    let file_options = files
        .iter()
        .cloned()
        .map(SharedString::from)
        .map(DropdownOption::from)
        .collect::<Vec<_>>();
    let files_for_dropdown = files.clone();
    let file_select = Dropdown::new(
        "native-mod-file-dropdown",
        colors,
        px(260.),
        SharedString::from(file_label),
        file_options,
        file_index,
        !install_busy && !files.is_empty(),
        move |index, _, cx| {
            if let Some(file) = files_for_dropdown.get(index) {
                cx.update_global(|state: &mut DownloadPageState, _| {
                    state.native_mod_selected_file = SharedString::from(file.clone());
                });
            }
        },
    )
    .with_height(px(32.))
    .rounded(px(crate::ui::theme::tokens::radius::SM));

    let target_is_available = target_path.is_some() && !targets.is_empty();
    let can_install = !install_busy && target_is_available && !files.is_empty();
    let entry_for_install = entry.clone();
    let selected_file_for_install = selected_file.to_string();
    div()
        .w(px(540.))
        .max_h(px(520.))
        .bg(colors.surface)
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div()
                .p(px(20.))
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
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(entry.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_muted)
                                .child(entry.repository.clone()),
                        ),
                )
                .child(
                    IconButton::new("native-mod-modal-close", lucide_gpui::icon!(x))
                        .icon_color(colors.text_muted)
                        .on_click(|_, _, cx| dismiss_modal(cx)),
                ),
        )
        .child(
            div()
                .flex_1()
                .overflow_y_scrollbar()
                .p(px(20.))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_primary)
                        .line_height(px(19.))
                        .child(if entry.description.is_empty() {
                            t!("NativeMods.no_description").to_string()
                        } else {
                            entry.description.clone()
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
                                .child(t!("NativeMods.select_file")),
                        )
                        .child(file_select),
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
                                .child(t!("NativeMods.install_to_game")),
                        )
                        .child(target_select)
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(if target_is_available {
                                    colors.text_muted
                                } else {
                                    colors.danger
                                })
                                .child(if target_is_available {
                                    t!("NativeMods.install_target_hint")
                                } else {
                                    t!("NativeMods.no_game_version")
                                }),
                        )
                        .children(install_error.map(|error| {
                            div()
                                .text_size(px(12.))
                                .text_color(colors.danger)
                                .child(error)
                                .into_any_element()
                        })),
                ),
        )
        .child(
            div()
                .px(px(20.))
                .py(px(14.))
                .border_t_1()
                .border_color(Hsla {
                    a: 0.08,
                    ..colors.border
                })
                .flex()
                .items_center()
                .justify_end()
                .gap(px(10.))
                .child(
                    Button::new("native-mod-modal-close-button")
                        .label(t!("common.close"))
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.text_primary
                        })
                        .text_color(colors.text_primary)
                        .on_click(|_, _, cx| dismiss_modal(cx)),
                )
                .child(
                    Button::new("native-mod-modal-install")
                        .label(if install_busy {
                            t!("NativeMods.installing")
                        } else {
                            t!("common.install")
                        })
                        .bg(colors.accent)
                        .text_color(colors.btn_primary_text)
                        .opacity(if can_install { 1.0 } else { 0.5 })
                        .on_click(move |_, _, cx| {
                            start_install(
                                cx,
                                entry_for_install.clone(),
                                selected_file_for_install.clone(),
                            );
                        }),
                ),
        )
}

fn start_install(cx: &mut App, entry: NativeModEntry, file_name: String) {
    let target =
        cx.read_global(|state: &DownloadPageState, _| state.native_mod_target_path.clone());
    let Some(game_directory) = target else {
        cx.update_global(|state: &mut DownloadPageState, _| {
            state.native_mod_install_error = Some(t!("NativeMods.no_game_version"));
        });
        return;
    };
    cx.update_global(|state: &mut DownloadPageState, _| {
        state.native_mod_install_busy = true;
        state.native_mod_install_error = None;
    });
    let request = NativeModInstallRequest {
        game_directory: std::path::PathBuf::from(game_directory.as_ref()),
        mod_entry: entry,
        file_name,
    };
    match crate::core::native_mods::start_install(request) {
        Ok(_) => dismiss_modal(cx),
        Err(error) => {
            cx.update_global(|state: &mut DownloadPageState, _| {
                state.native_mod_install_busy = false;
                state.native_mod_install_error = Some(SharedString::from(error));
            });
        }
    }
}

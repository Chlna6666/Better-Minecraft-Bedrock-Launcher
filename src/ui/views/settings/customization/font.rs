use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::toast::{self, ToastKind};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::common::{
    settings_action_button, settings_badge, settings_card, settings_card_header,
    settings_option_row_shell, settings_value_box, snapshot_from_state,
    spawn_persist_settings_with_success,
};
use crate::ui::views::settings::state::SettingsPageState;
use crate::utils::file_picker::pick_font_path;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;
use tracing::warn;

pub(super) fn render_font_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    system_font_names: &[String],
) -> impl IntoElement {
    let custom_option_label = custom_font_option_label(i18n, state);
    let font_names = dropdown_font_names(system_font_names, custom_option_label.as_ref());
    let selected_index = selected_font_index(&font_names, state);
    let selected_label = font_dropdown_label(&font_names, selected_index);
    let options = font_names
        .iter()
        .cloned()
        .map(|font| DropdownOption::from(SharedString::from(font)))
        .collect::<Vec<_>>();
    let font_desc = SharedString::from(format!(
        "{} {}",
        i18n.t("CustomizationSettings.font_desc").as_ref(),
        crate::utils::font_settings::default_app_font_display()
    ));

    settings_card(colors, "settings-custom-font")
        .child(
            settings_card_header(colors, i18n.t("CustomizationSettings.font"), font_desc)
                .child(settings_badge(colors, current_font_label(i18n, state))),
        )
        .child(system_font_row(
            colors,
            i18n.t("CustomizationSettings.system_font"),
            i18n.t("CustomizationSettings.system_font_desc"),
            selected_label,
            options,
            selected_index,
            font_names,
            custom_option_label,
            state.font_source.as_ref() != crate::config::config::FONT_SOURCE_LOCAL,
        ))
        .when(
            state.font_source.as_ref() == crate::config::config::FONT_SOURCE_LOCAL,
            |this| {
                this.child(local_font_row(
                    colors,
                    i18n.t("CustomizationSettings.local_font"),
                    i18n.t("CustomizationSettings.local_font_desc"),
                    local_font_display(i18n, state),
                    i18n.t("CustomizationSettings.select_font"),
                    state.font_source.as_ref() == crate::config::config::FONT_SOURCE_LOCAL,
                ))
            },
        )
}

fn local_font_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    display: SharedString,
    button_label: SharedString,
    active: bool,
) -> Div {
    settings_option_row_shell(colors, label, desc, active).child(
        div()
            .w(px(420.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .child(settings_value_box(colors, display, active, px(300.)))
            .child(
                settings_action_button(colors, button_label, true).on_mouse_up(
                    MouseButton::Left,
                    move |_, window, cx| {
                        window.defer(cx, |_window, cx| {
                            cx.spawn(async move |cx| {
                                let settings_loaded = cx
                                    .read_global(|state: &SettingsPageState, _cx| state.loaded)
                                    .unwrap_or(false);
                                if !settings_loaded {
                                    warn!(
                                        "skip picking font before settings page finishes loading"
                                    );
                                    return;
                                }

                                let selected = tokio::task::spawn_blocking(pick_font_path).await;
                                let path = match selected {
                                    Ok(Some(path)) => path,
                                    Ok(None) => return,
                                    Err(error) => {
                                        warn!("pick font join error: {error}");
                                        return;
                                    }
                                };

                                let font_family = tokio::task::spawn_blocking({
                                    let path = path.clone();
                                    move || {
                                        crate::utils::font_settings::read_local_font_family(&path)
                                    }
                                })
                                .await;
                                let family = match font_family {
                                    Ok(Ok(family)) => family,
                                    Ok(Err(error)) => {
                                        toast::push_async(
                                            cx,
                                            ToastKind::Error,
                                            SharedString::from(format!("读取字体失败: {error:#}")),
                                        );
                                        return;
                                    }
                                    Err(error) => {
                                        warn!("read font join error: {error}");
                                        return;
                                    }
                                };

                                let snapshot = cx.update({
                                    let path = path.clone();
                                    let family = family.clone();
                                    move |cx| {
                                        cx.update_global(|state: &mut SettingsPageState, _cx| {
                                            state.font_source = SharedString::from(
                                                crate::config::config::FONT_SOURCE_LOCAL,
                                            );
                                            state.local_font_path = SharedString::from(path);
                                            state.local_font_family = SharedString::from(family);
                                            snapshot_from_state(state)
                                        })
                                    }
                                });

                                let snapshot = match snapshot {
                                    Ok(snapshot) => snapshot,
                                    Err(error) => {
                                        warn!("update picked font state failed: {error:?}");
                                        return;
                                    }
                                };

                                if let Err(error) = cx.update(move |cx| {
                                    persist_font_settings_and_prompt_restart(snapshot, cx);
                                }) {
                                    warn!("persist picked font schedule failed: {error:?}");
                                }
                            })
                            .detach();
                        });
                    },
                ),
            ),
    )
}

fn system_font_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    selected_label: SharedString,
    options: Vec<DropdownOption>,
    selected_index: usize,
    font_names: Vec<String>,
    custom_option_label: SharedString,
    active: bool,
) -> Div {
    settings_option_row_shell(colors, label, desc, active).child(
        div()
            .w(px(420.))
            .flex()
            .items_center()
            .justify_end()
            .child(Dropdown::new(
                SharedString::from("settings-system-font-dropdown"),
                colors,
                px(300.),
                selected_label,
                options,
                selected_index,
                true,
                move |index, _window, cx| {
                    let Some(selected) = font_names.get(index).cloned() else {
                        return;
                    };

                    if selected == crate::utils::font_settings::default_app_font_display() {
                        let snapshot = cx.update_global(|state: &mut SettingsPageState, _cx| {
                            state.font_source =
                                SharedString::from(crate::config::config::FONT_SOURCE_DEFAULT);
                            snapshot_from_state(state)
                        });
                        persist_font_settings_and_prompt_restart(snapshot, cx);
                        return;
                    }

                    if selected == custom_option_label.as_ref() {
                        let (local_font_path, local_font_family) =
                            cx.read_global(|state: &SettingsPageState, _cx| {
                                (
                                    state.local_font_path.to_string(),
                                    state.local_font_family.to_string(),
                                )
                            });

                        let has_local_font = !local_font_path.trim().is_empty();

                        let snapshot = cx.update_global(|state: &mut SettingsPageState, _cx| {
                            state.font_source =
                                SharedString::from(crate::config::config::FONT_SOURCE_LOCAL);
                            snapshot_from_state(state)
                        });
                        if has_local_font {
                            persist_font_settings_and_prompt_restart(snapshot, cx);
                        } else {
                            cx.refresh_windows();
                        }
                        return;
                    }

                    let snapshot = cx.update_global(|state: &mut SettingsPageState, _cx| {
                        state.font_source =
                            SharedString::from(crate::config::config::FONT_SOURCE_SYSTEM);
                        state.system_font_family = SharedString::from(selected.clone());
                        snapshot_from_state(state)
                    });
                    persist_font_settings_and_prompt_restart(snapshot, cx);
                },
            )),
    )
}

fn persist_font_settings_and_prompt_restart(
    snapshot: crate::ui::views::settings::common::SettingsSnapshot,
    cx: &mut App,
) {
    spawn_persist_settings_with_success(
        snapshot,
        Some(Rc::new(|cx| {
            cx.update_global(|state: &mut SettingsPageState, _cx| {
                state.open_font_restart_confirm();
            });
        })),
        cx,
    );
}

fn custom_font_option_label(i18n: &I18n, state: &SettingsPageState) -> SharedString {
    let family = state.local_font_family.as_ref().trim();
    if family.is_empty() {
        i18n.t("CustomizationSettings.local_font")
    } else {
        SharedString::from(format!(
            "{}({family})",
            i18n.t("CustomizationSettings.local_font").as_ref()
        ))
    }
}

fn dropdown_font_names(system_font_names: &[String], custom_option_label: &str) -> Vec<String> {
    let mut fonts = system_font_names
        .iter()
        .filter_map(|font| non_empty(font))
        .collect::<Vec<_>>();

    fonts.sort_by_key(|font| font.to_ascii_lowercase());
    fonts.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    fonts.retain(|font| {
        !font.eq_ignore_ascii_case(&crate::utils::font_settings::default_app_font_display())
            && !font.eq_ignore_ascii_case(custom_option_label)
    });
    fonts.insert(0, custom_option_label.to_string());
    fonts.insert(0, crate::utils::font_settings::default_app_font_display());

    fonts
}

fn selected_font_index(font_names: &[String], state: &SettingsPageState) -> usize {
    match state.font_source.as_ref() {
        crate::config::config::FONT_SOURCE_DEFAULT => return 0,
        crate::config::config::FONT_SOURCE_LOCAL => {
            return 1.min(font_names.len().saturating_sub(1));
        }
        crate::config::config::FONT_SOURCE_SYSTEM => {}
        _ => return 0,
    }

    let target = state.system_font_family.as_ref().trim();
    if target.is_empty() {
        return usize::MAX;
    }

    font_names
        .iter()
        .position(|font| font.eq_ignore_ascii_case(target))
        .unwrap_or(usize::MAX)
}

fn font_dropdown_label(font_names: &[String], selected_index: usize) -> SharedString {
    font_names
        .get(selected_index)
        .cloned()
        .map(SharedString::from)
        .unwrap_or_else(|| {
            SharedString::from(crate::utils::font_settings::default_app_font_display())
        })
}

fn current_font_label(i18n: &I18n, state: &SettingsPageState) -> SharedString {
    match state.font_source.as_ref() {
        crate::config::config::FONT_SOURCE_LOCAL => {
            let family = state.local_font_family.as_ref().trim();
            if !family.is_empty() {
                SharedString::from(format!(
                    "{}: {}",
                    i18n.t("CustomizationSettings.local_font").as_ref(),
                    family
                ))
            } else {
                i18n.t("CustomizationSettings.local_font")
            }
        }
        crate::config::config::FONT_SOURCE_SYSTEM => {
            let family = state.system_font_family.as_ref().trim();
            if !family.is_empty() {
                SharedString::from(format!(
                    "{}: {}",
                    i18n.t("CustomizationSettings.system_font").as_ref(),
                    family
                ))
            } else {
                i18n.t("CustomizationSettings.system_font")
            }
        }
        _ => SharedString::from(crate::utils::font_settings::default_app_font_display()),
    }
}

fn local_font_display(i18n: &I18n, state: &SettingsPageState) -> SharedString {
    let path = state.local_font_path.as_ref().trim();
    if path.is_empty() {
        return i18n.t("CustomizationSettings.no_font");
    }

    let family = state.local_font_family.as_ref().trim();
    if family.is_empty() {
        SharedString::from(path.to_string())
    } else {
        SharedString::from(format!("{family} · {path}"))
    }
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let trimmed = value.as_ref().trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

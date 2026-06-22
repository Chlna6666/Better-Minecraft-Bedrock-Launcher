use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::input::{Input, InputState};
use crate::ui::components::slider::Slider;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use crate::utils::file_picker::pick_background_image_path;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use tracing::warn;

use crate::ui::views::settings::common::{
    settings_action_button, settings_badge, settings_card, settings_card_header,
    settings_control_box, settings_option_row_shell, settings_value_box, snapshot_from_state,
    spawn_persist_background_blur, spawn_persist_settings,
};

const BACKGROUND_BLUR_STEP: f32 = 0.1;

fn snap_background_blur(value: f32) -> f32 {
    crate::config::config::clamp_background_blur(
        (value / BACKGROUND_BLUR_STEP).round() * BACKGROUND_BLUR_STEP,
    )
}

pub(super) fn render_background_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    let options = vec![
        DropdownOption::from(i18n.t("CustomizationSettings.background_options.default")),
        DropdownOption::from(i18n.t("CustomizationSettings.background_options.local")),
        DropdownOption::from(i18n.t("CustomizationSettings.background_options.network")),
    ];
    let selected_index = match state.background_option.as_ref() {
        "local" => 1,
        "network" => 2,
        _ => 0,
    };
    let selected_label = options
        .get(selected_index)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| i18n.t("CustomizationSettings.background_options.default"));
    let current_source_label = selected_label.clone();
    let is_local = state.background_option.as_ref() == "local";
    let is_network = state.background_option.as_ref() == "network";

    settings_card(colors, "settings-custom-background")
        .child(
            settings_card_header(
                colors,
                i18n.t("CustomizationSettings.custom_background"),
                i18n.t("CustomizationSettings.custom_background_desc"),
            )
            .child(settings_badge(colors, current_source_label)),
        )
        .child(background_source_row(
            colors,
            i18n.t("CustomizationSettings.custom_background"),
            i18n.t("CustomizationSettings.custom_background_desc"),
            selected_label,
            options,
            selected_index,
            !is_local && !is_network,
        ))
        .child(background_blur_row(
            colors,
            i18n.t("CustomizationSettings.background_blur"),
            i18n.t("CustomizationSettings.background_blur_desc"),
            state.background_blur_preview,
        ))
        .when(is_local, |this| {
            this.child(local_picker_row(
                colors,
                i18n.t("CustomizationSettings.local_image"),
                i18n.t("CustomizationSettings.custom_background_desc"),
                state.local_image_path.clone(),
                i18n.t("CustomizationSettings.no_file"),
                i18n.t("CustomizationSettings.select_file"),
                true,
            ))
        })
        .when(is_network, |this| {
            let refresh_label = if state.network_image_refreshing {
                SharedString::from("刷新中")
            } else {
                SharedString::from("刷新")
            };
            this.child(network_input_row(
                colors,
                i18n.t("CustomizationSettings.network_image"),
                i18n.t("CustomizationSettings.custom_background_desc"),
                state.network_image_url_input.as_ref(),
                i18n.t("CustomizationSettings.network_placeholder"),
                refresh_label,
                state.network_image_refreshing,
                true,
            ))
        })
}

fn background_source_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    selected_label: SharedString,
    options: Vec<DropdownOption>,
    selected_index: usize,
    active: bool,
) -> Div {
    settings_option_row_shell(colors, label, desc, active).child(
        div()
            .w(px(420.))
            .flex()
            .items_center()
            .justify_end()
            .child(Dropdown::new(
                SharedString::from("settings-background-option-dropdown"),
                colors,
                px(300.),
                selected_label,
                options,
                selected_index,
                true,
                move |index, _window, cx| {
                    let option = match index {
                        1 => "local",
                        2 => "network",
                        _ => "default",
                    };
                    let snapshot = cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.commit_background_blur_preview();
                        state.background_option = SharedString::from(option);
                        if option != "network" {
                            state.network_image_refreshing = false;
                            state.network_image_refresh_started_at = None;
                            state.network_image_refresh_target_url = SharedString::from("");
                        }
                        snapshot_from_state(state)
                    });
                    spawn_persist_settings(snapshot, cx);
                },
            )),
    )
}

fn background_blur_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    value: f32,
) -> Div {
    let value = crate::config::config::clamp_background_blur(value);
    let value_label = SharedString::from(format!("{value:.1}px"));

    settings_option_row_shell(colors, label, desc, false).child(
        div()
            .w(px(420.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(10.))
            .child(
                div()
                    .w(px(56.))
                    .h(px(28.))
                    .rounded(px(9.))
                    .bg(Hsla {
                        a: 0.84,
                        ..colors.settings_field_bg
                    })
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_primary)
                            .child(value_label),
                    ),
            )
            .child(
                Slider::new(
                    "settings-background-blur-slider",
                    colors,
                    0.0,
                    crate::config::config::MAX_BACKGROUND_BLUR,
                    value,
                    move |next_value, cx| {
                        let blur = snap_background_blur(next_value);
                        cx.update_global(|state: &mut SettingsPageState, _cx| {
                            if !state.loaded
                                || (state.background_blur_preview - blur).abs() <= f32::EPSILON
                            {
                                return;
                            }
                            state.background_blur_preview = blur;
                        });
                    },
                )
                .on_commit(move |next_value, cx| {
                    let blur = snap_background_blur(next_value);
                    let should_persist = cx.update_global(|state: &mut SettingsPageState, _cx| {
                        if !state.loaded {
                            return false;
                        }
                        state.background_blur_preview = blur;
                        state.commit_background_blur_preview()
                    });
                    if should_persist {
                        spawn_persist_background_blur(blur, cx);
                    }
                })
                .width(px(300.)),
            ),
    )
}

fn local_picker_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    current_path: SharedString,
    placeholder: SharedString,
    button_label: SharedString,
    active: bool,
) -> Div {
    let display = if current_path.as_ref().trim().is_empty() {
        placeholder
    } else {
        current_path
    };

    settings_option_row_shell(colors, label, desc, active).child(
            div()
                .w(px(420.))
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.))
                .child(settings_value_box(colors, display, active, px(300.)))
                .child(
                    settings_action_button(colors, button_label, true)
                        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                            window.defer(cx, |_window, cx| {
                                cx.spawn(async move |cx| {
                                    let settings_loaded = cx
                                        .read_global(|state: &SettingsPageState, _cx| {
                                            state.loaded
                                        })
                                        .unwrap_or(false);
                                    if !settings_loaded {
                                        warn!(
                                            "skip picking background image before settings page finishes loading"
                                        );
                                        return;
                                    }

                                    let selected =
                                        tokio::task::spawn_blocking(pick_background_image_path)
                                            .await;
                                    let path = match selected {
                                        Ok(Some(path)) => path,
                                        Ok(None) => return,
                                        Err(error) => {
                                            warn!("pick background image join error: {error}");
                                            return;
                                        }
                                    };

                                    let blur = match cx.update_global(
                                        |state: &mut SettingsPageState, cx| {
                                            state.commit_background_blur_preview();
                                            state.local_image_path =
                                                SharedString::from(path.clone());
                                            state.background_option = SharedString::from("local");
                                            state.network_image_refreshing = false;
                                            state.network_image_refresh_started_at = None;
                                            state.network_image_refresh_target_url =
                                                SharedString::from("");
                                            state.background_blur
                                        },
                                    ) {
                                        Ok(blur) => blur,
                                        Err(error) => {
                                            warn!(
                                                "update settings after pick file failed: {error:?}"
                                            );
                                            return;
                                        }
                                    };

                                    let persist_result = tokio::task::spawn_blocking(move || {
                                        crate::config::config::update_config(|cfg| {
                                            cfg.custom_style.local_image_path = path;
                                            cfg.custom_style.background_option =
                                                "local".to_string();
                                            cfg.custom_style.background_blur =
                                                crate::config::config::clamp_background_blur(blur);
                                        })?;
                                        Ok::<(), std::io::Error>(())
                                    })
                                    .await;

                                    match persist_result {
                                        Err(error) => {
                                            warn!("persist picked file join error: {error}")
                                        }
                                        Ok(Err(error)) => {
                                            warn!("persist picked file failed: {error}")
                                        }
                                        Ok(Ok(())) => {}
                                    }
                                })
                                .detach();
                            });
                        }),
                ),
        )
}

fn network_input_row(
    colors: &ThemeColors,
    label: SharedString,
    desc: SharedString,
    input: Option<&Entity<InputState>>,
    placeholder: SharedString,
    refresh_label: SharedString,
    refresh_in_progress: bool,
    active: bool,
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

    settings_option_row_shell(colors, label, desc, active).child(
        div()
            .w(px(420.))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .child(settings_control_box(colors, active, px(320.), control))
            .child(settings_action_button(
                colors,
                refresh_label,
                !refresh_in_progress,
            )
            .on_mouse_up(MouseButton::Left, move |_, _window, cx| {
                cx.update_global(|state: &mut SettingsPageState, _cx| {
                    if state.network_image_refreshing {
                        tracing::debug!(
                            "network background refresh click ignored: already refreshing, target_url={}",
                            state.network_image_refresh_target_url
                        );
                        return;
                    }
                    state.background_option = SharedString::from("network");
                    state.network_image_refresh_nonce =
                        state.network_image_refresh_nonce.saturating_add(1);
                    state.network_image_refreshing = true;
                    state.network_image_refresh_started_at = Some(std::time::Instant::now());
                    state.network_image_refresh_target_url = state.network_image_url.clone();
                    tracing::info!(
                        "network background refresh started: nonce={}, target_url={}",
                        state.network_image_refresh_nonce,
                        state.network_image_refresh_target_url
                    );
                });
            })),
    )
}

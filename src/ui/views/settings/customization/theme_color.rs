use crate::ui::components::color_picker::{
    DEFAULT_THEME_COLOR_PRESETS, color_picker_control, normalize_hex_color,
};
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{ThemeColors, parse_hex_color_to_hsla};
use crate::ui::views::settings::state::SettingsPageState;
use gpui::*;

use crate::ui::views::settings::common::{
    settings_card, settings_card_header, snapshot_from_state, spawn_persist_settings,
};

const THEME_COLOR_PERSIST_DEBOUNCE_MS: u64 = 260;

pub(super) fn render_theme_color_card(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
) -> impl IntoElement {
    settings_card(colors, "settings-theme-color").child(
        settings_card_header(
            colors,
            i18n.t("CustomizationSettings.theme_color"),
            i18n.t("CustomizationSettings.theme_color_desc"),
        )
        .child(color_picker_control(
            "settings-theme-color-picker",
            colors,
            state.theme_color.as_ref(),
            state.theme_color_input.as_ref(),
            DEFAULT_THEME_COLOR_PRESETS,
            state.theme_color_picker_popup_open,
            state.theme_color_picker_drag_target.as_ref(),
            state.theme_color_picker_drag_origin_x,
            state.theme_color_picker_drag_origin_y,
            state.theme_color_picker_drag_origin_hue,
            state.theme_color_picker_drag_origin_saturation,
            state.theme_color_picker_drag_origin_value,
            state.theme_color_picker_drag_origin_alpha,
            state.theme_color_picker_popup_anchor_x,
            state.theme_color_picker_popup_anchor_y,
            move |picked, _window, cx| {
                let normalized =
                    normalize_hex_color(picked).unwrap_or_else(|| "#a0d9b6".to_string());
                cx.update_global(|state: &mut SettingsPageState, _cx| {
                    state.theme_color = SharedString::from(normalized.clone());
                });

                cx.update_global(|theme: &mut ThemeState, cx| {
                    theme.accent_hex = SharedString::from(normalized.clone());
                    theme.accent = parse_hex_color_to_hsla(&normalized);
                });

                schedule_theme_color_persist_debounced(cx);
            },
        )),
    )
}

fn schedule_theme_color_persist_debounced(cx: &mut App) {
    let should_spawn = cx.update_global(|state: &mut SettingsPageState, _cx| {
        state.theme_color_persist_revision = state.theme_color_persist_revision.saturating_add(1);
        if state.theme_color_persist_task_running {
            false
        } else {
            state.theme_color_persist_task_running = true;
            true
        }
    });

    if !should_spawn {
        return;
    }

    cx.spawn(async move |cx| {
        let mut observed_revision = 0u64;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(
                THEME_COLOR_PERSIST_DEBOUNCE_MS,
            ))
            .await;

            let (latest_revision, snapshot) =
                match cx.read_global(|state: &SettingsPageState, _cx| {
                    (
                        state.theme_color_persist_revision,
                        snapshot_from_state(state),
                    )
                }) {
                    Ok(value) => value,
                    Err(_) => return,
                };

            if latest_revision != observed_revision {
                observed_revision = latest_revision;
                continue;
            }

            if cx
                .update({
                    let snapshot = snapshot.clone();
                    move |cx| {
                        spawn_persist_settings(snapshot.clone(), cx);
                    }
                })
                .is_err()
            {
                return;
            }

            let _ = cx.update_global(|state: &mut SettingsPageState, _cx| {
                state.theme_color_persist_task_running = false;
            });
            return;
        }
    })
    .detach();
}

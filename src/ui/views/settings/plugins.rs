use crate::plugins::runtime::{PluginLogEntry, PluginStatus};
use crate::ui::components::dropdown::{Dropdown, DropdownOption};
use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::{Input, InputSize, InputState};
use crate::ui::components::markdown_renderer::{parse_markdown_document, render_markdown_document};
use crate::ui::components::toast;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::settings::common::{
    settings_action_button, settings_badge, settings_card, settings_control_box, settings_value_box,
};
use crate::ui::views::settings::state::{
    PluginReadmeCacheKey, PluginResourceCacheKey, PluginSettingsSubTab, SettingsPageState,
    SettingsTab,
};
use crate::ui::views::settings::{SettingsPageView, rows::tab_title};
use bmcbl_plugin_api::LogLevel;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::warn;

#[derive(Clone)]
pub(super) struct PluginSettingsModel {
    statuses: Vec<PluginStatus>,
    selected_id: Option<String>,
    readme: Option<String>,
    config_text: Option<String>,
    config_schema: Option<String>,
    logs: Vec<PluginLogEntry>,
    locale: String,
    translations: BTreeMap<String, String>,
    is_dark: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct PluginConfigSchema {
    #[serde(default)]
    fields: Vec<PluginConfigField>,
}

#[derive(Clone, Debug, Deserialize)]
struct PluginConfigField {
    key: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    label_key: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    description_key: Option<String>,
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    default: Option<toml::Value>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    options: Vec<PluginConfigOption>,
    #[serde(default)]
    restart_required: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct PluginConfigOption {
    value: toml::Value,
    #[serde(default)]
    label: String,
    #[serde(default)]
    label_key: Option<String>,
}

impl PluginSettingsModel {
    pub(super) fn snapshot(cx: &App, state: &SettingsPageState) -> Self {
        if state.tab != SettingsTab::Plugins {
            return Self::empty(cx, state);
        }

        let statuses = crate::plugins::runtime::statuses(cx);
        let selected_id = selected_plugin_id(state, &statuses);
        let selected_status = selected_id
            .as_deref()
            .and_then(|plugin_id| statuses.iter().find(|status| status.id == plugin_id));
        let readme = selected_status.and_then(|status| {
            let key = PluginReadmeCacheKey {
                plugin_id: status.id.clone(),
                generation: status.generation,
                locale: state.plugin_cached_locale.to_string(),
            };
            state.plugin_readme_cache.get(&key).cloned().flatten()
        });
        let config_text = selected_status.and_then(|status| {
            let key = PluginResourceCacheKey {
                plugin_id: status.id.clone(),
                generation: status.generation,
            };
            state.plugin_config_cache.get(&key).cloned().flatten()
        });
        let config_schema = selected_status.and_then(|status| {
            let key = PluginResourceCacheKey {
                plugin_id: status.id.clone(),
                generation: status.generation,
            };
            state
                .plugin_config_schema_cache
                .get(&key)
                .cloned()
                .flatten()
        });
        let translations = selected_status
            .and_then(|status| {
                config_schema
                    .as_deref()
                    .and_then(|schema| localized_schema_keys(schema).ok())
                    .map(|keys| {
                        keys.into_iter()
                            .filter_map(|key| {
                                crate::plugins::runtime::translate_plugin_resource_for_locale(
                                    cx,
                                    &status.id,
                                    state.plugin_cached_locale.as_ref(),
                                    &key,
                                )
                                .map(|value| (key, value))
                            })
                            .collect()
                    })
            })
            .unwrap_or_default();
        let logs = selected_id
            .as_deref()
            .map(|plugin_id| crate::plugins::runtime::plugin_logs(cx, plugin_id))
            .unwrap_or_default();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(std::time::Instant::now()),
            theme.accent,
        );

        Self {
            statuses,
            selected_id,
            readme,
            config_text,
            config_schema,
            logs,
            locale: state.plugin_cached_locale.to_string(),
            translations,
            is_dark: colors.bg.l < 0.5,
        }
    }

    fn empty(cx: &App, state: &SettingsPageState) -> Self {
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(std::time::Instant::now()),
            theme.accent,
        );

        Self {
            statuses: Vec::new(),
            selected_id: None,
            readme: None,
            config_text: None,
            config_schema: None,
            logs: Vec::new(),
            locale: state.plugin_cached_locale.to_string(),
            translations: BTreeMap::new(),
            is_dark: colors.bg.l < 0.5,
        }
    }
}

pub(super) fn ensure_plugin_resources(window: &mut Window, cx: &mut Context<SettingsPageView>) {
    if cx.global::<SettingsPageState>().tab != SettingsTab::Plugins {
        return;
    }

    crate::plugins::runtime::ensure_manifest_index(cx);

    let snapshot = cx.global::<SettingsPageState>();
    let statuses = crate::plugins::runtime::statuses(cx);
    let locale = cx.global::<I18n>().locale().code().to_string();
    let selected_id = selected_plugin_id(snapshot, &statuses);
    let selected_status = selected_id
        .as_deref()
        .and_then(|plugin_id| statuses.iter().find(|status| status.id == plugin_id))
        .cloned();
    let registry_generation = statuses
        .iter()
        .map(|status| status.generation)
        .max()
        .unwrap_or(0);

    let mut readme_load: Option<(PluginReadmeCacheKey, Option<String>)> = None;
    let mut config_load: Option<(PluginResourceCacheKey, Option<String>)> = None;
    let mut schema_load: Option<(PluginResourceCacheKey, Option<String>)> = None;
    let mut reset_selected = false;

    if let Some(status) = selected_status.as_ref() {
        match snapshot.plugin_sub_tab {
            PluginSettingsSubTab::Readme => {
                let key = PluginReadmeCacheKey {
                    plugin_id: status.id.clone(),
                    generation: status.generation,
                    locale: locale.clone(),
                };
                if !snapshot.plugin_readme_cache.contains_key(&key) {
                    readme_load = Some((
                        key,
                        crate::plugins::runtime::plugin_readme_for_locale(cx, &status.id, &locale),
                    ));
                }
            }
            PluginSettingsSubTab::Config => {
                let key = PluginResourceCacheKey {
                    plugin_id: status.id.clone(),
                    generation: status.generation,
                };
                if !snapshot.plugin_config_cache.contains_key(&key) {
                    config_load = Some((
                        key.clone(),
                        crate::plugins::runtime::plugin_config_text(cx, &status.id),
                    ));
                }
                if !snapshot.plugin_config_schema_cache.contains_key(&key) {
                    schema_load = Some((
                        key,
                        crate::plugins::runtime::plugin_config_schema(cx, &status.id),
                    ));
                }
            }
            PluginSettingsSubTab::Permissions | PluginSettingsSubTab::Logs => {}
        }
        reset_selected = snapshot
            .selected_plugin_id
            .as_ref()
            .is_none_or(|plugin_id| plugin_id.as_ref() != status.id);
    }

    let stale_generation = snapshot.plugin_cached_generation != registry_generation;
    let stale_locale = snapshot.plugin_cached_locale.as_ref() != locale;
    let needs_update = reset_selected
        || stale_generation
        || stale_locale
        || readme_load.is_some()
        || config_load.is_some()
        || schema_load.is_some();

    if needs_update {
        cx.update_global(|state: &mut SettingsPageState, _cx| {
            if stale_generation {
                state.plugin_readme_cache.clear();
                state.plugin_config_cache.clear();
                state.plugin_config_schema_cache.clear();
                state.plugin_config_loaded_for = None;
                state.plugin_config_draft = SharedString::from("");
                state.plugin_config_inputs.clear();
                state.plugin_config_inputs_for = None;
            } else if stale_locale {
                state.plugin_readme_cache.clear();
            }

            state.plugin_cached_generation = registry_generation;
            state.plugin_cached_locale = SharedString::from(locale.clone());

            if reset_selected {
                state.selected_plugin_id = selected_status
                    .as_ref()
                    .map(|status| SharedString::from(status.id.clone()));
                state.plugin_config_loaded_for = None;
                state.plugin_config_draft = SharedString::from("");
                state.plugin_config_inputs.clear();
                state.plugin_config_inputs_for = None;
            }
            if let Some((key, value)) = readme_load.take() {
                state.plugin_readme_cache.insert(key, value);
            }
            if let Some((key, value)) = config_load.take() {
                state.plugin_config_cache.insert(key, value);
            }
            if let Some((key, value)) = schema_load.take() {
                state.plugin_config_schema_cache.insert(key, value);
            }
        });
    }

    ensure_config_inputs_from_cache(window, cx);
}

fn ensure_config_inputs_from_cache(window: &mut Window, cx: &mut Context<SettingsPageView>) {
    let state = cx.global::<SettingsPageState>();
    if state.tab != SettingsTab::Plugins || state.plugin_sub_tab != PluginSettingsSubTab::Config {
        return;
    }

    let statuses = crate::plugins::runtime::statuses(cx);
    let Some(plugin_id) = selected_plugin_id(state, &statuses) else {
        return;
    };
    let Some(status) = statuses.iter().find(|status| status.id == plugin_id) else {
        return;
    };
    let cache_key = PluginResourceCacheKey {
        plugin_id: plugin_id.clone(),
        generation: status.generation,
    };
    let Some(schema_text) = state
        .plugin_config_schema_cache
        .get(&cache_key)
        .cloned()
        .flatten()
    else {
        return;
    };
    let Some(schema) = toml::from_str::<PluginConfigSchema>(&schema_text).ok() else {
        return;
    };
    let fields = schema
        .fields
        .into_iter()
        .filter(is_text_config_field)
        .collect::<Vec<_>>();
    let expected_keys = fields
        .iter()
        .map(|field| field.key.clone())
        .collect::<Vec<_>>();
    if plugin_config_inputs_match(state, &plugin_id, &expected_keys) {
        return;
    }

    let config_text = state
        .plugin_config_cache
        .get(&cache_key)
        .cloned()
        .flatten()
        .unwrap_or_default();
    let draft = current_config_draft(state, &plugin_id, &config_text);
    let values = draft
        .parse::<toml::Value>()
        .unwrap_or_else(|_| toml::Value::Table(Default::default()));

    cx.update_global(|state: &mut SettingsPageState, cx| {
        let stale_plugin = !state
            .plugin_config_inputs_for
            .as_ref()
            .is_some_and(|id| id.as_ref() == plugin_id.as_str());
        let stale_keys = state.plugin_config_inputs.len() != expected_keys.len()
            || expected_keys
                .iter()
                .any(|key| !state.plugin_config_inputs.contains_key(key));

        if stale_plugin || stale_keys {
            state.plugin_config_inputs.clear();
            state.plugin_config_inputs_for = Some(SharedString::from(plugin_id.clone()));
        }

        for field in &fields {
            if state.plugin_config_inputs.contains_key(&field.key) {
                continue;
            }

            let initial = config_value(&values, &field.key)
                .cloned()
                .or_else(|| field.default.clone())
                .unwrap_or_else(|| toml::Value::String(String::new()));
            let placeholder = SharedString::from(value_to_string(
                field
                    .default
                    .as_ref()
                    .unwrap_or(&toml::Value::String(String::new())),
            ));
            let initial = SharedString::from(value_to_string(&initial));
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                if !placeholder.is_empty() {
                    input.set_placeholder(placeholder, window, cx);
                }
                if !initial.is_empty() {
                    input.set_value(initial, window, cx);
                }
                input
            });
            state.plugin_config_inputs.insert(field.key.clone(), input);
        }
    });
}

pub(super) fn render_plugins_tab(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    model: &PluginSettingsModel,
) -> Div {
    let selected = model
        .selected_id
        .as_deref()
        .and_then(|id| model.statuses.iter().find(|status| status.id == id));

    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(tab_title(colors, i18n.t("Settings.tabs.plugins")))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(plugin_list(
                    colors,
                    i18n,
                    &model.statuses,
                    model.selected_id.as_deref(),
                ))
                .child(plugin_detail(colors, i18n, state, model, selected)),
        )
}

fn plugin_list(
    colors: &ThemeColors,
    i18n: &I18n,
    statuses: &[PluginStatus],
    selected_id: Option<&str>,
) -> Stateful<Div> {
    let mut list = settings_card(colors, "settings-plugins-list")
        .w(px(292.))
        .flex_shrink_0()
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(i18n.t("PluginSettings.installed")),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(
                            icon_action_button(
                                colors,
                                i18n.t("PluginSettings.import"),
                                lucide_icons::icon_file_up(),
                                true,
                            )
                            .w(px(78.))
                            .on_mouse_down(
                                MouseButton::Left,
                                move |_event, _window, cx| {
                                    import_plugin_package_from_picker(cx);
                                },
                            ),
                        )
                        .child(
                            icon_action_button(
                                colors,
                                i18n.t("PluginSettings.reload"),
                                lucide_icons::icon_refresh_cw(),
                                true,
                            )
                            .w(px(78.))
                            .on_mouse_down(
                                MouseButton::Left,
                                |_event, _window, cx| {
                                    crate::plugins::runtime::reload_plugins(cx);
                                },
                            ),
                        ),
                ),
        );

    if statuses.is_empty() {
        return list.child(
            div()
                .py(px(16.))
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(i18n.t("PluginSettings.empty")),
        );
    }

    for status in statuses {
        let is_selected = selected_id == Some(status.id.as_str());
        let plugin_id = status.id.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!(
                    "settings-plugin-item-{}",
                    status.id
                )))
                .rounded(px(10.))
                .px(px(10.))
                .py(px(9.))
                .flex()
                .flex_col()
                .gap(px(5.))
                .cursor_pointer()
                .bg(if is_selected {
                    Hsla {
                        a: 0.14,
                        ..colors.accent
                    }
                } else {
                    Hsla {
                        a: 0.0,
                        ..colors.surface
                    }
                })
                .hover(|this| {
                    this.bg(Hsla {
                        a: 0.10,
                        ..colors.accent
                    })
                })
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, _cx| {
                        state.selected_plugin_id = Some(SharedString::from(plugin_id.clone()));
                        state.plugin_config_loaded_for = None;
                        state.plugin_config_draft = SharedString::from("");
                        state.plugin_config_inputs.clear();
                        state.plugin_config_inputs_for = None;
                        state.plugin_sub_tab = PluginSettingsSubTab::Readme;
                    });
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(plugin_icon(status, colors, px(24.)))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(status.name.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(format!("{} · {}", status.id, status.version)),
                )
                .child(settings_badge(
                    colors,
                    if !status.enabled {
                        SharedString::from("Disabled")
                    } else if status.loaded {
                        i18n.t("PluginSettings.status.loaded")
                    } else if status.healthy {
                        SharedString::from("Enabled")
                    } else {
                        i18n.t("PluginSettings.status.failed")
                    },
                )),
        );
    }

    list
}

fn plugin_detail(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    model: &PluginSettingsModel,
    selected: Option<&PluginStatus>,
) -> Stateful<Div> {
    let Some(status) = selected else {
        return settings_card(colors, "settings-plugin-detail")
            .flex_1()
            .p(px(16.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_secondary)
                    .child(i18n.t("PluginSettings.empty")),
            );
    };

    settings_card(colors, "settings-plugin-detail")
        .flex_1()
        .min_w(px(0.))
        .p(px(16.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(plugin_header(colors, i18n, status))
        .child(plugin_actions_row(colors, i18n, status))
        .child(plugin_sub_tabs(
            colors,
            i18n,
            &status.id,
            state.plugin_sub_tab,
        ))
        .child(match state.plugin_sub_tab {
            PluginSettingsSubTab::Readme => plugin_readme_panel(colors, i18n, model),
            PluginSettingsSubTab::Permissions => plugin_permissions_panel(colors, status),
            PluginSettingsSubTab::Config => plugin_config_panel(colors, i18n, state, model, status),
            PluginSettingsSubTab::Logs => plugin_logs_panel(colors, i18n, model),
        })
}

fn plugin_header(colors: &ThemeColors, i18n: &I18n, status: &PluginStatus) -> Div {
    let authors = if status.authors.is_empty() {
        i18n.t("PluginSettings.unknown").to_string()
    } else {
        status.authors.join(", ")
    };
    let capabilities = if status.capabilities.is_empty() {
        i18n.t("PluginSettings.none").to_string()
    } else {
        status.capabilities.join(", ")
    };

    div()
        .w_full()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(plugin_icon(status, colors, px(42.)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(status.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.text_secondary)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(format!(
                                    "{} · v{} · {}",
                                    status.id, status.version, authors
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .line_height(px(18.))
                                .text_color(colors.text_muted)
                                .child(capabilities),
                        )
                        .when_some(status.error.clone(), |this, error| {
                            this.child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(colors.danger)
                                    .child(error),
                            )
                        }),
                ),
        )
        .child(settings_badge(
            colors,
            if !status.enabled {
                SharedString::from("Disabled")
            } else if status.loaded {
                i18n.t("PluginSettings.status.loaded")
            } else if status.healthy {
                SharedString::from("Enabled")
            } else {
                i18n.t("PluginSettings.status.failed")
            },
        ))
}

fn plugin_actions_row(colors: &ThemeColors, i18n: &I18n, status: &PluginStatus) -> Div {
    let plugin_id = status.id.clone();
    let enabled = status.enabled;
    let toggle_label = if enabled {
        SharedString::from("Disable")
    } else {
        SharedString::from("Enable")
    };
    let reload_id = status.id.clone();
    let uninstall_id = status.id.clone();
    let diagnostics_id = status.id.clone();
    let toggle_success = if enabled {
        SharedString::from("Plugin disabled")
    } else {
        SharedString::from("Plugin enabled")
    };
    div()
        .flex()
        .flex_wrap()
        .gap(px(8.))
        .child(
            settings_action_button(colors, toggle_label, true).on_mouse_down(
                MouseButton::Left,
                move |_event, _window, cx| match crate::plugins::runtime::set_plugin_enabled(
                    cx,
                    plugin_id.clone(),
                    !enabled,
                ) {
                    Ok(()) => {
                        toast::success(cx, toggle_success.clone());
                    }
                    Err(error) => {
                        toast::error(
                            cx,
                            SharedString::from(format!("Plugin state update failed: {error}")),
                        );
                    }
                },
            ),
        )
        .child(
            settings_action_button(colors, SharedString::from("Reload"), true).on_mouse_down(
                MouseButton::Left,
                move |_event, _window, cx| match crate::plugins::runtime::reload_plugin(
                    cx,
                    reload_id.clone(),
                ) {
                    Ok(()) => {
                        toast::success(cx, SharedString::from("Plugin reloaded"));
                    }
                    Err(error) => {
                        toast::error(
                            cx,
                            SharedString::from(format!("Plugin reload failed: {error}")),
                        );
                    }
                },
            ),
        )
        .child(
            settings_action_button(colors, SharedString::from("Diagnostics"), true).on_mouse_down(
                MouseButton::Left,
                move |_event, _window, cx| match crate::plugins::runtime::export_plugin_diagnostics(
                    cx,
                    &diagnostics_id,
                ) {
                    Ok(report) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(report));
                        toast::success(cx, SharedString::from("Plugin diagnostics copied"));
                    }
                    Err(error) => {
                        toast::error(
                            cx,
                            SharedString::from(format!("Diagnostics export failed: {error}")),
                        );
                    }
                },
            ),
        )
        .child(
            settings_action_button(colors, SharedString::from("Uninstall"), true).on_mouse_down(
                MouseButton::Left,
                move |_event, _window, cx| match crate::plugins::runtime::uninstall_plugin(
                    cx,
                    uninstall_id.clone(),
                ) {
                    Ok(()) => {
                        cx.update_global(|state: &mut SettingsPageState, _cx| {
                            state.selected_plugin_id = None;
                        });
                        toast::success(cx, SharedString::from("Plugin uninstalled"));
                    }
                    Err(error) => {
                        toast::error(
                            cx,
                            SharedString::from(format!("Plugin uninstall failed: {error}")),
                        );
                    }
                },
            ),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(colors.text_muted)
                .flex()
                .items_center()
                .child(i18n.t("PluginSettings.installed")),
        )
}

fn plugin_sub_tabs(
    colors: &ThemeColors,
    i18n: &I18n,
    plugin_id: &str,
    active: PluginSettingsSubTab,
) -> Div {
    let button = |label: SharedString, tab: PluginSettingsSubTab| {
        let is_active = active == tab;
        let plugin_id = plugin_id.to_string();
        div()
            .h(px(30.))
            .px(px(12.))
            .rounded(px(9.))
            .cursor_pointer()
            .bg(if is_active {
                Hsla {
                    a: 0.16,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.58,
                    ..colors.surface
                }
            })
            .text_size(px(12.))
            .font_weight(if is_active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .text_color(colors.text_primary)
            .flex()
            .items_center()
            .child(label)
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.update_global(|state: &mut SettingsPageState, _cx| {
                    state.selected_plugin_id = Some(SharedString::from(plugin_id.clone()));
                    state.plugin_sub_tab = tab;
                });
            })
    };

    div()
        .flex()
        .gap(px(8.))
        .child(button(
            i18n.t("PluginSettings.readme"),
            PluginSettingsSubTab::Readme,
        ))
        .child(button(
            SharedString::from("Permissions"),
            PluginSettingsSubTab::Permissions,
        ))
        .child(button(
            i18n.t("PluginSettings.config"),
            PluginSettingsSubTab::Config,
        ))
        .child(button(
            i18n.t("PluginSettings.logs"),
            PluginSettingsSubTab::Logs,
        ))
}

fn plugin_readme_panel(
    colors: &ThemeColors,
    i18n: &I18n,
    model: &PluginSettingsModel,
) -> AnyElement {
    let Some(markdown) = model.readme.as_ref() else {
        return empty_panel(colors, i18n.t("PluginSettings.readme_empty"));
    };
    let document = parse_markdown_document(markdown);
    div()
        .w_full()
        .p(px(2.))
        .child(render_markdown_document(&document, colors, model.is_dark))
        .into_any_element()
}

fn plugin_permissions_panel(colors: &ThemeColors, status: &PluginStatus) -> AnyElement {
    let permission_rows = [
        (
            "Network",
            if status.permissions.network_allow.is_empty() {
                "none".to_string()
            } else {
                status.permissions.network_allow.join("\n")
            },
        ),
        (
            "Resources",
            if status.permissions.resource_allow.is_empty() {
                "none".to_string()
            } else {
                status.permissions.resource_allow.join("\n")
            },
        ),
        (
            "External URLs",
            if status.permissions.external_allow.is_empty() {
                "none".to_string()
            } else {
                status.permissions.external_allow.join("\n")
            },
        ),
    ];
    let limit_rows = [
        ("Memory", format!("{} MB", status.limits.memory_mb)),
        (
            "HTTP body",
            format!("{} bytes", status.limits.max_http_bytes),
        ),
        (
            "Resource read",
            format!("{} bytes", status.limits.max_resource_bytes),
        ),
        (
            "KV storage",
            format!("{} bytes", status.limits.max_storage_bytes),
        ),
    ];

    let mut panel = div().w_full().flex().flex_col().gap(px(10.));
    for (label, value) in permission_rows {
        panel = panel.child(plugin_info_row(colors, label, value));
    }
    panel = panel.child(div().h(px(1.)).w_full().bg(Hsla {
        a: 0.16,
        ..colors.border
    }));
    for (label, value) in limit_rows {
        panel = panel.child(plugin_info_row(colors, label, value));
    }
    panel.into_any_element()
}

fn plugin_info_row(colors: &ThemeColors, label: &str, value: String) -> Div {
    div()
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.50,
            ..colors.settings_field_bg
        })
        .px(px(12.))
        .py(px(10.))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(colors.text_secondary)
                .child(value),
        )
}

fn plugin_config_panel(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    model: &PluginSettingsModel,
    status: &PluginStatus,
) -> AnyElement {
    let config_text = model.config_text.clone().unwrap_or_default();
    let draft = current_config_draft(state, &status.id, &config_text);
    let schema = model
        .config_schema
        .as_deref()
        .and_then(|text| toml::from_str::<PluginConfigSchema>(text).ok());

    let save_draft = draft.clone();
    let editable_fields = schema
        .as_ref()
        .map(|schema| {
            schema
                .fields
                .iter()
                .filter(|field| is_text_config_field(field))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let plugin_id = status.id.clone();
    let generation = status.generation;
    let save_success_message = i18n.t("PluginSettings.config_saved");
    let save_failed_message = i18n.t("PluginSettings.config_save_failed");
    let mut panel =
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(div().flex().justify_end().child(
                settings_action_button(colors, i18n.t("PluginSettings.save"), true).on_mouse_down(
                    MouseButton::Left,
                    move |_event, _window, cx| {
                        let content =
                            config_draft_with_inputs(cx, &plugin_id, &save_draft, &editable_fields);
                        match crate::plugins::runtime::save_plugin_config(
                            cx,
                            plugin_id.clone(),
                            content.clone(),
                        ) {
                            Ok(()) => {
                                cx.update_global(|state: &mut SettingsPageState, _cx| {
                                    let key = PluginResourceCacheKey {
                                        plugin_id: plugin_id.clone(),
                                        generation,
                                    };
                                    state.plugin_config_cache.insert(key, Some(content.clone()));
                                    state.plugin_config_loaded_for =
                                        Some(SharedString::from(plugin_id.clone()));
                                    state.plugin_config_draft = SharedString::from(content);
                                    state.plugin_config_inputs.clear();
                                    state.plugin_config_inputs_for = None;
                                });
                                toast::success(cx, save_success_message.clone());
                            }
                            Err(error) => {
                                toast::error(
                                    cx,
                                    SharedString::from(format!("{}: {error}", save_failed_message)),
                                );
                            }
                        };
                    },
                ),
            ));

    if let Some(schema) = schema {
        let values = draft
            .parse::<toml::Value>()
            .unwrap_or_else(|_| toml::Value::Table(Default::default()));
        for field in schema.fields {
            panel = panel.child(render_config_field(
                colors,
                i18n,
                state,
                &status.id,
                &model.locale,
                &model.translations,
                &field,
                &values,
                &draft,
            ));
        }
    } else if draft.is_empty() {
        panel = panel.child(empty_panel(colors, i18n.t("PluginSettings.config_empty")));
    } else {
        panel = panel.child(raw_config_panel(colors, i18n, &draft));
    }

    panel.into_any_element()
}

fn render_config_field(
    colors: &ThemeColors,
    i18n: &I18n,
    state: &SettingsPageState,
    plugin_id: &str,
    locale: &str,
    translations: &BTreeMap<String, String>,
    field: &PluginConfigField,
    values: &toml::Value,
    fallback_draft: &str,
) -> AnyElement {
    let current_value = config_value(values, &field.key)
        .cloned()
        .or_else(|| field.default.clone())
        .unwrap_or_else(|| toml::Value::String(String::new()));
    let label = localized_field_label(translations, field);
    let description = localized_field_description(translations, i18n, field);

    div()
        .w_full()
        .px(px(12.))
        .py(px(11.))
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.50,
            ..colors.settings_field_bg
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .min_w(px(0.))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(17.))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
        .child(match field.field_type.as_str() {
            "bool" => render_bool_control(colors, plugin_id, field, &current_value, fallback_draft),
            "select" => render_select_control(
                colors,
                plugin_id,
                locale,
                translations,
                field,
                &current_value,
                fallback_draft,
            ),
            "string" | "integer" | "float" => {
                render_input_control(colors, field, state.plugin_config_inputs.get(&field.key))
            }
            _ => render_value_control(colors, field, &current_value),
        })
        .into_any_element()
}

fn render_bool_control(
    colors: &ThemeColors,
    plugin_id: &str,
    field: &PluginConfigField,
    current_value: &toml::Value,
    fallback_draft: &str,
) -> AnyElement {
    let enabled = current_value.as_bool().unwrap_or(false);
    let key = field.key.clone();
    let plugin_id = plugin_id.to_string();
    let fallback_draft = fallback_draft.to_string();
    div()
        .w(px(260.))
        .flex()
        .justify_end()
        .child(ToggleSwitch::new(
            SharedString::from(format!("plugin-config-bool-{}-{key}", plugin_id)),
            colors,
            enabled,
            move |cx| {
                update_config_draft(
                    cx,
                    &plugin_id,
                    &key,
                    toml::Value::Boolean(!enabled),
                    &fallback_draft,
                );
            },
        ))
        .into_any_element()
}

fn render_select_control(
    colors: &ThemeColors,
    plugin_id: &str,
    _locale: &str,
    translations: &BTreeMap<String, String>,
    field: &PluginConfigField,
    value: &toml::Value,
    fallback_draft: &str,
) -> AnyElement {
    let options = field
        .options
        .iter()
        .map(|option| {
            DropdownOption::from(SharedString::from(localized_option_label(
                translations,
                option,
            )))
        })
        .collect::<Vec<_>>();
    let selected_index = field
        .options
        .iter()
        .position(|option| option.value == *value)
        .unwrap_or(0);
    let display = field
        .options
        .get(selected_index)
        .map(|option| SharedString::from(localized_option_label(translations, option)))
        .unwrap_or_else(|| SharedString::from(""));
    let key = field.key.clone();
    let plugin_id = plugin_id.to_string();
    let fallback_draft = fallback_draft.to_string();
    let values = field
        .options
        .iter()
        .map(|option| option.value.clone())
        .collect::<Vec<_>>();
    Dropdown::new(
        SharedString::from(format!("plugin-config-select-{}-{key}", plugin_id)),
        colors,
        px(260.),
        display,
        options,
        selected_index,
        !values.is_empty(),
        move |index, _window, cx| {
            if let Some(value) = values.get(index).cloned() {
                update_config_draft(cx, &plugin_id, &key, value, &fallback_draft);
            }
        },
    )
    .into_any_element()
}

fn render_value_control(
    colors: &ThemeColors,
    field: &PluginConfigField,
    value: &toml::Value,
) -> AnyElement {
    let mut display = value_to_string(value);
    if let Some(min) = field.min {
        display.push_str(&format!("  min {min}"));
    }
    if let Some(max) = field.max {
        display.push_str(&format!("  max {max}"));
    }
    settings_value_box(colors, SharedString::from(display), false, px(260.)).into_any_element()
}

fn render_input_control(
    colors: &ThemeColors,
    field: &PluginConfigField,
    input: Option<&Entity<InputState>>,
) -> AnyElement {
    let control: AnyElement = if let Some(input) = input {
        Input::new(input)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(true)
            .with_size(InputSize::Small)
            .w_full()
            .h(px(30.))
            .px(px(4.))
            .text_size(px(12.))
            .into_any_element()
    } else {
        let display = field
            .default
            .as_ref()
            .map(value_to_string)
            .unwrap_or_default();
        div()
            .w_full()
            .h(px(30.))
            .px(px(4.))
            .flex()
            .items_center()
            .text_size(px(12.))
            .text_color(colors.text_muted)
            .child(display)
            .into_any_element()
    };

    settings_control_box(colors, true, px(260.), control).into_any_element()
}

fn raw_config_panel(colors: &ThemeColors, i18n: &I18n, content: &str) -> AnyElement {
    div()
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(i18n.t("PluginSettings.raw_config")),
        )
        .child(
            div()
                .text_size(px(11.))
                .line_height(px(18.))
                .text_color(colors.text_secondary)
                .child(content.to_string()),
        )
        .into_any_element()
}

fn plugin_logs_panel(colors: &ThemeColors, i18n: &I18n, model: &PluginSettingsModel) -> AnyElement {
    if model.logs.is_empty() {
        return empty_panel(colors, i18n.t("PluginSettings.logs_empty"));
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(model.logs.iter().cloned().map(|entry| {
            div()
                .w_full()
                .rounded(px(8.))
                .px(px(10.))
                .py(px(7.))
                .bg(Hsla {
                    a: 0.52,
                    ..colors.settings_field_bg
                })
                .text_size(px(11.))
                .text_color(log_level_color(colors, entry.level))
                .child(entry.message)
                .into_any_element()
        }))
        .into_any_element()
}

fn empty_panel(colors: &ThemeColors, message: SharedString) -> AnyElement {
    div()
        .w_full()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .p(px(16.))
        .text_size(px(13.))
        .text_color(colors.text_secondary)
        .child(message)
        .into_any_element()
}

fn plugin_icon(status: &PluginStatus, colors: &ThemeColors, size: Pixels) -> AnyElement {
    let fallback = || {
        div()
            .w(size)
            .h(size)
            .rounded(px(8.))
            .bg(Hsla {
                a: 0.12,
                ..colors.accent
            })
            .flex()
            .items_center()
            .justify_center()
            .child(themed_icon(
                lucide_icons::icon_plug(),
                (size / px(1.) * 0.58).clamp(12.0, 24.0),
                colors.accent,
            ))
            .into_any_element()
    };
    status.icon_path.as_ref().map_or_else(fallback, |path| {
        div()
            .w(size)
            .h(size)
            .rounded(px(8.))
            .overflow_hidden()
            .bg(colors.surface)
            .child(img(path.clone()).size_full().object_fit(ObjectFit::Contain))
            .into_any_element()
    })
}

fn icon_action_button(
    colors: &ThemeColors,
    label: SharedString,
    icon_path: &'static str,
    enabled: bool,
) -> Div {
    settings_action_button(colors, SharedString::from(""), enabled)
        .gap(px(6.))
        .children([themed_icon(icon_path, 14.0, colors.text_primary).into_any_element()])
        .child(label)
}

fn import_plugin_package_from_picker(cx: &mut App) {
    let Some(path) =
        crate::utils::file_picker::pick_file_path_with_filter("BMCBL Plugin", &["bmcblx"])
    else {
        return;
    };
    let (success_message, failed_message) = cx.read_global(|i18n: &I18n, _cx| {
        (
            i18n.t("PluginSettings.import_success"),
            i18n.t("PluginSettings.import_failed"),
        )
    });
    let source = PathBuf::from(path);
    match crate::plugins::runtime::import_plugin_package(cx, &source) {
        Ok(()) => {
            toast::success(cx, success_message);
        }
        Err(error) => {
            warn!(error = ?error, path = %source.display(), "plugin import failed");
            toast::error(
                cx,
                SharedString::from(format!("{}: {error}", failed_message)),
            );
        }
    }
}

fn selected_plugin_id(state: &SettingsPageState, statuses: &[PluginStatus]) -> Option<String> {
    state
        .selected_plugin_id
        .as_ref()
        .map(ToString::to_string)
        .filter(|plugin_id| statuses.iter().any(|status| status.id == *plugin_id))
        .or_else(|| statuses.first().map(|status| status.id.clone()))
}

fn current_config_draft(state: &SettingsPageState, plugin_id: &str, config_text: &str) -> String {
    if state
        .plugin_config_loaded_for
        .as_ref()
        .is_some_and(|id| id.as_ref() == plugin_id)
    {
        state.plugin_config_draft.to_string()
    } else {
        config_text.to_string()
    }
}

fn update_config_draft(
    cx: &mut App,
    plugin_id: &str,
    key: &str,
    value: toml::Value,
    fallback_draft: &str,
) {
    let mut document = cx
        .read_global(|state: &SettingsPageState, _cx| {
            if state
                .plugin_config_loaded_for
                .as_ref()
                .is_some_and(|id| id.as_ref() == plugin_id)
            {
                state.plugin_config_draft.to_string()
            } else {
                fallback_draft.to_string()
            }
        })
        .parse::<toml::Table>()
        .unwrap_or_default();
    set_config_value(&mut document, key, value);
    cx.update_global(|state: &mut SettingsPageState, _cx| {
        state.selected_plugin_id = Some(SharedString::from(plugin_id.to_string()));
        state.plugin_config_draft = SharedString::from(toml::Value::Table(document).to_string());
        state.plugin_config_loaded_for = Some(SharedString::from(plugin_id.to_string()));
    });
}

fn config_draft_with_inputs(
    cx: &mut App,
    plugin_id: &str,
    fallback_draft: &str,
    fields: &[PluginConfigField],
) -> String {
    cx.read_global(|state: &SettingsPageState, cx| {
        let mut document = if state
            .plugin_config_loaded_for
            .as_ref()
            .is_some_and(|id| id.as_ref() == plugin_id)
        {
            state.plugin_config_draft.to_string()
        } else {
            fallback_draft.to_string()
        }
        .parse::<toml::Table>()
        .unwrap_or_default();

        if state
            .plugin_config_inputs_for
            .as_ref()
            .is_some_and(|id| id.as_ref() == plugin_id)
        {
            for field in fields {
                let Some(input) = state.plugin_config_inputs.get(&field.key) else {
                    continue;
                };
                let raw = input.read(cx).value().to_string();
                set_config_value(&mut document, &field.key, input_value(field, &raw));
            }
        }

        toml::Value::Table(document).to_string()
    })
}

fn is_text_config_field(field: &PluginConfigField) -> bool {
    matches!(field.field_type.as_str(), "string" | "integer" | "float")
}

fn input_value(field: &PluginConfigField, raw: &str) -> toml::Value {
    match field.field_type.as_str() {
        "integer" => toml::Value::Integer(
            clamp_number(raw.parse::<f64>().unwrap_or(0.0), field).round() as i64,
        ),
        "float" => toml::Value::Float(clamp_number(raw.parse::<f64>().unwrap_or(0.0), field)),
        _ => toml::Value::String(raw.to_string()),
    }
}

fn clamp_number(value: f64, field: &PluginConfigField) -> f64 {
    let min = field.min.unwrap_or(f64::NEG_INFINITY);
    let max = field.max.unwrap_or(f64::INFINITY);
    value.clamp(min, max)
}

fn localized_field_label(
    translations: &BTreeMap<String, String>,
    field: &PluginConfigField,
) -> SharedString {
    SharedString::from(localized_plugin_text(
        translations,
        field.label_key.as_deref(),
        &field.label,
    ))
}

fn localized_field_description(
    translations: &BTreeMap<String, String>,
    i18n: &I18n,
    field: &PluginConfigField,
) -> SharedString {
    let description = localized_plugin_text(
        translations,
        field.description_key.as_deref(),
        &field.description,
    );
    if field.restart_required {
        SharedString::from(format!(
            "{} {}",
            description,
            i18n.t("PluginSettings.restart_required")
        ))
    } else {
        SharedString::from(description)
    }
}

fn localized_option_label(
    translations: &BTreeMap<String, String>,
    option: &PluginConfigOption,
) -> String {
    localized_plugin_text(translations, option.label_key.as_deref(), &option.label)
}

fn localized_plugin_text(
    translations: &BTreeMap<String, String>,
    key: Option<&str>,
    fallback: &str,
) -> String {
    let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
        return fallback.to_string();
    };
    translations.get(key).cloned().unwrap_or_else(|| {
        if fallback.is_empty() {
            key.to_string()
        } else {
            fallback.to_string()
        }
    })
}

fn localized_schema_keys(schema_text: &str) -> Result<Vec<String>, toml::de::Error> {
    let schema = toml::from_str::<PluginConfigSchema>(schema_text)?;
    let mut keys = Vec::new();
    for field in schema.fields {
        push_translation_key(&mut keys, field.label_key);
        push_translation_key(&mut keys, field.description_key);
        for option in field.options {
            push_translation_key(&mut keys, option.label_key);
        }
    }
    Ok(keys)
}

fn plugin_config_inputs_match(
    state: &SettingsPageState,
    plugin_id: &str,
    expected_keys: &[String],
) -> bool {
    state
        .plugin_config_inputs_for
        .as_ref()
        .is_some_and(|id| id.as_ref() == plugin_id)
        && state.plugin_config_inputs.len() == expected_keys.len()
        && expected_keys
            .iter()
            .all(|key| state.plugin_config_inputs.contains_key(key))
}

fn push_translation_key(keys: &mut Vec<String>, key: Option<String>) {
    let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
        return;
    };
    if !keys.iter().any(|existing| existing == &key) {
        keys.push(key);
    }
}

fn config_value<'a>(values: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut current = values;
    for part in key.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn set_config_value(table: &mut toml::Table, key: &str, value: toml::Value) {
    let mut current = table;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_string(), value);
            return;
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()));
        if !entry.is_table() {
            *entry = toml::Value::Table(Default::default());
        }
        let Some(next) = entry.as_table_mut() else {
            return;
        };
        current = next;
    }
}

fn value_to_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        _ => value.to_string(),
    }
}

fn log_level_color(colors: &ThemeColors, level: LogLevel) -> Hsla {
    match level {
        LogLevel::Error => colors.danger,
        LogLevel::Warn => rgb(0xf59e0b).into(),
        LogLevel::Info => colors.text_secondary,
        LogLevel::Debug => colors.text_muted,
    }
}

#[cfg(test)]
mod tests {
    use super::PluginConfigSchema;

    #[::core::prelude::v1::test]
    fn schema_fields_allow_localization_keys() {
        let schema: PluginConfigSchema = toml::from_str(
            r#"
[[fields]]
key = "enabled"
label_key = "config.enabled.label"
description_key = "config.enabled.description"
type = "bool"
default = true
"#,
        )
        .expect("schema with localization keys should parse");

        let field = schema.fields.first().expect("field should exist");
        assert_eq!(field.label_key.as_deref(), Some("config.enabled.label"));
        assert_eq!(
            field.description_key.as_deref(),
            Some("config.enabled.description")
        );
    }
}

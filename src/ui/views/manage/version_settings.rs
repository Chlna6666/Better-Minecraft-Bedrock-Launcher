use super::ManagePageView;
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::components::toggle_switch::ToggleSwitch;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::manage::common::{
    card_title, ghost_button, panel_shell, primary_button, secondary_button,
};
use crate::ui::views::manage::state::{ManageVersionConfig, ManagedVersionEntry};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::rc::Rc;

mod icon;

const HOTKEY_OPTIONS: [&str; 5] = ["ALT", "CTRL", "SHIFT", "LWIN", "RWIN"];

#[derive(Clone)]
pub struct VersionSettingsModalState {
    pub version: ManagedVersionEntry,
    pub config: ManageVersionConfig,
    pub icon_source_path: Option<SharedString>,
    pub saving: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionSettingsToggle {
    DebugConsole,
    Redirection,
    EditorMode,
    DisableModLoading,
    LockMouseOnLaunch,
    ShortcutSilentLaunch,
}

pub fn render(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let can_use_editor = supports_editor_mode(&state.version);
    let is_gdk = state.version.is_gdk();
    let modal_dismiss_handle = modal::ModalDismissHandle::new();
    let dismiss_handle = view_handle.clone();
    let dismiss = Rc::new(move |cx: &mut App| {
        let _ = dismiss_handle.update(cx, |this, cx| {
            this.close_version_settings(cx);
        });
    });

    let cancel_dismiss = modal_dismiss_handle.clone();
    modal::modal_layer_dismissible_with_handle(
        modal_dismiss_handle,
        div()
            .w(px(640.))
            .max_w(relative(1.0))
            .min_w(px(0.))
            .max_h(px(540.))
            .rounded(px(22.))
            .border_1()
            .border_color(Hsla {
                a: 0.22,
                ..colors.border
            })
            .bg(colors.settings_panel_bg)
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.20,
                    ..gpui::black()
                },
                blur_radius: px(30.),
                spread_radius: px(-10.),
                offset: point(px(0.), px(16.)),
            }])
            .flex()
            .flex_col()
            .child(render_header(state, colors, i18n))
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .max_h(px(340.))
                    .overflow_y_scrollbar()
                    .p(px(16.))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .items_stretch()
                            .gap(px(10.))
                            .child(icon::render_icon_card(
                                state,
                                colors,
                                i18n,
                                view_handle.clone(),
                            ))
                            .child(render_toggle_card(
                                "settings-debug-console",
                                colors,
                                i18n.t("VersionSettingsModal.debug_console_label"),
                                i18n.t("VersionSettingsModal.debug_console_desc"),
                                state.config.enable_debug_console,
                                VersionSettingsToggle::DebugConsole,
                                view_handle.clone(),
                            ))
                            .child(render_toggle_card(
                                "settings-redirection",
                                colors,
                                i18n.t("VersionSettingsModal.redirection_label"),
                                i18n.t("VersionSettingsModal.redirection_desc"),
                                state.config.enable_redirection,
                                VersionSettingsToggle::Redirection,
                                view_handle.clone(),
                            ))
                            .when(!is_gdk, |this| {
                                this.child(render_mouse_lock_card(
                                    state,
                                    colors,
                                    i18n,
                                    view_handle.clone(),
                                ))
                            })
                            .child(render_toggle_card(
                                "settings-shortcut-silent-launch",
                                colors,
                                i18n.t("VersionSettingsModal.shortcut_silent_launch_label"),
                                i18n.t("VersionSettingsModal.shortcut_silent_launch_desc"),
                                state.config.shortcut_silent_launch,
                                VersionSettingsToggle::ShortcutSilentLaunch,
                                view_handle.clone(),
                            ))
                            .child(render_toggle_card(
                                "settings-disable-mod-loading",
                                colors,
                                i18n.t("VersionSettingsModal.disable_mod_loading_label"),
                                i18n.t("VersionSettingsModal.disable_mod_loading_desc"),
                                state.config.disable_mod_loading,
                                VersionSettingsToggle::DisableModLoading,
                                view_handle.clone(),
                            ))
                            .when(can_use_editor, |this| {
                                this.child(render_toggle_card(
                                    "settings-editor-mode",
                                    colors,
                                    i18n.t("VersionSettingsModal.editor_label"),
                                    i18n.t("VersionSettingsModal.editor_desc"),
                                    state.config.editor_mode,
                                    VersionSettingsToggle::EditorMode,
                                    view_handle.clone(),
                                ))
                            }),
                    ),
            )
            .child(
                div()
                    .px(px(18.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.16,
                        ..colors.border
                    })
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(10.))
                    .child({
                        ghost_button(colors, "manage-settings-cancel", i18n.t("common.cancel"))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cancel_dismiss.dismiss(cx);
                            })
                    })
                    .child({
                        let view_handle = view_handle.clone();
                        primary_button(
                            colors,
                            "manage-settings-save",
                            if state.saving {
                                i18n.t("common.saving")
                            } else {
                                i18n.t("VersionSettingsModal.save_changes")
                            },
                        )
                        .opacity(if state.saving { 0.75 } else { 1.0 })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.save_version_settings(cx);
                                });
                            },
                        )
                    }),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}

fn render_header(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    i18n: &I18n,
) -> impl IntoElement {
    div()
        .px(px(22.))
        .pt(px(22.))
        .pb(px(12.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(i18n.t("ManagePage.version_settings")),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(state.version.display_name()),
        )
}

fn render_toggle_card(
    id: &'static str,
    colors: &ThemeColors,
    title: SharedString,
    description: SharedString,
    enabled: bool,
    field: VersionSettingsToggle,
    view_handle: WeakEntity<ManagePageView>,
) -> Div {
    panel_shell(colors).w_full().p(px(14.)).child(
        div()
            .id(id)
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(card_title(colors, title))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.45))
                            .text_color(colors.text_secondary)
                            .child(description),
                    ),
            )
            .child(ToggleSwitch::new(
                SharedString::from(format!("toggle-{id}")),
                colors,
                enabled,
                move |cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.toggle_version_setting(field, cx);
                    });
                },
            )),
    )
}

fn render_mouse_lock_card(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> Div {
    let hotkey_group = div()
        .flex()
        .gap(px(8.))
        .flex_wrap()
        .children(HOTKEY_OPTIONS.iter().map(|hotkey| {
            let is_active = state.config.unlock_mouse_hotkey.as_ref() == *hotkey;
            {
                let hotkey_value = SharedString::from(*hotkey);
                let view_handle = view_handle.clone();
                div()
                    .id(SharedString::from(format!("mouse-hotkey-{hotkey}")))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(if is_active {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .bg(if is_active {
                        Hsla {
                            a: 0.14,
                            ..colors.accent
                        }
                    } else {
                        colors.surface
                    })
                    .cursor_pointer()
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if is_active {
                                colors.accent
                            } else {
                                colors.text_secondary
                            })
                            .child(*hotkey),
                    )
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        let hotkey = hotkey_value.clone();
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_version_hotkey(hotkey, cx);
                        });
                    })
            }
        }));

    panel_shell(colors)
        .w_full()
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child({
            let toggle_view_handle = view_handle.clone();
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(14.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(card_title(
                            colors,
                            i18n.t("VersionSettingsModal.mouse_lock_label"),
                        ))
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(relative(1.45))
                                .text_color(colors.text_secondary)
                                .child(i18n.t("VersionSettingsModal.mouse_lock_desc")),
                        ),
                )
                .child(ToggleSwitch::new(
                    SharedString::from("toggle-lock-mouse"),
                    colors,
                    state.config.lock_mouse_on_launch,
                    move |cx| {
                        let _ = toggle_view_handle.update(cx, |this, cx| {
                            this.toggle_version_setting(
                                VersionSettingsToggle::LockMouseOnLaunch,
                                cx,
                            );
                        });
                    },
                ))
        })
        .when(state.config.lock_mouse_on_launch, |this: Div| {
            this.child(
                panel_shell(colors).w_full().p(px(14.)).child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(14.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.))
                                        .child(card_title(
                                            colors,
                                            i18n.t("VersionSettingsModal.mouse_lock_reduce_label"),
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(colors.text_secondary)
                                                .child(i18n.t(
                                                    "VersionSettingsModal.mouse_lock_reduce_desc",
                                                )),
                                        ),
                                )
                                .child({
                                    let view_handle = view_handle.clone();
                                    secondary_button(
                                        colors,
                                        "manage-settings-reduce-pixels",
                                        SharedString::from(format!(
                                            "{} px",
                                            state.config.reduce_pixels
                                        )),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        move |_, window, cx| {
                                            let _ = view_handle.update(cx, |this, cx| {
                                                this.open_reduce_pixels_prompt(window, cx);
                                            });
                                        },
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(card_title(
                                    colors,
                                    i18n.t("VersionSettingsModal.mouse_lock_hotkey_label"),
                                ))
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(
                                            i18n.t("VersionSettingsModal.mouse_lock_hotkey_desc"),
                                        ),
                                )
                                .child(hotkey_group)
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.text_muted)
                                        .child(
                                            i18n.t("VersionSettingsModal.mouse_lock_hotkey_tip"),
                                        ),
                                ),
                        ),
                ),
            )
        })
}

pub fn supports_editor_mode(version: &ManagedVersionEntry) -> bool {
    is_version_at_least(version.version.as_ref(), "1.19.80.20")
}

fn is_version_at_least(current: &str, baseline: &str) -> bool {
    let mut current_parts = current
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    let mut baseline_parts = baseline
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));

    for _ in 0..5 {
        let left = current_parts.next().unwrap_or(0);
        let right = baseline_parts.next().unwrap_or(0);
        match left.cmp(&right) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }
    }

    true
}

use super::{VersionSettingsModalState, VersionSettingsToggle, levilamina, render_toggle_card};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::manage::ManagePageView;
use gpui::*;

pub(super) fn render_cards(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> Vec<AnyElement> {
    if !state.version.is_gdk() {
        return vec![render_uwp_notice(colors).into_any_element()];
    }

    vec![
        levilamina::render_card(state, colors, i18n, view_handle.clone()),
        render_toggle_card(
            "settings-debug-console",
            colors,
            t!("VersionSettingsModal.debug_console_label"),
            t!("VersionSettingsModal.debug_console_desc"),
            state.config.enable_debug_console,
            VersionSettingsToggle::DebugConsole,
            view_handle.clone(),
        )
        .into_any_element(),
        render_toggle_card(
            "settings-redirection",
            colors,
            t!("VersionSettingsModal.redirection_label"),
            t!("VersionSettingsModal.redirection_desc"),
            state.config.enable_redirection,
            VersionSettingsToggle::Redirection,
            view_handle,
        )
        .into_any_element(),
    ]
}

fn render_uwp_notice(colors: &ThemeColors) -> Div {
    div()
        .w_full()
        .p(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.28,
            ..colors.accent
        })
        .bg(Hsla {
            a: 0.08,
            ..colors.accent
        })
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(t!("VersionSettingsModal.uwp_compatibility_label")),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(relative(1.45))
                .text_color(colors.text_secondary)
                .child(t!("VersionSettingsModal.uwp_compatibility_desc")),
        )
}

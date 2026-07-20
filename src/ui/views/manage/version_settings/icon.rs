use super::VersionSettingsModalState;
use crate::ui::hooks::use_local_versions::{LaunchVersionIcon, launch_version_icon_path};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::manage::ManagePageView;
use crate::ui::views::manage::common::{card_title, panel_shell, secondary_button};
use gpui::*;
use std::path::Path;

pub(super) fn render_icon_card(
    state: &VersionSettingsModalState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> Div {
    let preview_icon = preview_icon_path(
        state
            .icon_source_path
            .as_ref()
            .map(|icon_source_path| icon_source_path.as_ref()),
        state
            .version
            .icon_path
            .as_ref()
            .map(|icon_path| icon_path.as_ref()),
        state.version.name.as_ref(),
    );
    let button_label = selected_icon_label(
        state
            .icon_source_path
            .as_ref()
            .map(|icon_source_path| icon_source_path.as_ref()),
    )
    .unwrap_or_else(|| i18n.t("VersionSettingsModal.icon_select"));

    panel_shell(colors).w_full().p(px(14.)).child(
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex_none()
                    .size(px(48.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(colors.border)
                    .child(
                        img(preview_icon)
                            .size_full()
                            .rounded(px(10.))
                            .object_fit(ObjectFit::Cover),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(card_title(
                        colors,
                        i18n.t("VersionSettingsModal.icon_label"),
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.45))
                            .text_color(colors.text_secondary)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(i18n.t("VersionSettingsModal.icon_desc")),
                    ),
            )
            .child(
                secondary_button(colors, "manage-settings-version-icon", button_label)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.select_version_icon(window, cx);
                        });
                    }),
            ),
    )
}

fn selected_icon_label(path: Option<&str>) -> Option<SharedString> {
    path.and_then(|path| Path::new(path).file_name())
        .and_then(|file_name| file_name.to_str())
        .map(|file_name| SharedString::from(file_name.to_owned()))
}

fn preview_icon_path(
    selected_icon_path: Option<&str>,
    saved_icon_path: Option<&str>,
    version_name: &str,
) -> LaunchVersionIcon {
    launch_version_icon_path(selected_icon_path.or(saved_icon_path), version_name)
}

#[cfg(test)]
mod tests {
    use super::{preview_icon_path, selected_icon_label};
    use crate::ui::hooks::use_local_versions::LaunchVersionIcon;
    use gpui::SharedString;
    use std::path::PathBuf;

    #[test]
    fn selected_icon_label_uses_the_source_file_name() {
        assert_eq!(
            selected_icon_label(Some("C:\\Images\\custom.png")),
            Some(SharedString::from("custom.png"))
        );
    }

    #[test]
    fn preview_icon_path_prefers_the_unsaved_selection() {
        assert_eq!(
            preview_icon_path(
                Some("C:\\Images\\selected.png"),
                Some("C:\\Games\\version\\icon.png"),
                "26.21",
            ),
            LaunchVersionIcon::Local(PathBuf::from("C:\\Images\\selected.png")),
        );
    }

    #[test]
    fn preview_icon_path_uses_the_saved_icon_without_a_new_selection() {
        assert_eq!(
            preview_icon_path(None, Some("C:\\Games\\version\\icon.png"), "26.21"),
            LaunchVersionIcon::Local(PathBuf::from("C:\\Games\\version\\icon.png")),
        );
    }
}

use std::rc::Rc;

use gpui::AnyElement;

use crate::ui::components::{dialog, modal::ModalDismissHandle};
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;

use super::actions;

pub(crate) fn render_minecraft_termination_dialog(
    colors: &ThemeColors,
    state: &ToolsPageState,
    i18n: &I18n,
) -> Option<AnyElement> {
    let dialog_state = &state.minecraft_termination_dialog;
    if !dialog_state.open {
        return None;
    }

    let error = dialog_state
        .error
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let description = t!("Tools.termination.description", error = error);

    let dismiss_handle = ModalDismissHandle::new();
    Some(dialog::confirm_dialog(
        i18n,
        colors,
        i18n.t_key(crate::i18n_key!("Tools.termination.title")),
        description,
        i18n.t_key(crate::i18n_key!("Tools.termination.confirm")),
        true,
        dialog_state.pending,
        dismiss_handle,
        Rc::new(actions::dismiss_minecraft_termination_dialog),
        |_event, _window, cx| {
            actions::confirm_minecraft_termination(cx);
        },
    ))
}

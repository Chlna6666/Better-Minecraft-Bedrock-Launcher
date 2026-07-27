use std::rc::Rc;

use gpui::{AnyElement, SharedString};

use crate::ui::components::{dialog, modal::ModalDismissHandle};
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::tools::state::ToolsPageState;

use super::actions;

pub(crate) fn render_minecraft_termination_dialog(
    colors: &ThemeColors,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    let dialog_state = &state.minecraft_termination_dialog;
    if !dialog_state.open {
        return None;
    }

    let mut description = String::from(
        "BMCBL 将查询并强制结束实际占用 UDP 7551 的应用。该应用通常是 Minecraft 基岩版；如果游戏正在运行，未保存的世界数据和当前进度可能丢失。建议先回到游戏保存世界并正常退出，只有无法正常关闭时才继续。",
    );
    if let Some(error) = dialog_state.error.as_ref() {
        description.push_str("\n\n");
        description.push_str(error.as_ref());
    }

    let dismiss_handle = ModalDismissHandle::new();
    Some(dialog::confirm_dialog(
        colors,
        "确认结束 UDP 7551 占用应用？",
        SharedString::from(description),
        "仍要结束应用",
        true,
        dialog_state.pending,
        dismiss_handle,
        Rc::new(actions::dismiss_minecraft_termination_dialog),
        |_event, _window, cx| {
            actions::confirm_minecraft_termination(cx);
        },
    ))
}

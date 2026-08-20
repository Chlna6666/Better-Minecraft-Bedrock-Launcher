use gpui::{App, SharedString, Window};

use crate::core::minecraft::local_package::{
    LOCAL_GAME_PACKAGE_EXTENSIONS, start_local_game_package_import,
};
use crate::ui::components::toast;
use crate::utils::file_picker::pick_file_path_with_filter_for_window;

pub(super) fn pick_and_import_local_version(window: &Window, cx: &mut App) {
    let Some(path) = pick_file_path_with_filter_for_window(
        window,
        "Minecraft 游戏版本安装包",
        LOCAL_GAME_PACKAGE_EXTENSIONS,
    ) else {
        return;
    };

    cx.spawn(async move |cx| {
        let result = start_local_game_package_import(path).await;
        cx.update(|cx| match result {
            Ok(_) => toast::push(cx, SharedString::from("游戏版本导入任务已开始")),
            Err(error) => toast::error(cx, SharedString::from(error)),
        })
    })
    .detach();
}

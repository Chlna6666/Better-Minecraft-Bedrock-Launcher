use gpui::{App, SharedString, Window};

use crate::core::minecraft::local_package::{
    LOCAL_GAME_PACKAGE_EXTENSIONS, start_local_game_package_import,
};
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::utils::file_picker::pick_file_path_with_filter_for_window;

pub(super) fn pick_and_import_local_version(window: &Window, cx: &mut App) {
    let file_filter = t!("DownloadPage.game_package_filter");
    let Some(path) = pick_file_path_with_filter_for_window(
        window,
        file_filter.as_ref(),
        LOCAL_GAME_PACKAGE_EXTENSIONS,
    ) else {
        return;
    };

    #[cfg(target_os = "windows")]
    {
        let is_uwp = std::path::Path::new(&path)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("appx") || extension.eq_ignore_ascii_case("zip")
            });
        if is_uwp {
            crate::ui::onboarding::uwp_safety::request_import(cx);
        }
    }

    cx.spawn(async move |cx| {
        let result = start_local_game_package_import(path).await;
        cx.update(|cx| match result {
            Ok(_) => toast::push(cx, t!("Import.game_import_started")),
            Err(error) => toast::error(cx, SharedString::from(error)),
        })
    })
    .detach();
}

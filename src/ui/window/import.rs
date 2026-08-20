pub mod view;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::*;

use crate::launch::ImportLaunchContext;
use crate::ui::components::{modal, toast};
use crate::ui::theme::colors::ThemeColors;

pub use view::ImportWindowView;

pub const IMPORT_ASSET_EXTENSIONS: &[&str] = &["mcpack", "mcworld", "mcaddon", "mctemplate", "zip"];
const UNAMBIGUOUS_GAME_PACKAGE_EXTENSIONS: &[&str] = &["appx", "msixvc"];

#[derive(Clone, Debug, Default)]
pub struct ImportWindowTarget {
    pub version_folder: Option<SharedString>,
    pub instance_name: Option<SharedString>,
    pub game_version: Option<SharedString>,
    pub user_id: Option<SharedString>,
    pub lock_version: bool,
}

impl ImportWindowTarget {
    pub fn locked(
        version_folder: SharedString,
        instance_name: SharedString,
        game_version: SharedString,
        user_id: Option<SharedString>,
    ) -> Self {
        Self {
            version_folder: Some(version_folder),
            instance_name: Some(instance_name),
            game_version: Some(game_version),
            user_id,
            lock_version: true,
        }
    }
}

pub fn pick_and_open_import_window(
    window: &mut Window,
    filter_name: &str,
    extensions: &[&str],
    target: ImportWindowTarget,
    cx: &mut App,
) {
    let Some(file_path) = crate::utils::file_picker::pick_file_path_with_filter_for_window(
        window,
        filter_name,
        extensions,
    ) else {
        return;
    };
    open_import_overlay(PathBuf::from(file_path), target, window, cx);
}

pub fn open_dropped_import(
    paths: &[PathBuf],
    extensions: &[&str],
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    let mut supported = paths
        .iter()
        .filter(|path| has_supported_extension(path, extensions));
    let Some(file_path) = supported.next().cloned() else {
        toast::error(
            cx,
            SharedString::from(format!("支持的导入文件类型：{}", extensions.join("、"))),
        );
        return;
    };
    if supported.next().is_some() {
        toast::push(
            cx,
            SharedString::from("一次处理一个导入包，已打开第一个文件"),
        );
    }
    open_import_overlay(file_path, target, window, cx);
}

pub fn open_dropped_import_any(paths: &[PathBuf], window: &mut Window, cx: &mut App) {
    let mut supported = paths.iter().filter(|path| {
        has_supported_extension(path, IMPORT_ASSET_EXTENSIONS)
            || has_supported_extension(path, UNAMBIGUOUS_GAME_PACKAGE_EXTENSIONS)
    });
    let Some(file_path) = supported.next().cloned() else {
        toast::error(
            cx,
            SharedString::from(
                "不支持该文件；游戏版本支持 APPX/MSIXVC，资源支持 MCPACK/MCADDON/MCWORLD/MCTEMPLATE/ZIP",
            ),
        );
        return;
    };
    if supported.next().is_some() {
        toast::push(
            cx,
            SharedString::from("一次处理一个导入文件，已打开第一个支持的文件"),
        );
    }
    if has_supported_extension(&file_path, UNAMBIGUOUS_GAME_PACKAGE_EXTENSIONS) {
        start_game_package_import(file_path, cx);
    } else {
        open_import_overlay(file_path, ImportWindowTarget::default(), window, cx);
    }
}

pub fn open_import_path(file_path: PathBuf, target: ImportWindowTarget, cx: &mut App) {
    if has_supported_extension(&file_path, UNAMBIGUOUS_GAME_PACKAGE_EXTENSIONS) {
        start_game_package_import(file_path, cx);
        return;
    }

    open_import_window(file_path, target, cx);
}

pub fn open_import_overlay(
    file_path: PathBuf,
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    crate::ui::state::import::show_import_overlay(
        ImportLaunchContext { file_path },
        target,
        window,
        cx,
    );
}

pub fn render_import_overlay(colors: &ThemeColors, cx: &mut App) -> Option<AnyElement> {
    let entry = cx
        .try_global::<crate::ui::state::import::ImportOverlayState>()?
        .active
        .clone()?;
    let dismiss = Rc::new(crate::ui::state::import::clear_import_overlay);
    let surface = modal::modal_surface(
        colors.settings_panel_bg,
        colors.border,
        px(900.0),
        px(560.0),
        px(crate::ui::theme::tokens::radius::XL),
    )
    .shadow_lg()
    .child(entry.view);
    Some(modal::modal_layer_dismissible_with_handle(
        entry.dismiss,
        surface,
        colors.backdrop,
        dismiss,
    ))
}

fn start_game_package_import(file_path: PathBuf, cx: &mut App) {
    cx.spawn(async move |cx| {
        let result =
            crate::core::minecraft::local_package::start_local_game_package_import(file_path).await;
        cx.update(|cx| match result {
            Ok(_) => toast::push(cx, SharedString::from("游戏版本导入任务已开始")),
            Err(error) => toast::error(cx, SharedString::from(error)),
        })
    })
    .detach();
}

pub fn open_import_window(file_path: PathBuf, target: ImportWindowTarget, cx: &mut App) {
    let window_options = import_window_options(cx);
    let import_view = Rc::new(RefCell::new(None));
    let import_view_in_closure = Rc::clone(&import_view);
    let import_context = ImportLaunchContext { file_path };
    let import_window = cx.open_window(window_options, move |window, cx| {
        window.set_title("资源导入");
        let view = cx.new(|cx| {
            ImportWindowView::new(
                import_context,
                target,
                view::ImportPresentation::Window,
                window,
                cx,
            )
        });
        *import_view_in_closure.borrow_mut() = Some(view.downgrade());
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    match import_window {
        Ok(handle) => {
            if let Some(import_view) = import_view.borrow().clone() {
                let window_id = handle.window_id().as_u64();
                if let Err(error) = import_view.update(cx, |view, cx| {
                    view.attach_window_id(window_id, cx);
                }) {
                    tracing::warn!("attach import window id failed: {error:?}");
                }
            }
        }
        Err(error) => crate::result::show_application_error_in_app(
            cx,
            "导入窗口打开失败",
            "open_import_window",
            format!("Failed to open import window: {error:#?}"),
        ),
    }
}

fn has_supported_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn import_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    let fixed_size = size(px(980.), px(720.));
    options.window_bounds = Some(WindowBounds::centered(fixed_size, cx));
    options.window_min_size = Some(fixed_size);
    options.is_resizable = false;
    options.is_minimizable = true;
    options.is_movable = true;

    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(SharedString::from("资源导入")),
            appears_transparent: true,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Transparent;
    }

    options
}

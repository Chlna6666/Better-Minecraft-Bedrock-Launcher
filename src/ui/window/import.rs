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
    let file_paths = crate::utils::file_picker::pick_file_paths_with_filter_for_window(
        window,
        filter_name,
        extensions,
    )
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    if file_paths.is_empty() {
        return;
    }
    open_import_overlay_batch(file_paths, target, window, cx);
}

pub fn open_dropped_import(
    paths: &[PathBuf],
    extensions: &[&str],
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    let supported = paths
        .iter()
        .filter(|path| has_supported_extension(path, extensions))
        .cloned()
        .collect::<Vec<_>>();
    if supported.is_empty() {
        toast::error(
            cx,
            t!(
                "Import.supported_types",
                extensions = &extensions.join(", ")
            ),
        );
        return;
    }
    open_import_overlay_batch(supported, target, window, cx);
}

pub fn open_dropped_import_any(paths: &[PathBuf], window: &mut Window, cx: &mut App) {
    let mut asset_paths = Vec::new();
    let mut game_package_paths = Vec::new();

    for path in paths {
        if has_supported_extension(path, UNAMBIGUOUS_GAME_PACKAGE_EXTENSIONS) {
            game_package_paths.push(path.clone());
        } else if has_supported_extension(path, IMPORT_ASSET_EXTENSIONS) {
            asset_paths.push(path.clone());
        }
    }

    if asset_paths.is_empty() && game_package_paths.is_empty() {
        toast::error(cx, t!("Import.unsupported_file"));
        return;
    }

    for file_path in game_package_paths {
        start_game_package_import(file_path, cx);
    }
    if !asset_paths.is_empty() {
        open_import_overlay_batch(
            asset_paths,
            ImportWindowTarget::default(),
            window,
            cx,
        );
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
    open_import_overlay_batch(vec![file_path], target, window, cx);
}

pub fn open_import_overlay_batch(
    file_paths: Vec<PathBuf>,
    target: ImportWindowTarget,
    window: &mut Window,
    cx: &mut App,
) {
    crate::ui::state::import::show_import_overlay_batch(file_paths, target, window, cx);
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
            Ok(_) => toast::push(cx, t!("Import.game_import_started")),
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
        window.set_title(t!("Import.title").as_ref());
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
            t!("Import.open_failed").as_ref(),
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
            title: Some(t!("Import.title")),
            appears_transparent: true,
            ..Default::default()
        });
        options.window_corner_preference = WindowCornerPreference::Rounded;
        // Match the main window's native DWM corner treatment. The import view paints the
        // complete client surface, so the window shape itself must own the outer R corners.
        // This view paints every client pixel. An opaque surface preserves the custom titlebar
        // while keeping a normal DWM redirection bitmap, so capture tools can identify the HWND.
        options.window_background = WindowBackgroundAppearance::Opaque;
    }

    options
}

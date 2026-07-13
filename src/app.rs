use crate::http::gpui_client::create_gpui_http_client;
use crate::i18n::Locale;
use crate::launch::LaunchMode;
use crate::ui::state::i18n::I18n;
use anyhow::Result;
use gpui::*;
use std::env;
use std::process;
use std::time::Duration;
use tracing::{debug, warn};

pub const APP_ID: &str = "com.bmcbl.app";
const DEBUG_WINDOW_STARTUP_DELAY: Duration = Duration::from_millis(900);
const STARTUP_WARMUP_DELAY: Duration = Duration::from_millis(1500);

pub(crate) struct AppBootstrap {
    debug_enabled: bool,
    theme_color_hex: String,
    theme_mode: String,
    initial_locale: Locale,
    renderer_backend: gpui::RendererBackend,
    gpu_adapter_name: Option<String>,
    startup_check_updates: bool,
    agreement_accepted: bool,
    launch_mode: LaunchMode,
    font_source: String,
    local_font_path: String,
    local_font_family: String,
    system_font_family: String,
    config: crate::config::config::Config,
}

impl AppBootstrap {
    pub(crate) async fn from_config(
        config: &crate::config::config::Config,
        launch_mode: LaunchMode,
    ) -> Self {
        let locale_code = match config.launcher.language.as_str() {
            "auto" => crate::utils::system_info::get_system_language(),
            "" => "en-US".to_string(),
            other => other.replace('_', "-"),
        };

        let renderer_backend = renderer_backend_from_config(&config.launcher.renderer_backend);
        let gpu_adapter_name =
            gpu_adapter_name_from_config(renderer_backend, &config.launcher.gpu_adapter_name).await;

        Self {
            debug_enabled: config.launcher.debug,
            theme_color_hex: config.custom_style.theme_color.clone(),
            theme_mode: config.custom_style.theme_mode.clone(),
            initial_locale: Locale::from_code(&locale_code).unwrap_or(Locale::EnUs),
            renderer_backend,
            gpu_adapter_name,
            startup_check_updates: config.launcher.auto_check_updates,
            agreement_accepted: config.agreement_accepted,
            launch_mode,
            font_source: config.custom_style.font_source.clone(),
            local_font_path: config.custom_style.local_font_path.clone(),
            local_font_family: config.custom_style.local_font_family.clone(),
            system_font_family: config.custom_style.system_font_family.clone(),
            config: config.clone(),
        }
    }
}

fn renderer_backend_from_config(renderer_backend: &str) -> gpui::RendererBackend {
    let normalized = crate::config::config::normalize_renderer_backend(renderer_backend);
    normalized
        .parse::<gpui::RendererBackend>()
        .unwrap_or_default()
}

async fn gpu_adapter_name_from_config(
    renderer_backend: gpui::RendererBackend,
    gpu_adapter_name: &str,
) -> Option<String> {
    let gpu_adapter_name = crate::config::config::normalize_gpu_adapter_name(gpu_adapter_name);
    if gpu_adapter_name == crate::config::config::default_gpu_adapter_name() {
        return None;
    }

    let adapters = match tokio::task::spawn_blocking(move || {
        gpui::enumerate_gpu_adapters(renderer_backend)
    })
    .await
    {
        Ok(adapters) => adapters,
        Err(error) => {
            warn!(
                ?error,
                gpu_adapter_name = %gpu_adapter_name,
                "failed to enumerate GPU adapters; resetting configured adapter to automatic selection"
            );
            reset_gpu_adapter_name_to_auto(&gpu_adapter_name);
            return None;
        }
    };

    if adapters.iter().any(|adapter| {
        adapter
            .name
            .trim()
            .eq_ignore_ascii_case(gpu_adapter_name.as_str())
    }) {
        return Some(gpu_adapter_name);
    }

    let available_gpu_adapters = adapters
        .iter()
        .map(|adapter| adapter.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    warn!(
        gpu_adapter_name = %gpu_adapter_name,
        available_gpu_adapters = %available_gpu_adapters,
        "configured GPU adapter is unavailable; resetting to automatic selection"
    );
    reset_gpu_adapter_name_to_auto(&gpu_adapter_name);
    None
}

fn reset_gpu_adapter_name_to_auto(gpu_adapter_name: &str) {
    let default_gpu_adapter_name = crate::config::config::default_gpu_adapter_name();
    if let Err(error) = crate::config::config::update_config(|config| {
        config.launcher.gpu_adapter_name = default_gpu_adapter_name;
    }) {
        warn!(
            ?error,
            gpu_adapter_name = %gpu_adapter_name,
            "failed to persist GPU adapter automatic reset"
        );
    }
}

fn application_default_font(bootstrap: &AppBootstrap) -> gpui::DefaultFontConfig {
    crate::utils::font_settings::font_config_for_selection(
        &bootstrap.font_source,
        &bootstrap.local_font_path,
        &bootstrap.local_font_family,
        &bootstrap.system_font_family,
    )
}

fn gpu_power_preference_for_adapter(_gpu_adapter_name: Option<&str>) -> gpui::GpuPowerPreference {
    gpui::GpuPowerPreference::HighPerformance
}

#[derive(Default)]
pub(crate) struct AppSubscriptions {
    window_close: Option<Subscription>,
}

impl Global for AppSubscriptions {}

#[cfg(windows)]
fn configure_platform_app_identity() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use windows::core::PCWSTR;

    let wide: Vec<u16> = OsStr::new(APP_ID)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `wide` is NUL-terminated and remains alive for the duration of the call.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(PCWSTR(wide.as_ptr()));
    }
}

#[cfg(not(windows))]
fn configure_platform_app_identity() {}

pub(crate) fn run(bootstrap: AppBootstrap) -> Result<()> {
    configure_platform_app_identity();

    let app = Application::new_with_renderer_options(gpui::RendererOptions {
        backend: bootstrap.renderer_backend,
        adapter_name: bootstrap.gpu_adapter_name.clone(),
        power_preference: gpu_power_preference_for_adapter(bootstrap.gpu_adapter_name.as_deref()),
        ..gpui::RendererOptions::default()
    })
    .with_image_pipeline_config(gpui::ImagePipelineConfig {
        animated: gpui::AnimatedImageConfig {
            play: true,
            max_gpu_frame_slots: 3,
            max_fps: 60.0,
            inactive_max_fps: 1.0,
            decode_ahead_frames: 4,
            max_resident_frames: 16,
            max_resident_bytes: 4 * 1024 * 1024,
            ..gpui::AnimatedImageConfig::default()
        },
        max_decoded_bytes: image_pipeline_decoded_budget_bytes(),
        slow_decode_threshold: Duration::from_millis(16),
        slow_upload_bytes: 8 * 1024 * 1024,
        slow_upload_threshold: Duration::from_millis(4),
    })
    .with_default_font_or_platform_default(application_default_font(&bootstrap))
    .with_assets(crate::assets::asset_source::AppAssets);
    app.run(move |cx| {
        configure_runtime(cx, &bootstrap.launch_mode);
        build_app_state(cx, &bootstrap);
        crate::plugins::runtime::init(cx);

        gpui_router::init(cx);
        if matches!(bootstrap.launch_mode, LaunchMode::Main) {
            let preloaded_backgrounds =
                crate::ui::main_window::preload_startup_background_bytes_from_values(
                    &bootstrap.config.custom_style.background_option,
                    &bootstrap.config.custom_style.local_image_path,
                    &bootstrap.config.custom_style.network_image_url,
                    cx,
                );
            if preloaded_backgrounds > 0 {
                debug!("startup background compressed image bytes preload scheduled");
            }
            crate::ui::navigation::set_route(cx, crate::ui::navigation::AppRoute::Home);
            let main_window_opened = open_main_window(&bootstrap, cx);
            if main_window_opened {
                schedule_post_startup_warmups(cx);
                if bootstrap.debug_enabled {
                    schedule_debug_window_after_startup(cx);
                }
            }
        } else if let LaunchMode::Import(ref import_context) = bootstrap.launch_mode {
            open_import_window(import_context.clone(), cx);
        }

        register_app_lifecycle(cx);
        if bootstrap.launch_mode.is_main() {
            start_background_maintenance();
        }
    });

    Ok(())
}

fn image_pipeline_decoded_budget_bytes() -> usize {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let total_memory_bytes = system.total_memory();
    let system_budget = usize::try_from(total_memory_bytes / 64).unwrap_or(16 * 1024 * 1024);

    system_budget.clamp(16 * 1024 * 1024, 64 * 1024 * 1024)
}

fn configure_runtime(cx: &mut App, launch_mode: &LaunchMode) {
    if let Err(error) = crate::assets::load_startup_fonts(cx) {
        eprintln!("Failed to load startup fonts: {error:?}");
    }
    crate::assets::spawn_deferred_font_load(cx);

    crate::ui::components::input::init(cx);
    crate::ui::components::code_editor::init(cx);
    crate::ui::window::map_viewer::init(cx);
    if launch_mode.is_main() {
        crate::ui::views::download::init(cx);
        install_http_client(cx);
    }
}

fn install_http_client(cx: &mut App) {
    cx.set_http_client(create_gpui_http_client());
}

fn build_app_state(cx: &mut App, bootstrap: &AppBootstrap) {
    let config = &bootstrap.config;
    let startup_background_option = config.custom_style.background_option.clone();
    let startup_local_image_path = config.custom_style.local_image_path.clone();
    let startup_network_image_url = config.custom_style.network_image_url.clone();
    if bootstrap.launch_mode.is_main() {
        tracing::info!(
            "startup_trace: config_ready t={:.3}ms background_option={} local_path_len={} network_url_len={} phase=app_state",
            crate::ui::main_window::startup_trace_elapsed_ms(),
            startup_background_option,
            startup_local_image_path.len(),
            startup_network_image_url.len()
        );
    }

    cx.default_global::<I18n>();
    cx.default_global::<crate::ui::state::navigation::NavState>();
    cx.default_global::<crate::ui::views::download::state::DownloadPageState>();
    cx.default_global::<crate::ui::state::local_versions::LocalVersionsState>();
    cx.default_global::<crate::ui::state::launcher::LauncherState>();
    cx.default_global::<crate::ui::state::launch_prereq::LaunchPrereqState>();
    cx.default_global::<crate::ui::views::manage::state::ManagePageState>();
    cx.default_global::<crate::ui::views::tools::state::ToolsPageState>();
    cx.default_global::<crate::ui::views::settings::state::SettingsPageState>();
    cx.default_global::<crate::ui::state::quit::QuitState>();
    cx.default_global::<crate::ui::state::theme::ThemeState>();
    cx.default_global::<crate::ui::state::update::UpdateState>();
    cx.default_global::<crate::ui::state::agreement::AgreementState>();
    cx.default_global::<crate::ui::state::diagnostics::DiagnosticsState>();
    cx.default_global::<crate::ui::state::music::MusicState>();
    cx.default_global::<crate::ui::main_window::AppChromeState>();
    cx.default_global::<crate::ui::components::toast::ToastState>();
    cx.default_global::<crate::ui::components::dropdown::DropdownOverlayState>();
    cx.default_global::<crate::ui::window::debug::DebugState>();
    cx.default_global::<crate::plugins::runtime::PluginRegistry>();
    cx.default_global::<AppSubscriptions>();

    cx.update_global(|i18n: &mut I18n, _cx| {
        i18n.set_locale(bootstrap.initial_locale);
    });

    crate::ui::state::theme::ThemeState::apply_startup_config(
        &bootstrap.theme_color_hex,
        &bootstrap.theme_mode,
        cx,
    );

    cx.update_global(
        |agreement: &mut crate::ui::state::agreement::AgreementState, _cx| {
            agreement.initialize(bootstrap.agreement_accepted);
        },
    );

    cx.update_global(
        |debug_state: &mut crate::ui::window::debug::DebugState, _cx| {
            debug_state.enabled = bootstrap.debug_enabled;
            debug_state.main_window_id = None;
            debug_state.debug_window_id = None;
            debug_state.reset_runtime_state();

            if let Ok(exe_path) = env::current_exe() {
                debug_state.exe_path = SharedString::from(exe_path.to_string_lossy().to_string());
                if let Ok(metadata) = std::fs::metadata(&exe_path) {
                    debug_state.exe_size_bytes = metadata.len();
                }
            }
        },
    );

    if bootstrap.debug_enabled {
        crate::ui::window::debug::devtools::configure_devtools(cx);
    }

    cx.update_global(
        |settings: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
            settings.apply_config(&config);
        },
    );

    if bootstrap.launch_mode.is_main() {
        match crate::utils::diagnostics::load_pending_report() {
            Ok(report) => {
                cx.update_global(
                    |diagnostics: &mut crate::ui::state::diagnostics::DiagnosticsState, _cx| {
                        diagnostics.set_pending_report(report);
                    },
                );
            }
            Err(error) => {
                tracing::warn!(?error, "failed to load pending diagnostics report");
            }
        }
    }
}

fn open_main_window(bootstrap: &AppBootstrap, cx: &mut App) -> bool {
    let startup_check_updates = bootstrap.startup_check_updates;
    let startup_background_option = bootstrap.config.custom_style.background_option.clone();
    let startup_local_image_path = bootstrap.config.custom_style.local_image_path.clone();
    let startup_network_image_url = bootstrap.config.custom_style.network_image_url.clone();
    let window_title = crate::utils::app_info::runtime_app_name();
    let window_options = main_window_options(&window_title, cx);
    let main_window = cx.open_window(window_options, move |window, cx| {
        window.set_title(&window_title);
        let preloaded_targets =
            crate::ui::main_window::preload_startup_background_target_from_values(
                &startup_background_option,
                &startup_local_image_path,
                &startup_network_image_url,
                window,
                cx,
            );
        if preloaded_targets > 0 {
            debug!("startup background target image preload scheduled");
        }

        let view = cx.new(|cx| {
            let view =
                crate::ui::main_window::MainWindowView::new(startup_check_updates, window, cx);
            view
        });
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    match main_window {
        Ok(handle) => {
            cx.update_global(
                |debug_state: &mut crate::ui::window::debug::DebugState, _cx| {
                    debug_state.main_window_id = Some(handle.window_id().as_u64());
                },
            );
            true
        }
        Err(error) => {
            eprintln!("Failed to open window: {error:?}");
            crate::result::show_application_error_in_app(
                cx,
                "主窗口打开失败",
                "open_main_window",
                format!("Failed to open window: {error:#?}"),
            );
            false
        }
    }
}

fn schedule_debug_window_after_startup(cx: &mut App) {
    cx.spawn(async move |cx| {
        Timer::after(DEBUG_WINDOW_STARTUP_DELAY).await;

        match cx.update(|cx| {
            let already_open =
                cx.read_global(|state: &crate::ui::window::debug::DebugState, _cx| {
                    state.debug_window_id.is_some()
                });
            if !already_open {
                open_debug_window(cx);
            }
        }) {
            Ok(()) => {}
            Err(error) => warn!("delayed debug window open failed: {error:?}"),
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn schedule_post_startup_warmups(cx: &mut App) {
    cx.spawn(async move |_cx| {
        Timer::after(STARTUP_WARMUP_DELAY).await;

        let markdown_warmup = tokio::task::spawn_blocking(
            crate::ui::components::markdown_renderer::warm_highlighter_assets,
        );
        let http_warmup =
            tokio::task::spawn_blocking(crate::http::proxy::prewarm_current_proxy_clients);

        match markdown_warmup.await {
            Ok(()) => debug!("post-startup markdown highlighter warmup finished"),
            Err(error) => warn!("post-startup markdown highlighter warmup failed: {error}"),
        }

        match http_warmup.await {
            Ok(Ok(())) => debug!("post-startup HTTP client warmup finished"),
            Ok(Err(error)) => warn!("post-startup HTTP client warmup failed: {error}"),
            Err(error) => warn!("post-startup HTTP client warmup task failed: {error}"),
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

fn open_debug_window(cx: &mut App) {
    let window_title = format!("{} Debug", crate::utils::app_info::runtime_app_name());
    let window_options = debug_window_options(&window_title, cx);
    let debug_window = cx.open_window(window_options, move |window, cx| {
        window.set_title(&window_title);

        let view = cx.new(|cx| crate::ui::window::debug::DebugView::new(window, cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    match debug_window {
        Ok(handle) => {
            cx.update_global(
                |debug_state: &mut crate::ui::window::debug::DebugState, _cx| {
                    debug_state.debug_window_id = Some(handle.window_id().as_u64());
                },
            );
        }
        Err(error) => {
            eprintln!("Failed to open debug window: {error:?}");
            crate::result::show_application_error_in_app(
                cx,
                "调试窗口打开失败",
                "open_debug_window",
                format!("Failed to open debug window: {error:#?}"),
            );
        }
    }
}

fn open_import_window(import_context: crate::launch::ImportLaunchContext, cx: &mut App) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let window_options = import_window_options(cx);
    let import_view = Rc::new(RefCell::new(None));
    let import_view_in_closure = Rc::clone(&import_view);
    let import_window = cx.open_window(window_options, move |window, cx| {
        window.set_title("资源导入");

        let view = cx
            .new(|cx| crate::ui::window::import::ImportWindowView::new(import_context, window, cx));
        *import_view_in_closure.borrow_mut() = Some(view.downgrade());
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });

    match import_window {
        Ok(handle) => {
            if let Some(import_view) = import_view.borrow().clone() {
                let window_id = handle.window_id().as_u64();
                let _ = import_view.update(cx, |view, cx| {
                    view.attach_window_id(window_id, cx);
                });
            }
        }
        Err(error) => {
            eprintln!("Failed to open import window: {error:?}");
            crate::result::show_application_error_in_app(
                cx,
                "导入窗口打开失败",
                "open_import_window",
                format!("Failed to open import window: {error:#?}"),
            );
        }
    }
}

fn main_window_options(window_title: &str, cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    let fixed_size = crate::ui::main_window::MAIN_WINDOW_INITIAL_SIZE;
    options.window_bounds = Some(WindowBounds::centered(fixed_size, cx));
    options.window_min_size = Some(fixed_size);
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;

    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(SharedString::from(window_title.to_string())),
            appears_transparent: true,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Transparent;
    }

    options
}

fn debug_window_options(window_title: &str, cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(size(px(1280.), px(860.)), cx));
    options.window_min_size = Some(size(px(760.), px(560.)));
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;

    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(SharedString::from(window_title.to_string())),
            appears_transparent: false,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Opaque;
    }

    options
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

fn register_app_lifecycle(cx: &mut App) {
    let subscription = cx.on_window_closed(|cx| {
        let (main_id, debug_id, debug_enabled) =
            cx.read_global(|debug_state: &crate::ui::window::debug::DebugState, _cx| {
                (
                    debug_state.main_window_id,
                    debug_state.debug_window_id,
                    debug_state.enabled,
                )
            });

        let mut any_window = false;
        let mut has_main = false;
        let mut has_debug = false;
        for window in cx.windows() {
            any_window = true;
            let window_id = window.window_id().as_u64();
            if Some(window_id) == main_id {
                has_main = true;
            }
            if Some(window_id) == debug_id {
                has_debug = true;
            }
        }

        cx.update_global(|debug_state: &mut crate::ui::window::debug::DebugState, _cx| {
            if !has_main {
                debug_state.main_window_id = None;
            }
            if !has_debug {
                debug_state.debug_window_id = None;
                debug_state.reset_runtime_state();
            }
        });

        debug!(
            "window closed main_id={:?} debug_id={:?} has_main={} has_debug={} any_window={} debug_enabled={}",
            main_id,
            debug_id,
            has_main,
            has_debug,
            any_window,
            debug_enabled
        );

        if !any_window {
            if let Err(error) = crate::utils::diagnostics::mark_clean_shutdown() {
                warn!(?error, "failed to mark clean shutdown");
            }
            cx.quit();
            force_exit_after_delay(Duration::from_millis(1500));
        } else if debug_enabled && !has_main && has_debug {
            if let Err(error) = crate::utils::diagnostics::mark_clean_shutdown() {
                warn!(?error, "failed to mark clean shutdown");
            }
            cx.quit();
            force_exit_after_delay(Duration::from_millis(1500));
        }
    });

    cx.update_global(|subscriptions: &mut AppSubscriptions, _cx| {
        subscriptions.window_close = Some(subscription);
    });
}

fn start_background_maintenance() {
    // Keep default runtime maintenance passive. Aggressive memory cleanup and
    // working-set trimming are available from diagnostics when explicitly
    // requested, but running them by default causes background CPU usage and
    // frame pacing regressions.
}

fn force_exit_after_delay(delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        process::exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::gpu_power_preference_for_adapter;
    use gpui::GpuPowerPreference;

    #[test]
    fn automatic_gpu_adapter_uses_high_performance_preference() {
        assert_eq!(
            gpu_power_preference_for_adapter(None),
            GpuPowerPreference::HighPerformance
        );
    }

    #[test]
    fn explicit_gpu_adapter_uses_high_performance_preference() {
        assert_eq!(
            gpu_power_preference_for_adapter(Some("NVIDIA GeForce RTX 4060")),
            GpuPowerPreference::HighPerformance
        );
    }
}

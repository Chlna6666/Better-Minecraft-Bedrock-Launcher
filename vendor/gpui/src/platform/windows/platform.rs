use std::{
    cell::RefCell,
    ffi::OsStr,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ::util::{ResultExt, paths::SanitizedPath};
use anyhow::{Context as _, Result, anyhow, bail};
use async_task::Runnable;
use collections::FxHashMap;
use futures::channel::oneshot::{self, Receiver};
use itertools::Itertools;
use smallvec::SmallVec;
use windows::{
    UI::ViewManagement::UISettings,
    Win32::{
        Foundation::*,
        Security::Credentials::*,
        System::{
            Com::*, Ole::*, ProcessStatus::K32EmptyWorkingSet, SystemInformation::*,
            Threading::GetCurrentProcess,
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, PROCESS_DPI_AWARENESS,
                PROCESS_PER_MONITOR_DPI_AWARE, SetProcessDpiAwareness,
                SetProcessDpiAwarenessContext,
            },
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::*,
};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

use super::{
    apply_cursor_style_to_window, keystroke_from_winit, modifiers_from_winit,
    mouse_button_from_winit,
};
use crate::*;

const DISABLE_DIRECT_COMPOSITION: &str = "GPUI_DISABLE_DIRECT_COMPOSITION";
const DISABLE_STARTUP_WORKING_SET_TRIM: &str = "GPUI_DISABLE_STARTUP_WORKING_SET_TRIM";
const STARTUP_WORKING_SET_TRIM_DELAY: Duration = Duration::from_secs(5);
#[cfg(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan"))]
const WINDOWS_AUTO_RENDERER_BACKEND_ORDER: &[RendererBackend] =
    &[RendererBackend::NovaDx12, RendererBackend::NovaVulkan];
#[cfg(not(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan")))]
const WINDOWS_AUTO_RENDERER_BACKEND_ORDER: &[RendererBackend] = &[RendererBackend::NovaDx12];

pub(super) fn windows_auto_renderer_backend_order() -> &'static [RendererBackend] {
    WINDOWS_AUTO_RENDERER_BACKEND_ORDER
}

fn startup_working_set_trim_enabled() -> bool {
    !std::env::var(DISABLE_STARTUP_WORKING_SET_TRIM)
        .is_ok_and(|value| value == "true" || value == "1")
}

#[cfg(target_os = "windows")]
fn spawn_startup_working_set_trim_task() {
    std::thread::spawn(|| {
        std::thread::sleep(STARTUP_WORKING_SET_TRIM_DELAY);
        unsafe {
            let process = GetCurrentProcess();
            let _ = K32EmptyWorkingSet(process);
        }
    });
}

thread_local! {
    static ACTIVE_CONTEXT: RefCell<Option<(*const ActiveEventLoop, *mut WindowsApplication)>> = const { RefCell::new(None) };
}

fn with_active_context<R>(
    f: impl FnOnce(&ActiveEventLoop, &mut WindowsApplication) -> R,
) -> Option<R> {
    ACTIVE_CONTEXT.with(|storage| {
        let (event_loop, app) = storage.borrow().as_ref().copied()?;
        // SAFETY: The pointers are only set while winit is executing callbacks on the same thread.
        Some(unsafe { f(&*event_loop, &mut *app) })
    })
}

#[derive(Debug, Clone)]
pub(crate) enum WindowsUserEvent {
    RunMainThreadTasks,
    DockMenuAction(usize),
    Quit,
}

pub(crate) struct WindowsPlatform {
    inner: Rc<WindowsPlatformInner>,
    // The below members will never change throughout the entire lifecycle of the app.
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    disable_direct_composition: bool,
    renderer_backend: RendererBackend,
    renderer_options: RendererOptions,
    event_loop_proxy: Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
    ole_initialized: bool,
}

pub(crate) struct WindowsPlatformInner {
    state: RefCell<WindowsPlatformState>,
    // The below members will never change throughout the entire lifecycle of the app.
    main_receiver: flume::Receiver<Runnable>,
    main_thread_wakeup_pending: Arc<AtomicBool>,
}

#[derive(Default)]
struct PendingFileDrop {
    paths: SmallVec<[PathBuf; 2]>,
}

pub(crate) struct WindowsPlatformState {
    callbacks: PlatformCallbacks,
    menus: Vec<OwnedMenu>,
    jump_list: JumpList,
    cursor_style: CursorStyle,
    displays: Vec<WindowsDisplay>,
    primary_display_id: Option<DisplayId>,
    active_window_handle: Option<AnyWindowHandle>,
}

#[derive(Default)]
struct PlatformCallbacks {
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    quit: Option<Box<dyn FnMut()>>,
    reopen: Option<Box<dyn FnMut()>>,
    app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,
    will_open_app_menu: Option<Box<dyn FnMut()>>,
    validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    keyboard_layout_change: Option<Box<dyn FnMut()>>,
}

impl WindowsPlatformState {
    fn new() -> Self {
        let callbacks = PlatformCallbacks::default();
        let jump_list = JumpList::new();

        Self {
            callbacks,
            jump_list,
            cursor_style: CursorStyle::Arrow,
            displays: Vec::new(),
            primary_display_id: None,
            active_window_handle: None,
            menus: Vec::new(),
        }
    }
}

fn create_windows_text_system() -> Result<Arc<dyn PlatformTextSystem>> {
    Ok(Arc::new(DirectWriteTextSystem::new()?))
}

fn become_dpi_aware() {
    if set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok()
        || set_process_dpi_awareness_context(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE).is_ok()
        || set_process_dpi_awareness(PROCESS_PER_MONITOR_DPI_AWARE).is_ok()
    {
        return;
    }

    // SAFETY: This process-wide DPI fallback is called before GPUI creates winit windows.
    unsafe {
        if !SetProcessDPIAware().as_bool() {
            log::debug!("failed to set process DPI awareness with legacy Windows API");
        }
    }
}

fn set_process_dpi_awareness_context(context: DPI_AWARENESS_CONTEXT) -> windows::core::Result<()> {
    // SAFETY: This process-wide DPI setting is only attempted during platform initialization,
    // before any GPUI platform window has been created.
    unsafe { SetProcessDpiAwarenessContext(context) }
}

fn set_process_dpi_awareness(awareness: PROCESS_DPI_AWARENESS) -> windows::core::Result<()> {
    // SAFETY: This process-wide DPI setting is only attempted during platform initialization,
    // before any GPUI platform window has been created.
    unsafe { SetProcessDpiAwareness(awareness) }
}

impl WindowsPlatform {
    fn new_common_parts() -> (
        Rc<WindowsPlatformInner>,
        BackgroundExecutor,
        ForegroundExecutor,
        Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
    ) {
        let (main_sender, main_receiver) = flume::unbounded::<Runnable>();
        let main_thread_wakeup_pending = Arc::new(AtomicBool::new(false));
        let event_loop_proxy = Arc::new(Mutex::new(None));
        let inner = Rc::new(WindowsPlatformInner {
            state: RefCell::new(WindowsPlatformState::new()),
            main_receiver,
            main_thread_wakeup_pending: main_thread_wakeup_pending.clone(),
        });
        let dispatcher = Arc::new(WindowsDispatcher::new(
            main_sender,
            main_thread_wakeup_pending,
            event_loop_proxy.clone(),
        ));
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);

        (
            inner,
            background_executor,
            foreground_executor,
            event_loop_proxy,
        )
    }

    pub(crate) fn new_headless() -> Self {
        let (inner, background_executor, foreground_executor, event_loop_proxy) =
            Self::new_common_parts();
        let renderer_backend = RendererBackend::HeadlessTest;
        let text_system = create_windows_text_system()
            .unwrap_or_else(|_| Arc::new(NoopTextSystem) as Arc<dyn PlatformTextSystem>);

        Self {
            inner,
            background_executor,
            foreground_executor,
            text_system,
            disable_direct_composition: true,
            renderer_backend,
            renderer_options: RendererOptions::with_backend(renderer_backend),
            event_loop_proxy,
            ole_initialized: false,
        }
    }

    fn resolve_renderer_backend(renderer_options: &RendererOptions) -> Result<RendererBackend> {
        match renderer_options.backend {
            RendererBackend::Auto => {
                resolve_auto_renderer_backend(WINDOWS_AUTO_RENDERER_BACKEND_ORDER, |backend| {
                    match backend {
                        RendererBackend::NovaDx12 => dx12_renderer_backend_is_available(),
                        RendererBackend::NovaVulkan => vulkan_renderer_backend_is_available(),
                        RendererBackend::Auto
                        | RendererBackend::NovaMetal
                        | RendererBackend::HeadlessTest => {
                            Err(anyhow!("{backend} is not a Windows auto GPU backend"))
                        }
                    }
                })
            }
            RendererBackend::NovaVulkan => Ok(RendererBackend::NovaVulkan),
            RendererBackend::NovaDx12 | RendererBackend::NovaMetal => Ok(RendererBackend::NovaDx12),
            RendererBackend::HeadlessTest => {
                Err(anyhow!("headless test is not a Windows GPU backend"))
            }
        }
    }

    pub(crate) fn new(renderer_options: RendererOptions) -> Result<Self> {
        become_dpi_aware();
        unsafe {
            OleInitialize(None).context("unable to initialize Windows OLE")?;
        }
        let requested_renderer_backend = renderer_options.backend;
        let renderer_backend = match requested_renderer_backend {
            RendererBackend::HeadlessTest => RendererBackend::HeadlessTest,
            RendererBackend::Auto
            | RendererBackend::NovaVulkan
            | RendererBackend::NovaDx12
            | RendererBackend::NovaMetal => Self::resolve_renderer_backend(&renderer_options)?,
        };
        record_renderer_backend(renderer_backend);
        if matches!(
            requested_renderer_backend,
            RendererBackend::Auto
                | RendererBackend::NovaVulkan
                | RendererBackend::NovaDx12
                | RendererBackend::NovaMetal
        ) {
            log::info!(
                "GPUI Windows resolved renderer backend: {}",
                renderer_backend
            );
        }
        let text_system = create_windows_text_system()?;
        let disable_direct_composition = std::env::var(DISABLE_DIRECT_COMPOSITION)
            .is_ok_and(|value| value == "true" || value == "1");
        let (inner, background_executor, foreground_executor, event_loop_proxy) =
            Self::new_common_parts();

        if startup_working_set_trim_enabled() {
            spawn_startup_working_set_trim_task();
        }

        Ok(Self {
            inner,
            background_executor,
            foreground_executor,
            text_system,
            disable_direct_composition,
            renderer_backend,
            renderer_options,
            event_loop_proxy,
            ole_initialized: true,
        })
    }

    fn generate_creation_info(&self) -> WindowCreationInfo {
        WindowCreationInfo {
            background_executor: self.background_executor.clone(),
            executor: self.foreground_executor.clone(),
            disable_direct_composition: self.disable_direct_composition,
            renderer_backend: self.renderer_backend,
            renderer_options: self.renderer_options.clone(),
        }
    }

    fn set_dock_menus(&self, menus: Vec<MenuItem>) {
        let mut actions = Vec::new();
        menus.into_iter().for_each(|menu| {
            if let Some(dock_menu) = DockMenuItem::new(menu).log_err() {
                actions.push(dock_menu);
            }
        });
        let mut lock = self.inner.state.borrow_mut();
        lock.jump_list.dock_menus = actions;
        update_jump_list(&lock.jump_list).log_err();
    }

    fn update_jump_list(
        &self,
        menus: Vec<MenuItem>,
        entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Vec<SmallVec<[PathBuf; 2]>> {
        let mut actions = Vec::new();
        menus.into_iter().for_each(|menu| {
            if let Some(dock_menu) = DockMenuItem::new(menu).log_err() {
                actions.push(dock_menu);
            }
        });
        let mut lock = self.inner.state.borrow_mut();
        lock.jump_list.dock_menus = actions;
        lock.jump_list.recent_workspaces = entries;
        update_jump_list(&lock.jump_list)
            .log_err()
            .unwrap_or_default()
    }
}

fn resolve_auto_renderer_backend(
    backends: &[RendererBackend],
    mut backend_available: impl FnMut(RendererBackend) -> Result<()>,
) -> Result<RendererBackend> {
    let mut unavailable = Vec::new();

    for backend in backends {
        match backend_available(*backend) {
            Ok(()) => return Ok(*backend),
            Err(error) => {
                log::warn!("GPUI Windows auto renderer skipped {backend}: {error}");
                unavailable.push(format!("{backend}: {error}"));
            }
        }
    }

    if unavailable.is_empty() {
        bail!("GPUI Windows auto renderer has no compiled GPU backends");
    }

    bail!(
        "GPUI Windows auto renderer found no usable GPU backend; checked {}",
        unavailable.join("; ")
    );
}

fn dx12_renderer_backend_is_available() -> Result<()> {
    #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
    {
        backend_has_adapters(
            RendererBackend::NovaDx12,
            gfx_dx12::enumerate_adapter_info(),
        )
    }

    #[cfg(not(all(target_os = "windows", feature = "nova-gfx-dx12")))]
    {
        bail!("nova-gfx DX12 renderer was not compiled in");
    }
}

fn vulkan_renderer_backend_is_available() -> Result<()> {
    #[cfg(all(target_os = "windows", feature = "nova-gfx-vulkan"))]
    {
        backend_has_adapters(
            RendererBackend::NovaVulkan,
            gfx_vulkan::enumerate_adapter_info(),
        )
    }

    #[cfg(not(all(target_os = "windows", feature = "nova-gfx-vulkan")))]
    {
        bail!("nova-gfx Vulkan renderer was not compiled in");
    }
}

#[cfg(any(
    all(target_os = "windows", feature = "nova-gfx-dx12"),
    all(target_os = "windows", feature = "nova-gfx-vulkan")
))]
fn backend_has_adapters(
    backend: RendererBackend,
    adapters: std::result::Result<Vec<gfx_core::AdapterInfo>, gfx_core::GfxError>,
) -> Result<()> {
    let adapters = adapters?;
    if adapters.is_empty() {
        bail!("{backend} enumerated no hardware adapters");
    }
    Ok(())
}

impl Platform for WindowsPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(
            WindowsKeyboardLayout::new()
                .log_err()
                .unwrap_or(WindowsKeyboardLayout::unknown()),
        )
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(WindowsKeyboardMapper::new())
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner
            .state
            .borrow_mut()
            .callbacks
            .keyboard_layout_change = Some(callback);
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        let event_loop = EventLoop::<WindowsUserEvent>::with_user_event()
            .build()
            .expect("event loop");
        {
            let mut lock = self.event_loop_proxy.lock().unwrap();
            *lock = Some(event_loop.create_proxy());
        }
        let inner = self.inner.clone();
        let event_loop_proxy = self.event_loop_proxy.clone();
        let mut application = WindowsApplication {
            inner,
            on_finish_launching: Some(on_finish_launching),
            event_loop_proxy,
            windows: FxHashMap::default(),
            focused_window_id: None,
            current_modifiers: Modifiers::default(),
            pressed_button: None,
            hovered_window_id: None,
            pending_file_drops: FxHashMap::default(),
        };
        let _ = event_loop.run_app(&mut application);
    }

    fn quit(&self) {
        if let Some(proxy) = self.event_loop_proxy.lock().unwrap().clone() {
            let _ = proxy.send_event(WindowsUserEvent::Quit);
        }
    }

    fn restart(&self, binary_path: Option<PathBuf>) {
        let pid = std::process::id();
        let Some(app_path) = binary_path.or(self.app_path().log_err()) else {
            return;
        };
        let script = format!(
            r#"
            $pidToWaitFor = {}
            $exePath = "{}"

            while ($true) {{
                $process = Get-Process -Id $pidToWaitFor -ErrorAction SilentlyContinue
                if (-not $process) {{
                    Start-Process -FilePath $exePath
                    break
                }}
                Start-Sleep -Seconds 0.1
            }}
            "#,
            pid,
            app_path.display(),
        );

        #[allow(
            clippy::disallowed_methods,
            reason = "We are restarting ourselves, using std command thus is fine"
        )]
        let restart_process = util::command::new_std_command("powershell.exe")
            .arg("-command")
            .arg(script)
            .spawn();

        match restart_process {
            Ok(_) => self.quit(),
            Err(e) => log::error!("failed to spawn restart script: {:?}", e),
        }
    }

    fn activate(&self, _ignoring_other_apps: bool) {
        let _ = with_active_context(|_event_loop, app| app.activate_window());
    }

    fn hide(&self) {}

    // todo(windows)
    fn hide_other_apps(&self) {
        unimplemented!()
    }

    // todo(windows)
    fn unhide_other_apps(&self) {
        unimplemented!()
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        if self.inner.state.borrow().displays.is_empty() {
            let _ = with_active_context(|event_loop, app| app.refresh_display_cache(event_loop));
        }

        self.inner
            .state
            .borrow()
            .displays
            .iter()
            .cloned()
            .map(|display| Rc::new(display) as Rc<dyn PlatformDisplay>)
            .collect()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        if self.inner.state.borrow().displays.is_empty() {
            let _ = with_active_context(|event_loop, app| app.refresh_display_cache(event_loop));
        }

        let state = self.inner.state.borrow();
        let primary_id = state.primary_display_id?;
        state
            .displays
            .iter()
            .find(|display| display.id() == primary_id)
            .cloned()
            .map(|display| Rc::new(display) as Rc<dyn PlatformDisplay>)
    }

    #[cfg(feature = "screen-capture")]
    fn is_screen_capture_supported(&self) -> bool {
        true
    }

    #[cfg(feature = "screen-capture")]
    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        crate::platform::scap_screen_capture::scap_screen_sources(&self.foreground_executor)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.inner.state.borrow().active_window_handle
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let creation_info = self.generate_creation_info();
        let cursor_style = self.inner.state.borrow().cursor_style;
        let window = with_active_context(|event_loop, app| {
            let window = WindowsWindow::new(event_loop, handle, options, creation_info)?;
            let window_id = window.window_id();
            apply_cursor_style_to_window(window.window(), cursor_style);
            app.windows.insert(window_id, window.clone());
            window.window().request_redraw();
            Ok::<_, anyhow::Error>(window)
        })
        .context("winit event loop is not active")??;

        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        system_appearance().log_err().unwrap_or_default()
    }

    fn open_url(&self, url: &str) {
        if url.is_empty() {
            return;
        }
        let url_string = url.to_string();
        self.background_executor()
            .spawn(async move {
                open_target(&url_string)
                    .with_context(|| format!("Opening url: {}", url_string))
                    .log_err();
            })
            .detach();
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.inner.state.borrow_mut().callbacks.open_urls = Some(callback);
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        let window = with_active_context(|_event_loop, app| app.focused_window_hwnd()).flatten();
        self.foreground_executor()
            .spawn(async move {
                let _ = tx.send(file_open_dialog(options, window));
            })
            .detach();

        rx
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> Receiver<Result<Option<PathBuf>>> {
        let directory = directory.to_owned();
        let suggested_name = suggested_name.map(|s| s.to_owned());
        let (tx, rx) = oneshot::channel();
        let window = with_active_context(|_event_loop, app| app.focused_window_hwnd()).flatten();
        self.foreground_executor()
            .spawn(async move {
                let _ = tx.send(file_save_dialog(directory, suggested_name, window));
            })
            .detach();

        rx
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        // The FOS_PICKFOLDERS flag toggles between "only files" and "only folders".
        false
    }

    fn reveal_path(&self, path: &Path) {
        if path.as_os_str().is_empty() {
            return;
        }
        let path = path.to_path_buf();
        self.background_executor()
            .spawn(async move {
                open_target_in_explorer(&path)
                    .with_context(|| format!("Revealing path {} in explorer", path.display()))
                    .log_err();
            })
            .detach();
    }

    fn open_with_system(&self, path: &Path) {
        if path.as_os_str().is_empty() {
            return;
        }
        let path = path.to_path_buf();
        self.background_executor()
            .spawn(async move {
                open_target(&path)
                    .with_context(|| format!("Opening {} with system", path.display()))
                    .log_err();
            })
            .detach();
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.inner.state.borrow_mut().callbacks.quit = Some(callback);
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.inner.state.borrow_mut().callbacks.reopen = Some(callback);
    }

    fn set_menus(&self, menus: Vec<Menu>, _keymap: &Keymap) {
        self.inner.state.borrow_mut().menus = menus.into_iter().map(|menu| menu.owned()).collect();
    }

    fn menus(&self) -> Option<Vec<OwnedMenu>> {
        Some(self.inner.state.borrow().menus.clone())
    }

    fn set_dock_menu(&self, menus: Vec<MenuItem>, _keymap: &Keymap) {
        self.set_dock_menus(menus);
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.inner.state.borrow_mut().callbacks.app_menu_action = Some(callback);
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.inner.state.borrow_mut().callbacks.will_open_app_menu = Some(callback);
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.inner
            .state
            .borrow_mut()
            .callbacks
            .validate_app_menu_command = Some(callback);
    }

    fn app_path(&self) -> Result<PathBuf> {
        Ok(std::env::current_exe()?)
    }

    // todo(windows)
    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        anyhow::bail!("not yet implemented");
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        let mut lock = self.inner.state.borrow_mut();
        if lock.cursor_style == style {
            return;
        }
        lock.cursor_style = style;
        drop(lock);

        let _ = with_active_context(|_event_loop, app| {
            for window in app.windows.values() {
                apply_cursor_style_to_window(window.window(), style);
            }
        });
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        should_auto_hide_scrollbars().log_err().unwrap_or(false)
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Err(error) = write_to_clipboard(item) {
            log::error!("Failed to write clipboard: {error:#}");
        }
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        read_from_clipboard()
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        let mut password = password.to_vec();
        let mut username = username.encode_utf16().chain(Some(0)).collect_vec();
        let mut target_name = windows_credentials_target_name(url)
            .encode_utf16()
            .chain(Some(0))
            .collect_vec();
        self.foreground_executor().spawn(async move {
            let credentials = CREDENTIALW {
                LastWritten: unsafe { GetSystemTimeAsFileTime() },
                Flags: CRED_FLAGS(0),
                Type: CRED_TYPE_GENERIC,
                TargetName: PWSTR::from_raw(target_name.as_mut_ptr()),
                CredentialBlobSize: password.len() as u32,
                CredentialBlob: password.as_ptr() as *mut _,
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                UserName: PWSTR::from_raw(username.as_mut_ptr()),
                ..CREDENTIALW::default()
            };
            unsafe { CredWriteW(&credentials, 0) }?;
            Ok(())
        })
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        let mut target_name = windows_credentials_target_name(url)
            .encode_utf16()
            .chain(Some(0))
            .collect_vec();
        self.foreground_executor().spawn(async move {
            let mut credentials: *mut CREDENTIALW = std::ptr::null_mut();
            unsafe {
                CredReadW(
                    PCWSTR::from_raw(target_name.as_ptr()),
                    CRED_TYPE_GENERIC,
                    None,
                    &mut credentials,
                )?
            };

            if credentials.is_null() {
                Ok(None)
            } else {
                let username: String = unsafe { (*credentials).UserName.to_string()? };
                let credential_blob = unsafe {
                    std::slice::from_raw_parts(
                        (*credentials).CredentialBlob,
                        (*credentials).CredentialBlobSize as usize,
                    )
                };
                let password = credential_blob.to_vec();
                unsafe { CredFree(credentials as *const _ as _) };
                Ok(Some((username, password)))
            }
        })
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        let mut target_name = windows_credentials_target_name(url)
            .encode_utf16()
            .chain(Some(0))
            .collect_vec();
        self.foreground_executor().spawn(async move {
            unsafe {
                CredDeleteW(
                    PCWSTR::from_raw(target_name.as_ptr()),
                    CRED_TYPE_GENERIC,
                    None,
                )?
            };
            Ok(())
        })
    }

    fn register_url_scheme(&self, _: &str) -> Task<anyhow::Result<()>> {
        Task::ready(Err(anyhow!("register_url_scheme unimplemented")))
    }

    fn perform_dock_menu_action(&self, action: usize) {
        if let Some(proxy) = self.event_loop_proxy.lock().unwrap().clone() {
            proxy
                .send_event(WindowsUserEvent::DockMenuAction(action))
                .log_err();
        }
    }

    fn update_jump_list(
        &self,
        menus: Vec<MenuItem>,
        entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Vec<SmallVec<[PathBuf; 2]>> {
        self.update_jump_list(menus, entries)
    }
}

impl WindowsPlatformInner {
    #[inline]
    fn run_foreground_tasks(&self) -> bool {
        let has_pending_tasks = drain_foreground_tasks(
            || self.main_receiver.try_recv().ok(),
            || !self.main_receiver.is_empty(),
        );
        if has_pending_tasks {
            return true;
        }

        // Keep the wakeup coalesced while polling tasks. A runnable may schedule itself again;
        // clearing this flag before the drain would post another winit user event for every
        // batch and can prevent the Windows message queue from reaching input and redraw events.
        self.main_thread_wakeup_pending
            .store(false, Ordering::Release);
        if self.main_receiver.is_empty() {
            false
        } else {
            self.main_thread_wakeup_pending
                .store(true, Ordering::Release);
            true
        }
    }

    pub(crate) fn handle_dock_action_event(&self, action_idx: usize) -> Option<isize> {
        let mut lock = self.state.borrow_mut();
        let mut callback = lock.callbacks.app_menu_action.take()?;
        let Some(action) = lock
            .jump_list
            .dock_menus
            .get(action_idx)
            .map(|dock_menu| dock_menu.action.boxed_clone())
        else {
            lock.callbacks.app_menu_action = Some(callback);
            log::error!("Dock menu for index {action_idx} not found");
            return Some(1);
        };
        drop(lock);
        callback(&*action);
        self.state.borrow_mut().callbacks.app_menu_action = Some(callback);
        Some(0)
    }
}

struct WindowsApplication {
    inner: Rc<WindowsPlatformInner>,
    on_finish_launching: Option<Box<dyn FnOnce()>>,
    event_loop_proxy: Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
    windows: FxHashMap<winit::window::WindowId, WindowsWindow>,
    focused_window_id: Option<winit::window::WindowId>,
    current_modifiers: Modifiers,
    pressed_button: Option<MouseButton>,
    hovered_window_id: Option<winit::window::WindowId>,
    pending_file_drops: FxHashMap<winit::window::WindowId, PendingFileDrop>,
}

impl WindowsApplication {
    fn run_foreground_tasks(&self, event_loop: &ActiveEventLoop) {
        let control_flow = if self.inner.run_foreground_tasks() {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        };
        event_loop.set_control_flow(control_flow);
    }

    fn dispatch_pending_frame_requests(&self) {
        let windows: Vec<_> = self.windows.values().cloned().collect();
        for window in windows {
            let options = window.take_pending_frame_request();
            if options.requires_frame() {
                window.invoke_request_frame(options);
            }
        }
    }

    fn sync_window_size(
        window: &WindowsWindow,
        physical_size: winit::dpi::PhysicalSize<u32>,
        scale_factor: f32,
    ) -> Option<(Size<Pixels>, f32)> {
        if physical_size.width == 0 || physical_size.height == 0 {
            return None;
        }

        let logical_size = Size {
            width: Pixels(physical_size.width as f32 / scale_factor),
            height: Pixels(physical_size.height as f32 / scale_factor),
        };
        // Keep resize callbacks responsive; the renderer applies the latest size
        // from the frame path where repeated Windows resize events are coalesced.
        window.queue_renderer_resize(Size {
            width: DevicePixels(physical_size.width as i32),
            height: DevicePixels(physical_size.height as i32),
        });
        if let Ok(state) = window.try_borrow_state() {
            state.logical_size.set(logical_size);
            state.scale_factor.set(scale_factor);
        } else {
            log::warn!("window state is already borrowed while synchronizing Windows size");
        }

        Some((logical_size, scale_factor))
    }

    fn refresh_display_cache(&mut self, event_loop: &ActiveEventLoop) {
        let displays: Vec<WindowsDisplay> = event_loop
            .available_monitors()
            .enumerate()
            .map(|(index, monitor)| {
                WindowsDisplay::from_monitor_handle(DisplayId(index as u32), &monitor)
            })
            .collect();
        let primary_display_id = event_loop.primary_monitor().and_then(|primary_monitor| {
            displays
                .iter()
                .find(|display| display.matches_monitor(&primary_monitor))
                .map(PlatformDisplay::id)
        });

        let mut state = self.inner.state.borrow_mut();
        state.displays = displays;
        state.primary_display_id = primary_display_id;
    }

    fn sync_active_window_handle(&mut self) {
        let active_window_handle = self
            .focused_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .map(|window| window.0.handle);
        self.inner.state.borrow_mut().active_window_handle = active_window_handle;
    }

    fn activate_window(&mut self) {
        let window = self
            .focused_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .or_else(|| self.windows.values().next())
            .cloned();

        if let Some(window) = window {
            window.activate();
        }
    }

    fn focused_window_hwnd(&self) -> Option<HWND> {
        self.focused_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .and_then(WindowsWindow::native_hwnd)
    }
}

impl ApplicationHandler<WindowsUserEvent> for WindowsApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = Some((event_loop as *const _, self as *mut _));
        });
        self.refresh_display_cache(event_loop);
        if let Some(on_finish_launching) = self.on_finish_launching.take() {
            on_finish_launching();
        }
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: WindowsUserEvent) {
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = Some((event_loop as *const _, self as *mut _));
        });
        match event {
            // Drain in `about_to_wait`, after winit has dispatched the current batch of native
            // window messages. This keeps mouse, non-client drag, and redraw events responsive.
            WindowsUserEvent::RunMainThreadTasks => {}
            WindowsUserEvent::DockMenuAction(action_index) => {
                self.inner.handle_dock_action_event(action_index);
            }
            WindowsUserEvent::Quit => event_loop.exit(),
        }
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = Some((event_loop as *const _, self as *mut _));
        });
        self.run_foreground_tasks(event_loop);
        // `RedrawRequested` can be suppressed while a native window is being mapped or when
        // Windows coalesces redraws. Consume any request that survived the event cycle so the
        // frame watchdog does not become the normal delivery path.
        self.dispatch_pending_frame_requests();
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = Some((event_loop as *const _, self as *mut _));
        });
        let Some(window) = self.windows.get(&window_id) else {
            ACTIVE_CONTEXT.with(|storage| {
                *storage.borrow_mut() = None;
            });
            return;
        };
        let window = window.clone();

        match event {
            winit::event::WindowEvent::Resized(physical_size) => {
                let scale_factor = window.scale_factor();
                if let Some((logical_size, scale_factor)) =
                    Self::sync_window_size(&window, physical_size, scale_factor)
                {
                    window.invoke_resize(logical_size, scale_factor);
                    window.request_frame(RequestFrameOptions::from_refresh());
                }
                self.refresh_display_cache(event_loop);
            }
            winit::event::WindowEvent::Moved(_) => {
                self.refresh_display_cache(event_loop);
                let callback = window.0.state.borrow_mut().callbacks.moved.take();
                if let Some(mut callback) = callback {
                    callback();
                    window.0.state.borrow_mut().callbacks.moved = Some(callback);
                }
            }
            winit::event::WindowEvent::Focused(active) => {
                if active {
                    self.focused_window_id = Some(window_id);
                    window.request_frame(RequestFrameOptions::from_refresh());
                } else if self.focused_window_id == Some(window_id) {
                    self.focused_window_id = None;
                }
                self.sync_active_window_handle();
                window.invoke_active_status_change(active);
            }
            winit::event::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let physical_size = window.window().inner_size();
                if let Some((logical_size, scale_factor)) =
                    Self::sync_window_size(&window, physical_size, scale_factor as f32)
                {
                    self.refresh_display_cache(event_loop);
                    window.invoke_resize(logical_size, scale_factor);
                    window.request_frame(RequestFrameOptions::from_refresh());
                } else {
                    self.refresh_display_cache(event_loop);
                }
            }
            winit::event::WindowEvent::ThemeChanged(_) => {
                let callback = window
                    .0
                    .state
                    .borrow_mut()
                    .callbacks
                    .appearance_changed
                    .take();
                if let Some(mut callback) = callback {
                    callback();
                    window.0.state.borrow_mut().callbacks.appearance_changed = Some(callback);
                }
            }
            winit::event::WindowEvent::CloseRequested => {
                let should_close = window.should_close().unwrap_or(true);
                if should_close {
                    window.invoke_close();
                    if self.hovered_window_id == Some(window_id) {
                        self.hovered_window_id = None;
                    }
                    if self.focused_window_id == Some(window_id) {
                        self.focused_window_id = None;
                    }
                    self.windows.remove(&window_id);
                    self.sync_active_window_handle();
                    if self.windows.is_empty() {
                        event_loop.exit();
                    }
                }
            }
            winit::event::WindowEvent::RedrawRequested => {
                let options = window.take_pending_frame_request();
                if options.requires_frame() {
                    window.invoke_request_frame(options);
                }
            }
            winit::event::WindowEvent::CursorEntered { .. } => {
                self.hovered_window_id = Some(window_id);
                let mut state = window.0.state.borrow_mut();
                if !state.hovered.get() {
                    state.hovered.set(true);
                    let callback = state.callbacks.hovered_status_change.take();
                    drop(state);
                    if let Some(mut callback) = callback {
                        callback(true);
                        window.0.state.borrow_mut().callbacks.hovered_status_change =
                            Some(callback);
                    }
                }
            }
            winit::event::WindowEvent::CursorMoved { position, .. } => {
                self.hovered_window_id = Some(window_id);
                let scale_factor = window.scale_factor();
                let position = point(
                    Pixels(position.x as f32 / scale_factor),
                    Pixels(position.y as f32 / scale_factor),
                );
                let mut state = window.0.state.borrow_mut();
                state.mouse_position.set(position);
                let hovered_callback = if !state.hovered.get() {
                    state.hovered.set(true);
                    state.callbacks.hovered_status_change.take()
                } else {
                    None
                };
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = hovered_callback {
                    callback(true);
                    window.0.state.borrow_mut().callbacks.hovered_status_change = Some(callback);
                }
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: self.pressed_button,
                        modifiers: self.current_modifiers,
                    }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::CursorLeft { .. } => {
                if self.hovered_window_id == Some(window_id) {
                    self.hovered_window_id = None;
                }
                let mut state = window.0.state.borrow_mut();
                state.hovered.set(false);
                let position = state.mouse_position.get();
                let pressed_button = self.pressed_button;
                let modifiers = self.current_modifiers;
                let hovered_callback = state.callbacks.hovered_status_change.take();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = hovered_callback {
                    callback(false);
                    window.0.state.borrow_mut().callbacks.hovered_status_change = Some(callback);
                }
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::MouseExited(MouseExitEvent {
                        position,
                        pressed_button,
                        modifiers,
                    }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::HoveredFile(path) => {
                let position = window.0.state.borrow().mouse_position.get();
                let entry = self.pending_file_drops.entry(window_id).or_default();
                entry.paths.push(path);
                let paths = ExternalPaths(entry.paths.clone());
                let mut state = window.0.state.borrow_mut();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::FileDrop(FileDropEvent::Entered {
                        position,
                        paths,
                    }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::DroppedFile(path) => {
                let position = window.0.state.borrow().mouse_position.get();
                let entry = self.pending_file_drops.entry(window_id).or_default();
                if !entry.paths.iter().any(|existing| existing == &path) {
                    entry.paths.push(path.clone());
                }
                let paths = ExternalPaths(entry.paths.clone());
                self.pending_file_drops.remove(&window_id);
                let mut state = window.0.state.borrow_mut();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::FileDrop(FileDropEvent::Entered {
                        position,
                        paths,
                    }));
                    let _ = callback(PlatformInput::FileDrop(FileDropEvent::Submit { position }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::HoveredFileCancelled => {
                self.pending_file_drops.remove(&window_id);
                let mut state = window.0.state.borrow_mut();
                let position = state.mouse_position.get();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::FileDrop(FileDropEvent::Exited));
                    let _ = callback(PlatformInput::FileDrop(FileDropEvent::Pending { position }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = mouse_button_from_winit(button) {
                    let mut window_state = window.0.state.borrow_mut();
                    let position = window_state.mouse_position.get();
                    let modifiers = self.current_modifiers;
                    let scale_factor = window_state.scale_factor.get();
                    let input_callback = window_state.callbacks.input.take();
                    match state {
                        winit::event::ElementState::Pressed => {
                            self.pressed_button = Some(button);
                            let click_count = window_state.click_state.borrow_mut().update(
                                button,
                                point(
                                    DevicePixels((position.x.0 * scale_factor) as i32),
                                    DevicePixels((position.y.0 * scale_factor) as i32),
                                ),
                            );
                            drop(window_state);
                            if let Some(mut callback) = input_callback {
                                let _ = callback(PlatformInput::MouseDown(MouseDownEvent {
                                    button,
                                    position,
                                    modifiers,
                                    click_count,
                                    first_mouse: false,
                                }));
                                window.0.state.borrow_mut().callbacks.input = Some(callback);
                            }
                        }
                        winit::event::ElementState::Released => {
                            self.pressed_button = None;
                            let click_count = window_state.click_state.borrow().current_count;
                            drop(window_state);
                            if let Some(mut callback) = input_callback {
                                let _ = callback(PlatformInput::MouseUp(MouseUpEvent {
                                    button,
                                    position,
                                    modifiers,
                                    click_count,
                                }));
                                window.0.state.borrow_mut().callbacks.input = Some(callback);
                            }
                        }
                    }
                }
            }
            winit::event::WindowEvent::MouseWheel { delta, phase, .. } => {
                let mut state = window.0.state.borrow_mut();
                let position = state.mouse_position.get();
                let delta = match delta {
                    winit::event::MouseScrollDelta::LineDelta(x, y) => {
                        ScrollDelta::Lines(point(x, y))
                    }
                    winit::event::MouseScrollDelta::PixelDelta(pixel) => {
                        let scale_factor = window.scale_factor();
                        ScrollDelta::Pixels(point(
                            Pixels(pixel.x as f32 / scale_factor),
                            Pixels(pixel.y as f32 / scale_factor),
                        ))
                    }
                };
                let touch_phase = match phase {
                    winit::event::TouchPhase::Started => TouchPhase::Started,
                    winit::event::TouchPhase::Moved => TouchPhase::Moved,
                    winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                        TouchPhase::Ended
                    }
                };
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position,
                        delta,
                        modifiers: self.current_modifiers,
                        touch_phase,
                    }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::ModifiersChanged(new_modifiers) => {
                let modifiers = modifiers_from_winit(new_modifiers.state());
                self.current_modifiers = modifiers;
                let mut state = window.0.state.borrow_mut();
                state.modifiers.set(modifiers);
                let capslock = state.capslock.get();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                        modifiers,
                        capslock,
                    }));
                    window.0.state.borrow_mut().callbacks.input = Some(callback);
                }
            }
            winit::event::WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        logical_key,
                        state,
                        text,
                        repeat,
                        ..
                    },
                ..
            } => {
                if let Some(keystroke) =
                    keystroke_from_winit(&logical_key, self.current_modifiers, &text)
                {
                    let mut state_ref = window.0.state.borrow_mut();
                    let input_callback = state_ref.callbacks.input.take();
                    drop(state_ref);
                    if let Some(mut callback) = input_callback {
                        let input = match state {
                            winit::event::ElementState::Pressed => {
                                PlatformInput::KeyDown(KeyDownEvent {
                                    keystroke,
                                    is_held: repeat,
                                })
                            }
                            winit::event::ElementState::Released => {
                                PlatformInput::KeyUp(KeyUpEvent { keystroke })
                            }
                        };
                        let _ = callback(input);
                        window.0.state.borrow_mut().callbacks.input = Some(callback);
                    }
                }
            }
            _ => {}
        }
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(ref mut callback) = self.inner.state.borrow_mut().callbacks.quit {
            callback();
        }
        *self.event_loop_proxy.lock().unwrap() = None;
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }
}

impl Drop for WindowsPlatform {
    fn drop(&mut self) {
        if self.ole_initialized {
            unsafe {
                OleUninitialize();
            }
        }
    }
}

impl Drop for WindowsPlatformState {
    fn drop(&mut self) {}
}

pub(crate) struct WindowCreationInfo {
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) executor: ForegroundExecutor,
    pub(crate) disable_direct_composition: bool,
    pub(crate) renderer_backend: RendererBackend,
    pub(crate) renderer_options: RendererOptions,
}

fn open_target(target: impl AsRef<OsStr>) -> Result<()> {
    let target = target.as_ref();
    let ret = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            &HSTRING::from(target),
            None,
            None,
            SW_SHOWDEFAULT,
        )
    };
    if ret.0 as isize <= 32 {
        Err(anyhow::anyhow!(
            "Unable to open target: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn open_target_in_explorer(target: &Path) -> Result<()> {
    let dir = target.parent().context("No parent folder found")?;
    let desktop = unsafe { SHGetDesktopFolder()? };

    let mut dir_item = std::ptr::null_mut();
    unsafe {
        desktop.ParseDisplayName(
            HWND::default(),
            None,
            &HSTRING::from(dir),
            None,
            &mut dir_item,
            std::ptr::null_mut(),
        )?;
    }

    let mut file_item = std::ptr::null_mut();
    unsafe {
        desktop.ParseDisplayName(
            HWND::default(),
            None,
            &HSTRING::from(target),
            None,
            &mut file_item,
            std::ptr::null_mut(),
        )?;
    }

    let highlight = [file_item as *const _];
    unsafe { SHOpenFolderAndSelectItems(dir_item as _, Some(&highlight), 0) }.or_else(|err| {
        if err.code().0 == ERROR_FILE_NOT_FOUND.0 as i32 {
            // On some systems, the above call mysteriously fails with "file not
            // found" even though the file is there.  In these cases, ShellExecute()
            // seems to work as a fallback (although it won't select the file).
            open_target(dir).context("Opening target parent folder")
        } else {
            Err(anyhow::anyhow!("Can not open target path: {}", err))
        }
    })
}

fn file_open_dialog(
    options: PathPromptOptions,
    window: Option<HWND>,
) -> Result<Option<Vec<PathBuf>>> {
    let folder_dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)? };

    let mut dialog_options = FOS_FILEMUSTEXIST;
    if options.multiple {
        dialog_options |= FOS_ALLOWMULTISELECT;
    }
    if options.directories {
        dialog_options |= FOS_PICKFOLDERS;
    }

    unsafe {
        folder_dialog.SetOptions(dialog_options)?;

        if let Some(prompt) = options.prompt {
            let prompt: &str = &prompt;
            folder_dialog.SetOkButtonLabel(&HSTRING::from(prompt))?;
        }

        if folder_dialog.Show(window).is_err() {
            // User cancelled
            return Ok(None);
        }
    }

    let results = unsafe { folder_dialog.GetResults()? };
    let file_count = unsafe { results.GetCount()? };
    if file_count == 0 {
        return Ok(None);
    }

    let mut paths = Vec::with_capacity(file_count as usize);
    for i in 0..file_count {
        let item = unsafe { results.GetItemAt(i)? };
        let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)?.to_string()? };
        paths.push(PathBuf::from(path));
    }

    Ok(Some(paths))
}

fn file_save_dialog(
    directory: PathBuf,
    suggested_name: Option<String>,
    window: Option<HWND>,
) -> Result<Option<PathBuf>> {
    let dialog: IFileSaveDialog = unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL)? };
    if !directory.to_string_lossy().is_empty()
        && let Some(full_path) = directory
            .canonicalize()
            .context("failed to canonicalize directory")
            .log_err()
    {
        let full_path = SanitizedPath::new(&full_path);
        let full_path_string = full_path.to_string();
        let path_item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&HSTRING::from(full_path_string), None)? };
        unsafe {
            dialog
                .SetFolder(&path_item)
                .context("failed to set dialog folder")
                .log_err()
        };
    }

    if let Some(suggested_name) = suggested_name {
        unsafe {
            dialog
                .SetFileName(&HSTRING::from(suggested_name))
                .context("failed to set file name")
                .log_err()
        };
    }

    unsafe {
        dialog.SetFileTypes(&[Common::COMDLG_FILTERSPEC {
            pszName: windows::core::w!("All files"),
            pszSpec: windows::core::w!("*.*"),
        }])?;
        if dialog.Show(window).is_err() {
            // User cancelled
            return Ok(None);
        }
    }
    let shell_item = unsafe { dialog.GetResult()? };
    let file_path_string = unsafe {
        let pwstr = shell_item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let string = pwstr.to_string()?;
        CoTaskMemFree(Some(pwstr.0 as _));
        string
    };
    Ok(Some(PathBuf::from(file_path_string)))
}

#[inline]
fn should_auto_hide_scrollbars() -> Result<bool> {
    let ui_settings = UISettings::new()?;
    Ok(ui_settings.AutoHideScrollBars()?)
}

#[cfg(test)]
mod tests {
    use super::WINDOWS_AUTO_RENDERER_BACKEND_ORDER;
    use crate::{ClipboardItem, RendererBackend, read_from_clipboard, write_to_clipboard};

    #[test]
    fn test_clipboard() {
        let item = ClipboardItem::new_string("你好，我是张小白".to_string());
        write_to_clipboard(item.clone()).expect("writes CJK clipboard text");
        assert_eq!(read_from_clipboard(), Some(item));

        let item = ClipboardItem::new_string("12345".to_string());
        write_to_clipboard(item.clone()).expect("writes ASCII clipboard text");
        assert_eq!(read_from_clipboard(), Some(item));

        let item = ClipboardItem::new_string_with_json_metadata("abcdef".to_string(), vec![3, 4]);
        write_to_clipboard(item.clone()).expect("writes clipboard metadata");
        assert_eq!(read_from_clipboard(), Some(item));
    }

    #[test]
    fn windows_renderer_backends_remain_nova_only() {
        assert_eq!(
            "nova-vulkan".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaVulkan
        );
        assert_eq!(
            "nova-dx12".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaDx12
        );
    }

    #[test]
    fn windows_auto_renderer_prefers_dx12_before_vulkan() {
        #[cfg(not(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan")))]
        assert_eq!(
            WINDOWS_AUTO_RENDERER_BACKEND_ORDER,
            &[RendererBackend::NovaDx12]
        );

        #[cfg(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan"))]
        assert_eq!(
            WINDOWS_AUTO_RENDERER_BACKEND_ORDER,
            &[RendererBackend::NovaDx12, RendererBackend::NovaVulkan]
        );
    }

    #[test]
    fn windows_auto_renderer_skips_unavailable_backend() {
        let resolved = super::resolve_auto_renderer_backend(
            &[RendererBackend::NovaDx12, RendererBackend::NovaVulkan],
            |backend| match backend {
                RendererBackend::NovaDx12 => Err(anyhow::anyhow!("DX12 driver unavailable")),
                RendererBackend::NovaVulkan => Ok(()),
                RendererBackend::Auto
                | RendererBackend::NovaMetal
                | RendererBackend::HeadlessTest => Err(anyhow::anyhow!("unexpected backend")),
            },
        )
        .unwrap();

        assert_eq!(resolved, RendererBackend::NovaVulkan);
    }

    #[test]
    fn windows_auto_renderer_reports_all_unavailable_backends() {
        let error = super::resolve_auto_renderer_backend(
            &[RendererBackend::NovaDx12, RendererBackend::NovaVulkan],
            |backend| Err(anyhow::anyhow!("{backend} unavailable")),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("nova-dx12"));
        assert!(error.contains("nova-vulkan"));
    }

    #[test]
    fn windows_auto_renderer_reports_empty_backend_list() {
        let error = super::resolve_auto_renderer_backend(&[], |_| Ok(()))
            .unwrap_err()
            .to_string();

        assert!(error.contains("no compiled GPU backends"));
    }

    #[test]
    fn windows_headless_platform_skips_gpu_initialization() {
        let platform = super::WindowsPlatform::new_headless();

        assert_eq!(platform.renderer_backend, RendererBackend::HeadlessTest);
        assert!(!platform.ole_initialized);
        assert!(platform.disable_direct_composition);
    }
}

#![expect(
    unsafe_code,
    reason = "the Windows platform boundary calls Win32 and COM APIs"
)]

use std::{
    cell::RefCell,
    ffi::OsStr,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use ::util::{ResultExt, paths::SanitizedPath};
use anyhow::{Context as _, Result, anyhow, bail};
use async_task::Runnable;
use collections::FxHashMap;
use futures::channel::oneshot::Receiver;
use itertools::Itertools;
use smallvec::SmallVec;
use windows::{
    UI::ViewManagement::UISettings,
    Win32::{
        Foundation::*,
        Graphics::Gdi::ScreenToClient,
        Security::Credentials::*,
        System::{
            Com::*,
            Ole::*,
            Power::{
                PowerClearRequest, PowerCreateRequest, PowerRequestSystemRequired, PowerSetRequest,
            },
            ProcessStatus::K32EmptyWorkingSet,
            SystemInformation::*,
            SystemServices::POWER_REQUEST_CONTEXT_VERSION,
            Threading::{
                GetCurrentProcess, POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT,
                REASON_CONTEXT_0,
            },
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
    mouse_button_from_winit, spawn_sta_dialog,
};
use crate::window::Decorations;
use crate::*;

mod proxy;
pub(crate) use proxy::QueuedScene;
use proxy::{SceneMailbox, WindowProxy};

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
    static UI_OWNER_BRIDGE: RefCell<Option<WindowsUiBridge>> = const { RefCell::new(None) };
    static UI_OWNER_TASKS: RefCell<Option<flume::Receiver<Runnable>>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct WindowsUiBridge {
    event_loop: EventLoopProxy<WindowsUserEvent>,
    native_owner_closing: Arc<AtomicBool>,
    displays: Vec<WindowsDisplay>,
    primary_display_id: Option<DisplayId>,
    renderer_backend: RendererBackend,
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

#[derive(Debug)]
pub(crate) enum WindowsUserEvent {
    RunMainThreadTasks,
    VSync(super::vsync::VSyncEventTiming),
    RenderOwnerFrame {
        window_id: winit::window::WindowId,
        frame: crate::platform::render_owner::RenderOwnerFrame,
    },
    DockMenuAction(usize),
    NativeCommand(WindowsNativeCommand),
    ActivateApp,
    SetCursorStyle(CursorStyle),
    Quit,
}

pub(crate) enum WindowsNativeCommand {
    CreateWindow {
        handle: AnyWindowHandle,
        options: WindowParams,
        reply: std::sync::mpsc::Sender<Result<WindowsNativeWindow>>,
    },
    CommitScene {
        window_id: winit::window::WindowId,
        packet: PresentationPacket,
        reply: std::sync::mpsc::Sender<PlatformFrameResult>,
    },
    CommitLatestScene {
        window_id: winit::window::WindowId,
        mailbox: Arc<parking_lot::Mutex<SceneMailbox>>,
    },
    SetFrameRequestSender {
        window_id: winit::window::WindowId,
        sender: PlatformFrameRequestSender,
    },
    SetAnimationCompletionSender {
        window_id: winit::window::WindowId,
        sender: SceneAnimationCompletionSender,
    },
    SetEventSender {
        window_id: winit::window::WindowId,
        sender: flume::Sender<WindowsNativeEvent>,
    },
    CloseWindow {
        window_id: winit::window::WindowId,
    },
    WindowAction {
        window_id: winit::window::WindowId,
        action: WindowsWindowAction,
    },
    WindowCall {
        window_id: winit::window::WindowId,
        call: Box<dyn FnOnce(Option<&mut WindowsWindow>) + Send>,
    },
}

pub(crate) enum WindowsWindowAction {
    RequestFrame(PlatformFrameRequest),
    FrameRequestTimedOut(PlatformFrameRequest),
    StartMove,
    StartResize(ResizeEdge),
    Resize(Size<Pixels>),
    SetTitle(String),
    SetBackgroundAppearance(WindowBackgroundAppearance),
    Activate,
    Show,
    Hide,
    Minimize,
    Maximize,
    Restore,
    Zoom,
    ToggleFullscreen,
}

pub(crate) enum WindowsNativeEvent {
    Input(PlatformInput),
    Active(bool),
    Visibility(WindowVisibility),
    Hovered(bool),
    Resized(Size<Pixels>, f32),
    Moved,
    AppearanceChanged,
    CloseRequested,
    Closed,
}

impl std::fmt::Debug for WindowsNativeCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CreateWindow { .. } => "CreateWindow",
            Self::CommitScene { .. } => "CommitScene",
            Self::CommitLatestScene { .. } => "CommitLatestScene",
            Self::SetFrameRequestSender { .. } => "SetFrameRequestSender",
            Self::SetAnimationCompletionSender { .. } => "SetAnimationCompletionSender",
            Self::SetEventSender { .. } => "SetEventSender",
            Self::CloseWindow { .. } => "CloseWindow",
            Self::WindowAction { .. } => "WindowAction",
            Self::WindowCall { .. } => "WindowCall",
        })
    }
}

pub(crate) struct WindowsNativeWindow {
    pub(crate) window_id: winit::window::WindowId,
    pub(crate) window: Arc<winit::window::Window>,
    pub(crate) atlas: Arc<dyn PlatformAtlas>,
    pub(crate) display: Option<WindowsDisplay>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) window_bounds: WindowBounds,
    pub(crate) content_size: Size<Pixels>,
    pub(crate) scale_factor: f32,
    pub(crate) appearance: WindowAppearance,
    pub(crate) background_appearance: WindowBackgroundAppearance,
    pub(crate) mouse_position: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    pub(crate) capslock: Capslock,
    pub(crate) active: bool,
    pub(crate) hovered: bool,
    pub(crate) visibility: WindowVisibility,
    pub(crate) maximized: bool,
    pub(crate) minimized: bool,
    pub(crate) fullscreen: bool,
    pub(crate) gpu_specs: Option<GpuSpecs>,
    pub(crate) decorations: Decorations,
    pub(crate) default_client_inset: Option<Pixels>,
}

#[allow(dead_code)]
fn assert_native_window_boundary_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<WindowsNativeCommand>();
    assert_send::<WindowsNativeWindow>();
    assert_send::<WindowsNativeEvent>();
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
    ui_bridge: Option<WindowsUiBridge>,
    vsync_scheduler: Arc<super::vsync::VSyncScheduler>,
    ole_initialized: bool,
}

pub(crate) struct WindowsPlatformInner {
    state: RefCell<WindowsPlatformState>,
    /// Stops window commands before shutdown starts draining queued UI callbacks.
    native_owner_closing: Arc<AtomicBool>,
    // The below members will never change throughout the entire lifecycle of the app.
    main_receiver: flume::Receiver<Runnable>,
    main_thread_wakeup_pending: Arc<AtomicBool>,
}

struct PowerRequest {
    handle: HANDLE,
}

unsafe impl Send for PowerRequest {}

impl PowerRequest {
    fn prevent_idle_sleep(reason: &str) -> Result<Self> {
        let mut reason = reason.encode_utf16().chain([0]).collect::<Vec<_>>();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(reason.as_mut_ptr()),
            },
        };
        let handle = unsafe { PowerCreateRequest(&context) }
            .context("failed to create a Windows power request")?;
        if let Err(error) = unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) } {
            unsafe { CloseHandle(handle) }
                .context("failed to close the Windows power request")
                .log_err();
            return Err(error).context("failed to set the Windows power request");
        }
        Ok(Self { handle })
    }
}

impl Drop for PowerRequest {
    fn drop(&mut self) {
        unsafe { PowerClearRequest(self.handle, PowerRequestSystemRequired) }
            .context("failed to clear the Windows power request")
            .log_err();
        unsafe { CloseHandle(self.handle) }
            .context("failed to close the Windows power request")
            .log_err();
    }
}

#[derive(Default)]
struct PendingFileDrop {
    paths: SmallVec<[PathBuf; 2]>,
    submit_position: Option<Point<Pixels>>,
}

impl PendingFileDrop {
    fn push_hovered(&mut self, path: PathBuf) {
        if !self.paths.iter().any(|existing| existing == &path) {
            self.paths.push(path);
        }
    }

    fn mark_dropped(&mut self, path: PathBuf, position: Point<Pixels>) {
        self.push_hovered(path);
        self.submit_position = Some(position);
    }

    fn is_ready_to_submit(&self) -> bool {
        self.submit_position.is_some() && !self.paths.is_empty()
    }

    fn external_paths(&self) -> ExternalPaths {
        ExternalPaths(self.paths.clone())
    }
}

pub(crate) struct WindowsPlatformState {
    callbacks: PlatformCallbacks,
    menus: Vec<OwnedMenu>,
    jump_list: JumpList,
    cursor_style: CursorStyle,
    displays: Vec<WindowsDisplay>,
    primary_display_id: Option<DisplayId>,
    active_window_handle: Option<AnyWindowHandle>,
    system_suspended: bool,
}

#[derive(Default)]
struct PlatformCallbacks {
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,
    quit: Option<Box<dyn FnMut() -> bool>>,
    reopen: Option<Box<dyn FnMut()>>,
    app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,
    will_open_app_menu: Option<Box<dyn FnMut()>>,
    validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>,
    keyboard_layout_change: Option<Box<dyn FnMut()>>,
    system_sleep: Option<Box<dyn FnMut()>>,
    system_wake: Option<Box<dyn FnMut()>>,
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
            system_suspended: false,
            menus: Vec::new(),
        }
    }
}

fn create_windows_text_system(
    renderer_capabilities: RendererCapabilities,
) -> Result<Arc<dyn PlatformTextSystem>> {
    Ok(Arc::new(DirectWriteTextSystem::new(renderer_capabilities)?))
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
    pub(crate) fn run_separate(
        renderer_options: RendererOptions,
        ui_main: impl FnOnce(flume::Receiver<()>) + Send + 'static,
    ) -> Result<()> {
        let native = Rc::new(Self::new_native_owner(renderer_options)?);
        if native.renderer_backend != RendererBackend::HeadlessTest {
            // Device creation needs no HWND. Queue it on the same GPU owner that will create
            // the window renderer while winit and the UI owner prepare their own state.
            let options = RendererOptions {
                backend: native.renderer_backend,
                ..native.renderer_options.clone()
            };
            if let Err(error) = crate::platform::render_owner::prepare_device(options) {
                log::warn!("failed to queue GPUI device preparation: {error:#}");
            }
        }
        let (shutdown, shutdown_receiver) = flume::bounded(1);
        let ui_thread = Rc::new(RefCell::new(None));
        let ui_thread_slot = ui_thread.clone();
        let native_for_launch = native.clone();
        native.run(Box::new(move || {
            let Some(event_loop) = native_for_launch.event_loop_proxy.lock().unwrap().clone()
            else {
                log::error!("Windows native event loop proxy was not initialized");
                return;
            };
            let state = native_for_launch.inner.state.borrow();
            let bridge = WindowsUiBridge {
                event_loop,
                native_owner_closing: native_for_launch.inner.native_owner_closing.clone(),
                displays: state.displays.clone(),
                primary_display_id: state.primary_display_id,
                renderer_backend: native_for_launch.renderer_backend,
            };
            drop(state);
            let native_events = bridge.event_loop.clone();
            match std::thread::Builder::new()
                .name("gpui-ui".into())
                .spawn(move || {
                    UI_OWNER_BRIDGE.with(|slot| *slot.borrow_mut() = Some(bridge));
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        ui_main(shutdown_receiver);
                    }));
                    let _ = native_events.send_event(WindowsUserEvent::Quit);
                    if let Err(payload) = result {
                        std::panic::resume_unwind(payload);
                    }
                }) {
                Ok(handle) => *ui_thread_slot.borrow_mut() = Some(handle),
                Err(error) => {
                    log::error!("failed to start GPUI UI owner thread: {error}");
                    let _ = native_for_launch
                        .event_loop_proxy
                        .lock()
                        .unwrap()
                        .as_ref()
                        .map(|proxy| proxy.send_event(WindowsUserEvent::Quit));
                }
            }
        }));
        native
            .inner
            .native_owner_closing
            .store(true, Ordering::Release);
        if shutdown.send(()).is_err() {
            log::debug!("GPUI UI owner was already stopped when native event loop exited");
        }
        if let Some(handle) = ui_thread.borrow_mut().take() {
            handle
                .join()
                .map_err(|_| anyhow!("GPUI UI owner thread panicked"))?;
        }
        Ok(())
    }

    pub(crate) fn run_ui_owner_tasks(shutdown: flume::Receiver<()>) {
        let tasks = UI_OWNER_TASKS.with(|slot| slot.borrow_mut().take());
        let Some(tasks) = tasks else {
            log::error!("GPUI UI owner task queue was not initialized");
            return;
        };
        loop {
            enum Event {
                Task(Result<Runnable, flume::RecvError>),
                Shutdown,
            }
            match flume::Selector::new()
                .recv(&tasks, Event::Task)
                .recv(&shutdown, |_| Event::Shutdown)
                .wait()
            {
                Event::Task(Ok(runnable)) => {
                    runnable.run();
                }
                Event::Task(Err(_)) => break,
                Event::Shutdown => {
                    // Native close events are queued before the event loop exits. Let the UI
                    // owner consume those callbacks before dropping its App and windows.
                    while let Ok(runnable) = tasks.try_recv() {
                        runnable.run();
                    }
                    break;
                }
            }
        }
    }

    fn new_common_parts(
        ui_owner_mode: bool,
    ) -> (
        Rc<WindowsPlatformInner>,
        BackgroundExecutor,
        ForegroundExecutor,
        Arc<Mutex<Option<EventLoopProxy<WindowsUserEvent>>>>,
    ) {
        let (main_sender, main_receiver) = flume::unbounded::<Runnable>();
        let main_thread_wakeup_pending = Arc::new(AtomicBool::new(false));
        let event_loop_proxy = Arc::new(Mutex::new(None));
        let native_owner_closing = if ui_owner_mode {
            UI_OWNER_BRIDGE.with(|bridge| {
                bridge
                    .borrow()
                    .as_ref()
                    .map(|bridge| bridge.native_owner_closing.clone())
            })
        } else {
            None
        }
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let inner = Rc::new(WindowsPlatformInner {
            state: RefCell::new(WindowsPlatformState::new()),
            native_owner_closing,
            main_receiver,
            main_thread_wakeup_pending: main_thread_wakeup_pending.clone(),
        });
        let dispatcher = Arc::new(WindowsDispatcher::new(
            main_sender,
            main_thread_wakeup_pending,
            event_loop_proxy.clone(),
            ui_owner_mode,
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
            Self::new_common_parts(false);
        let renderer_backend = RendererBackend::HeadlessTest;
        let text_system = create_windows_text_system(renderer_backend.capabilities())
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
            ui_bridge: None,
            vsync_scheduler: Arc::new(super::vsync::VSyncScheduler::new()),
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

    fn new_native_owner(renderer_options: RendererOptions) -> Result<Self> {
        Self::new_for_role(renderer_options, true)
    }

    pub(crate) fn new(renderer_options: RendererOptions) -> Result<Self> {
        Self::new_for_role(renderer_options, false)
    }

    fn new_for_role(
        renderer_options: RendererOptions,
        native_owner_only: bool,
    ) -> Result<Self> {
        become_dpi_aware();
        unsafe {
            OleInitialize(None).context("unable to initialize Windows OLE")?;
        }

        let ui_bridge = UI_OWNER_BRIDGE.with(|bridge| bridge.borrow().clone());
        let requested_renderer_backend = renderer_options.backend;
        let renderer_backend = if let Some(bridge) = &ui_bridge {
            // The split native owner already resolved and validated the backend.
            // Reuse it to avoid a second DXGI/Vulkan adapter enumeration on the UI owner.
            bridge.renderer_backend
        } else {
            match requested_renderer_backend {
                RendererBackend::HeadlessTest => RendererBackend::HeadlessTest,
                RendererBackend::Auto
                | RendererBackend::NovaVulkan
                | RendererBackend::NovaDx12
                | RendererBackend::NovaMetal => Self::resolve_renderer_backend(&renderer_options)?,
            }
        };
        record_renderer_backend(renderer_backend);
        if ui_bridge.is_none()
            && matches!(
                requested_renderer_backend,
                RendererBackend::Auto
                    | RendererBackend::NovaVulkan
                    | RendererBackend::NovaDx12
                    | RendererBackend::NovaMetal
            )
        {
            log::info!(
                "GPUI Windows resolved renderer backend: {}",
                renderer_backend
            );
        }

        // The native owner hosts winit, DWM pacing and window creation only. The GPUI App and
        // all text shaping live on the separate UI owner, so constructing DirectWrite here would
        // create/register a second font loader and system font collection that is never used.
        let text_system: Arc<dyn PlatformTextSystem> = if native_owner_only {
            Arc::new(NoopTextSystem)
        } else {
            create_windows_text_system(renderer_backend.capabilities())?
        };
        let disable_direct_composition = std::env::var(DISABLE_DIRECT_COMPOSITION)
            .is_ok_and(|value| value == "true" || value == "1");
        let (inner, background_executor, foreground_executor, event_loop_proxy) =
            Self::new_common_parts(ui_bridge.is_some());
        if let Some(bridge) = &ui_bridge {
            let mut state = inner.state.borrow_mut();
            state.displays = bridge.displays.clone();
            state.primary_display_id = bridge.primary_display_id;
            UI_OWNER_TASKS.with(|tasks| {
                *tasks.borrow_mut() = Some(inner.main_receiver.clone());
            });
        }

        // EmptyWorkingSet is process-wide. In split-owner mode schedule it only from the UI
        // platform after the real App has been constructed, not once per platform object.
        if !native_owner_only && startup_working_set_trim_enabled() {
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
            ui_bridge,
            vsync_scheduler: Arc::new(super::vsync::VSyncScheduler::new()),
            ole_initialized: true,
        })
    }

    fn generate_creation_info(&self) -> WindowCreationInfo {
        let platform = Rc::downgrade(&self.inner);
        let end_session_platform = platform.clone();
        WindowCreationInfo {
            executor: self.foreground_executor.clone(),
            power_event: Rc::new(move |wparam| {
                if let Some(platform) = platform.upgrade() {
                    platform.handle_power_broadcast(wparam);
                }
            }),
            end_session_event: Rc::new(move || {
                end_session_platform
                    .upgrade()
                    .map_or(true, |platform| platform.handle_end_session())
            }),
            disable_direct_composition: self.disable_direct_composition,
            renderer_backend: self.renderer_backend,
            renderer_options: self.renderer_options.clone(),
            vsync_scheduler: self.vsync_scheduler.clone(),
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
    adapters: std::result::Result<Vec<gfx_core::AdapterInfo>, gfx_core::Error>,
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

    fn keyboard_mapper(&self) -> Arc<dyn PlatformKeyboardMapper> {
        Arc::new(WindowsKeyboardMapper::new())
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner
            .state
            .borrow_mut()
            .callbacks
            .keyboard_layout_change = Some(callback);
    }

    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        self.inner.state.borrow_mut().callbacks.system_sleep = Some(callback);
    }

    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        self.inner.state.borrow_mut().callbacks.system_wake = Some(callback);
    }

    fn prevent_idle_sleep(&self, reason: &str) -> Task<Result<ActivityGuard>> {
        Task::ready(
            PowerRequest::prevent_idle_sleep(reason)
                .map(|request| ActivityGuard::new(move || drop(request))),
        )
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
            creation_info: self.generate_creation_info(),
            windows: FxHashMap::default(),
            focused_window_id: None,
            current_modifiers: Modifiers::default(),
            pressed_button: None,
            hovered_window_id: None,
            pending_file_drops: FxHashMap::default(),
            vsync_scheduler: self.vsync_scheduler.clone(),
        };
        if let Err(error) = super::vsync::spawn_vsync_thread(
            event_loop.create_proxy(),
            self.vsync_scheduler.clone(),
        ) {
            log::error!("failed to start GPUI Windows DWM frame pacing: {error}");
        }
        if let Err(error) = event_loop.run_app(&mut application) {
            log::error!("Windows native event loop failed: {error}");
        }
    }

    fn quit(&self) {
        self.inner
            .native_owner_closing
            .store(true, Ordering::Release);
        if let Some(bridge) = &self.ui_bridge {
            let _ = bridge.event_loop.send_event(WindowsUserEvent::Quit);
            return;
        }
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
        if let Some(bridge) = &self.ui_bridge {
            let _ = bridge.event_loop.send_event(WindowsUserEvent::ActivateApp);
            return;
        }
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

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.inner.state.borrow().active_window_handle
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        if let Some(bridge) = &self.ui_bridge {
            let (reply, receiver) = std::sync::mpsc::channel();
            bridge
                .event_loop
                .send_event(WindowsUserEvent::NativeCommand(
                    WindowsNativeCommand::CreateWindow {
                        handle,
                        options,
                        reply,
                    },
                ))
                .map_err(|error| anyhow!("native Windows owner is closed: {error:?}"))?;
            let snapshot = receiver
                .recv()
                .context("native Windows owner did not answer window creation")??;
            return Ok(Box::new(WindowProxy::new(
                snapshot,
                handle,
                Rc::downgrade(&self.inner),
                bridge.event_loop.clone(),
                self.foreground_executor.clone(),
            )));
        }
        let creation_info = self.generate_creation_info();
        let cursor_style = self.inner.state.borrow().cursor_style;
        let window = with_active_context(|event_loop, app| {
            let event_loop_proxy = app
                .event_loop_proxy
                .lock()
                .map_err(|_| anyhow!("Windows event loop proxy mutex is poisoned"))?
                .clone()
                .context("Windows event loop proxy is not initialized")?;
            let window =
                WindowsWindow::new(event_loop, handle, options, creation_info, event_loop_proxy)?;
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
        // HWND is a process-local opaque handle. Move only its integer value across the worker
        // boundary so the windows crate's raw handle wrapper itself does not need to be Send.
        let owner = with_active_context(|_event_loop, app| app.focused_window_hwnd())
            .flatten()
            .map(|hwnd| hwnd.0 as isize);
        spawn_sta_dialog("gpui-file-open-dialog", move || {
            let owner = owner.map(|raw| HWND(raw as *mut _));
            file_open_dialog(options, owner)
        })
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> Receiver<Result<Option<PathBuf>>> {
        let directory = directory.to_owned();
        let suggested_name = suggested_name.map(str::to_owned);
        let owner = with_active_context(|_event_loop, app| app.focused_window_hwnd())
            .flatten()
            .map(|hwnd| hwnd.0 as isize);
        spawn_sta_dialog("gpui-file-save-dialog", move || {
            let owner = owner.map(|raw| HWND(raw as *mut _));
            file_save_dialog(directory, suggested_name, owner)
        })
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

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
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

        if let Some(bridge) = &self.ui_bridge {
            let _ = bridge
                .event_loop
                .send_event(WindowsUserEvent::SetCursorStyle(style));
            return;
        }

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
        // Windows Credential Manager calls are synchronous and may hit disk/LSA work. Keep them
        // off the GPUI foreground executor so login/token persistence cannot stall input or frames.
        self.background_executor().spawn(async move {
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
        // Windows Credential Manager calls are synchronous and may hit disk/LSA work. Keep them
        // off the GPUI foreground executor so login/token persistence cannot stall input or frames.
        self.background_executor().spawn(async move {
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
                return Ok(None);
            }

            // SAFETY: CredReadW succeeded, so this points to a valid CREDENTIALW until CredFree.
            let result = unsafe { username_and_password(&*credentials) };
            unsafe { CredFree(credentials as *const _ as _) };
            result.map(Some)
        })
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        let mut target_name = windows_credentials_target_name(url)
            .encode_utf16()
            .chain(Some(0))
            .collect_vec();
        // Windows Credential Manager calls are synchronous and may hit disk/LSA work. Keep them
        // off the GPUI foreground executor so login/token persistence cannot stall input or frames.
        self.background_executor().spawn(async move {
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
    pub(crate) fn handle_end_session(&self) -> bool {
        let callback = self.state.borrow_mut().callbacks.quit.take();
        let Some(mut callback) = callback else {
            return true;
        };

        let completed = callback();
        let mut state = self.state.borrow_mut();
        if state.callbacks.quit.is_none() {
            state.callbacks.quit = Some(callback);
        }
        completed
    }

    pub(crate) fn handle_power_broadcast(&self, wparam: WPARAM) {
        let suspended = match wparam.0 as u32 {
            PBT_APMSUSPEND => true,
            PBT_APMRESUMEAUTOMATIC => false,
            _ => return,
        };

        let callback = {
            let mut state = self.state.borrow_mut();
            if state.system_suspended == suspended {
                return;
            }
            state.system_suspended = suspended;
            if suspended {
                state.callbacks.system_sleep.take()
            } else {
                state.callbacks.system_wake.take()
            }
        };

        if let Some(mut callback) = callback {
            callback();
            let mut state = self.state.borrow_mut();
            let slot = if suspended {
                &mut state.callbacks.system_sleep
            } else {
                &mut state.callbacks.system_wake
            };
            if slot.is_none() {
                *slot = Some(callback);
            }
        }
    }

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
    creation_info: WindowCreationInfo,
    windows: FxHashMap<winit::window::WindowId, WindowsWindow>,
    focused_window_id: Option<winit::window::WindowId>,
    current_modifiers: Modifiers,
    pressed_button: Option<MouseButton>,
    hovered_window_id: Option<winit::window::WindowId>,
    pending_file_drops: FxHashMap<winit::window::WindowId, PendingFileDrop>,
    vsync_scheduler: Arc<super::vsync::VSyncScheduler>,
}

#[cfg(test)]
mod pending_file_drop_tests {
    use super::PendingFileDrop;
    use crate::{point, px};
    use std::path::PathBuf;

    #[test]
    fn multi_file_drop_submits_only_after_every_hovered_path_arrives() {
        let mut pending = PendingFileDrop::default();
        pending.push_hovered(PathBuf::from("a.mcpack"));
        pending.push_hovered(PathBuf::from("b.mcpack"));
        pending.push_hovered(PathBuf::from("c.mcpack"));

        pending.mark_dropped(PathBuf::from("a.mcpack"), point(px(0.0), px(0.0)));
        assert!(pending.is_ready_to_submit());
        pending.mark_dropped(PathBuf::from("b.mcpack"), point(px(0.0), px(0.0)));
        pending.mark_dropped(PathBuf::from("c.mcpack"), point(px(0.0), px(0.0)));
        assert_eq!(pending.external_paths().paths().len(), 3);
    }

    #[test]
    fn repeated_hover_and_drop_events_do_not_duplicate_paths() {
        let mut pending = PendingFileDrop::default();
        pending.push_hovered(PathBuf::from("a.mcpack"));
        pending.push_hovered(PathBuf::from("a.mcpack"));

        pending.mark_dropped(PathBuf::from("a.mcpack"), point(px(0.0), px(0.0)));
        pending.mark_dropped(PathBuf::from("a.mcpack"), point(px(1.0), px(1.0)));
        assert!(pending.is_ready_to_submit());
        assert_eq!(pending.external_paths().paths().len(), 1);
        assert_eq!(pending.submit_position, Some(point(px(1.0), px(1.0))));
    }
}

impl WindowsApplication {
    fn close_window(&mut self, event_loop: &ActiveEventLoop, window_id: winit::window::WindowId) {
        let Some(window) = self.windows.get(&window_id).cloned() else {
            return;
        };
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
            self.inner
                .native_owner_closing
                .store(true, Ordering::Release);
            event_loop.exit();
        }
    }

    fn create_native_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<WindowsNativeWindow> {
        let event_loop_proxy = self
            .event_loop_proxy
            .lock()
            .map_err(|_| anyhow!("Windows event loop proxy mutex is poisoned"))?
            .clone()
            .context("Windows event loop proxy is not initialized")?;
        let window = WindowsWindow::new(
            event_loop,
            handle,
            options,
            self.creation_info.clone(),
            event_loop_proxy,
        )?;
        let native_window = window
            .0
            .winit_window
            .get()
            .cloned()
            .context("native Windows window is not initialized")?;
        let snapshot = WindowsNativeWindow {
            window_id: window.window_id(),
            display: WindowsDisplay::from_window_monitor(&native_window),
            bounds: window.bounds(),
            window_bounds: window.window_bounds(),
            content_size: window.content_size(),
            scale_factor: window.scale_factor(),
            appearance: window.appearance(),
            background_appearance: window.background_appearance(),
            mouse_position: window.mouse_position(),
            modifiers: window.modifiers(),
            capslock: window.capslock(),
            active: window.is_active(),
            hovered: window.is_hovered(),
            visibility: window.visibility(),
            maximized: window.is_maximized(),
            minimized: window.is_minimized(),
            fullscreen: window.is_fullscreen(),
            gpu_specs: window.gpu_specs(),
            decorations: window.window_decorations(),
            default_client_inset: window.default_client_inset(),
            atlas: window.sprite_atlas(),
            window: native_window,
        };
        apply_cursor_style_to_window(window.window(), self.inner.state.borrow().cursor_style);
        self.windows.insert(snapshot.window_id, window);
        Ok(snapshot)
    }

    fn handle_native_command(
        &mut self,
        event_loop: &ActiveEventLoop,
        command: WindowsNativeCommand,
    ) {
        match command {
            WindowsNativeCommand::CreateWindow {
                handle,
                options,
                reply,
            } => {
                if reply
                    .send(self.create_native_window(event_loop, handle, options))
                    .is_err()
                {
                    log::warn!("Windows UI owner dropped native window creation reply");
                }
            }
            WindowsNativeCommand::CommitScene {
                window_id,
                mut packet,
                reply,
            } => {
                // UI Render may have started before an active compositor sample was presented.
                // Sample the committed visual timelines on the native owner's clock, otherwise
                // installing this scene can rewind geometry to the older UI frame timestamp.
                packet.frame_time = Instant::now();
                let result = self
                    .windows
                    .get(&window_id)
                    .map_or(PlatformFrameResult::Deferred, |window| window.draw(packet));
                if reply.send(result).is_err() {
                    log::warn!("Windows UI owner dropped native scene submission reply");
                }
            }
            WindowsNativeCommand::CommitLatestScene { window_id, mailbox } => {
                let Some(mut scene) = mailbox.lock().take() else {
                    return;
                };
                scene.prepare_for_native_frame(Instant::now());
                if let Some(window) = self.windows.get(&window_id) {
                    if scene.framebuffer_only {
                        window.present_framebuffer_only(scene.packet);
                    } else {
                        window.draw(scene.packet);
                    }
                }
            }
            WindowsNativeCommand::SetFrameRequestSender { window_id, sender } => {
                if let Some(window) = self.windows.get(&window_id) {
                    window.set_frame_request_sender(sender);
                }
            }
            WindowsNativeCommand::SetAnimationCompletionSender { window_id, sender } => {
                if let Some(window) = self.windows.get(&window_id) {
                    window.set_presentation_animation_completion_sender(sender);
                }
            }
            WindowsNativeCommand::SetEventSender { window_id, sender } => {
                if let Some(window) = self.windows.get(&window_id) {
                    let input_sender = sender.clone();
                    window.on_input(Box::new(move |input| {
                        if input_sender.send(WindowsNativeEvent::Input(input)).is_err() {
                            log::warn!("Windows UI owner dropped input event");
                        }
                        DispatchEventResult::default()
                    }));
                    let active_sender = sender.clone();
                    window.on_active_status_change(Box::new(move |active| {
                        let _ = active_sender.send(WindowsNativeEvent::Active(active));
                    }));
                    let visibility_sender = sender.clone();
                    window.on_visibility_change(Box::new(move |visibility| {
                        let _ = visibility_sender.send(WindowsNativeEvent::Visibility(visibility));
                    }));
                    let hover_sender = sender.clone();
                    window.on_hover_status_change(Box::new(move |hovered| {
                        let _ = hover_sender.send(WindowsNativeEvent::Hovered(hovered));
                    }));
                    let resize_sender = sender.clone();
                    window.on_resize(Box::new(move |size, scale| {
                        let _ = resize_sender.send(WindowsNativeEvent::Resized(size, scale));
                    }));
                    let move_sender = sender.clone();
                    window.on_moved(Box::new(move || {
                        let _ = move_sender.send(WindowsNativeEvent::Moved);
                    }));
                    let appearance_sender = sender.clone();
                    window.on_appearance_changed(Box::new(move || {
                        let _ = appearance_sender.send(WindowsNativeEvent::AppearanceChanged);
                    }));
                    let close_request_sender = sender.clone();
                    window.on_should_close(Box::new(move || {
                        close_request_sender
                            .send(WindowsNativeEvent::CloseRequested)
                            .is_err()
                    }));
                    window.on_close(Box::new(move || {
                        let _ = sender.send(WindowsNativeEvent::Closed);
                    }));
                }
            }
            WindowsNativeCommand::CloseWindow { window_id } => {
                self.close_window(event_loop, window_id);
            }
            WindowsNativeCommand::WindowAction { window_id, action } => {
                // Moving/resizing pumps native messages synchronously; release the registry
                // borrow before entering that loop.
                let Some(mut window) = self.windows.get(&window_id).cloned() else {
                    return;
                };
                match action {
                    WindowsWindowAction::RequestFrame(request) => window.request_frame(request),
                    WindowsWindowAction::FrameRequestTimedOut(request) => {
                        window.frame_request_timed_out(request);
                    }
                    WindowsWindowAction::Resize(size) => window.resize(size),
                    WindowsWindowAction::StartMove => window.start_window_move(),
                    WindowsWindowAction::StartResize(edge) => window.start_window_resize(edge),
                    WindowsWindowAction::SetTitle(title) => window.set_title(&title),
                    WindowsWindowAction::SetBackgroundAppearance(appearance) => {
                        window.set_background_appearance(appearance);
                    }
                    WindowsWindowAction::Activate => window.activate(),
                    WindowsWindowAction::Show => window.show(),
                    WindowsWindowAction::Hide => window.hide_window(),
                    WindowsWindowAction::Minimize => window.minimize(),
                    WindowsWindowAction::Maximize => window.maximize(),
                    WindowsWindowAction::Restore => window.restore(),
                    WindowsWindowAction::Zoom => window.zoom(),
                    WindowsWindowAction::ToggleFullscreen => window.toggle_fullscreen(),
                }
            }
            WindowsNativeCommand::WindowCall { window_id, call } => {
                call(self.windows.get_mut(&window_id));
            }
        }
    }

    fn run_foreground_tasks(&self, event_loop: &ActiveEventLoop) {
        let control_flow = if self.inner.run_foreground_tasks() {
            ControlFlow::Poll
        } else {
            ControlFlow::Wait
        };
        event_loop.set_control_flow(control_flow);
    }

    fn dispatch_pending_window_updates(&self, timing: super::vsync::VSyncEventTiming) {
        let event_received_at = Instant::now();
        let windows: Vec<_> = self.windows.values().cloned().collect();
        for window in windows {
            window.dispatch_pending_update_from_vsync(timing, event_received_at);
        }
    }

    fn sync_window_size(
        window: &WindowsWindow,
        physical_size: winit::dpi::PhysicalSize<u32>,
        scale_factor: f32,
    ) {
        window.sync_size(physical_size, scale_factor);
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

    fn flush_pending_file_drops(&mut self) {
        let ready = self
            .pending_file_drops
            .iter()
            .filter_map(|(window_id, pending)| pending.is_ready_to_submit().then_some(*window_id))
            .collect::<Vec<_>>();

        for window_id in ready {
            let Some(pending) = self.pending_file_drops.remove(&window_id) else {
                continue;
            };
            let Some(position) = pending.submit_position else {
                continue;
            };
            let Some(window) = self.windows.get(&window_id).cloned() else {
                continue;
            };

            let paths = ExternalPaths(pending.paths);
            let mut state = window.0.state.borrow_mut();
            let input_callback = state.callbacks.input.take();
            drop(state);
            if let Some(mut callback) = input_callback {
                // Winit emits one DroppedFile event per path. Waiting until AboutToWait
                // preserves the entire native event burst as one logical ExternalPaths drop,
                // without guessing completion from HoveredFile coverage.
                let _ = callback(PlatformInput::FileDrop(FileDropEvent::Entered {
                    position,
                    paths,
                }));
                let _ = callback(PlatformInput::FileDrop(FileDropEvent::Submit { position }));
                window.0.state.borrow_mut().callbacks.input = Some(callback);
            }
        }
    }

    fn focused_window_hwnd(&self) -> Option<HWND> {
        self.focused_window_id
            .and_then(|window_id| self.windows.get(&window_id))
            .and_then(WindowsWindow::native_hwnd)
    }
}

fn current_cursor_position(hwnd: HWND, scale_factor: f32) -> Option<Point<Pixels>> {
    let mut cursor = POINT::default();
    unsafe {
        GetCursorPos(&mut cursor).ok()?;
        if !ScreenToClient(hwnd, &mut cursor).as_bool() {
            return None;
        }
    }
    Some(point(
        px(cursor.x as f32 / scale_factor),
        px(cursor.y as f32 / scale_factor),
    ))
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
            WindowsUserEvent::RunMainThreadTasks => self.run_foreground_tasks(event_loop),
            WindowsUserEvent::VSync(timing) => self.dispatch_pending_window_updates(timing),
            WindowsUserEvent::RenderOwnerFrame { window_id, frame } => {
                if let Some(window) = self.windows.get(&window_id) {
                    window.report_render_owner_frame(frame);
                }
            }
            WindowsUserEvent::DockMenuAction(action_index) => {
                self.inner.handle_dock_action_event(action_index);
            }
            WindowsUserEvent::NativeCommand(command) => {
                self.handle_native_command(event_loop, command);
            }
            WindowsUserEvent::ActivateApp => self.activate_window(),
            WindowsUserEvent::SetCursorStyle(style) => {
                self.inner.state.borrow_mut().cursor_style = style;
                for window in self.windows.values() {
                    apply_cursor_style_to_window(window.window(), style);
                }
            }
            WindowsUserEvent::Quit => {
                self.inner
                    .native_owner_closing
                    .store(true, Ordering::Release);
                event_loop.exit();
            }
        }
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = None;
        });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        ACTIVE_CONTEXT.with(|storage| {
            *storage.borrow_mut() = Some((event_loop as *const _, self as *mut _));
        });
        self.flush_pending_file_drops();
        self.run_foreground_tasks(event_loop);
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
                Self::sync_window_size(&window, physical_size, scale_factor);
                // `sync_window_size` queues the newest extent and requests a platform frame.
                // Maximize/restore may emit this event reentrantly from a GPUI window-control
                // callback, so invoking the resize/frame callbacks here would borrow Window twice.
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
                    window.request_frame(PlatformFrameRequest::ui_commit());
                } else if self.focused_window_id == Some(window_id) {
                    self.focused_window_id = None;
                }
                self.sync_active_window_handle();
                window.invoke_active_status_change(active);
            }
            winit::event::WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let physical_size = window.window().inner_size();
                Self::sync_window_size(&window, physical_size, scale_factor as f32);
                self.refresh_display_cache(event_loop);
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
                    self.close_window(event_loop, window_id);
                }
            }
            winit::event::WindowEvent::RedrawRequested => {
                // WM_PAINT is also the foreground-task/frame pump while Win32 is inside its modal
                // size/move loop. Run queued work before consuming the newest resize generation.
                self.run_foreground_tasks(event_loop);
                window.dispatch_pending_update();
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
                entry.push_hovered(path);
                let paths = entry.external_paths();
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
                entry.mark_dropped(path, position);

                // Keep the active drag payload current for hover previews, but defer MouseUp
                // until AboutToWait so every DroppedFile from this native event burst joins the
                // same logical drop.
                let paths = entry.external_paths();
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
                    let modifiers = self.current_modifiers;
                    let scale_factor = window_state.scale_factor.get();
                    let position = window
                        .native_hwnd()
                        .and_then(|hwnd| current_cursor_position(hwnd, scale_factor))
                        .unwrap_or_else(|| window_state.mouse_position.get());
                    window_state.mouse_position.set(position);
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
                let scale_factor = state.scale_factor.get();
                let position = window
                    .native_hwnd()
                    .and_then(|hwnd| current_cursor_position(hwnd, scale_factor))
                    .unwrap_or_else(|| state.mouse_position.get());
                state.mouse_position.set(position);
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
                    winit::event::TouchPhase::Ended => TouchPhase::Ended,
                    winit::event::TouchPhase::Cancelled => TouchPhase::Cancelled,
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
            winit::event::WindowEvent::Touch(touch) => {
                let scale_factor = window.scale_factor();
                let position = point(
                    Pixels(touch.location.x as f32 / scale_factor),
                    Pixels(touch.location.y as f32 / scale_factor),
                );
                let phase = match touch.phase {
                    winit::event::TouchPhase::Started => TouchPhase::Started,
                    winit::event::TouchPhase::Moved => TouchPhase::Moved,
                    winit::event::TouchPhase::Ended => TouchPhase::Ended,
                    winit::event::TouchPhase::Cancelled => TouchPhase::Cancelled,
                };
                let force = touch.force.map(|force| force.normalized() as f32);
                let mut state = window.0.state.borrow_mut();
                let input_callback = state.callbacks.input.take();
                drop(state);
                if let Some(mut callback) = input_callback {
                    let _ = callback(PlatformInput::Touch(TouchEvent {
                        id: TouchId(touch.id),
                        phase,
                        position,
                        predicted_position: None,
                        force,
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
                        physical_key,
                        state,
                        text,
                        repeat,
                        ..
                    },
                ..
            } => {
                if let Some(keystroke) =
                    keystroke_from_winit(&logical_key, &physical_key, self.current_modifiers, &text)
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
        self.inner
            .native_owner_closing
            .store(true, Ordering::Release);
        self.vsync_scheduler.shutdown();
        if !self.inner.handle_end_session() {
            log::warn!(
                "GPUI shutdown callback remained borrowed while the Windows event loop exited"
            );
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

#[derive(Clone)]
pub(crate) struct WindowCreationInfo {
    pub(crate) executor: ForegroundExecutor,
    pub(crate) power_event: Rc<dyn Fn(WPARAM)>,
    pub(crate) end_session_event: Rc<dyn Fn() -> bool>,
    pub(crate) disable_direct_composition: bool,
    pub(crate) renderer_backend: RendererBackend,
    pub(crate) renderer_options: RendererOptions,
    pub(in crate::platform::windows) vsync_scheduler: Arc<super::vsync::VSyncScheduler>,
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

        if let Err(error) = folder_dialog.Show(window) {
            if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error.into());
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
        if let Err(error) = dialog.Show(window) {
            if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error.into());
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

/// Copies the optional username and secret out of a credential returned by CredReadW.
///
/// Credential Manager legally returns null pointers for an absent username and for an empty
/// credential blob. Treat both as empty values and finish copying before CredFree releases the
/// native allocation.
///
/// # Safety
///
/// A non-null UserName must point to a NUL-terminated wide string and a non-null CredentialBlob
/// must be readable for CredentialBlobSize bytes, as guaranteed for values returned by CredReadW.
unsafe fn username_and_password(credential: &CREDENTIALW) -> Result<(String, Vec<u8>)> {
    let username = if credential.UserName.is_null() {
        String::new()
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe { credential.UserName.to_string()? }
    };
    let password = if credential.CredentialBlob.is_null() {
        Vec::new()
    } else {
        // SAFETY: guaranteed by the caller.
        unsafe {
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
        }
        .to_vec()
    };
    Ok((username, password))
}

#[inline]
fn should_auto_hide_scrollbars() -> Result<bool> {
    let ui_settings = UISettings::new()?;
    Ok(ui_settings.AutoHideScrollBars()?)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, sync::atomic::Ordering};

    use super::{WINDOWS_AUTO_RENDERER_BACKEND_ORDER, username_and_password};
    use crate::{ClipboardItem, RendererBackend, read_from_clipboard, write_to_clipboard};
    use windows::Win32::Security::Credentials::CREDENTIALW;

    #[test]
    fn credential_copy_accepts_absent_username_and_secret() {
        let credential = CREDENTIALW::default();

        // SAFETY: both optional pointers are null, which the helper explicitly supports.
        let (username, password) = unsafe { username_and_password(&credential) }
            .expect("empty credential fields should be accepted");

        assert!(username.is_empty());
        assert!(password.is_empty());
    }

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

    #[test]
    fn windows_foreground_task_drain_clears_coalesced_wakeup() {
        let (inner, _background_executor, foreground_executor, _event_loop_proxy) =
            super::WindowsPlatform::new_common_parts(false);
        let task_ran = Rc::new(Cell::new(false));

        foreground_executor
            .spawn({
                let task_ran = task_ran.clone();
                async move {
                    task_ran.set(true);
                }
            })
            .detach();
        inner
            .main_thread_wakeup_pending
            .store(true, Ordering::Release);

        assert!(!inner.run_foreground_tasks());
        assert!(task_ran.get());
        assert!(!inner.main_thread_wakeup_pending.load(Ordering::Acquire));
    }

    #[test]
    fn windows_quit_marks_native_owner_closed_before_queued_ui_callbacks() {
        use crate::platform::traits::Platform as _;

        let platform = super::WindowsPlatform::new_headless();
        let observed_closing = Rc::new(Cell::new(false));
        let inner = platform.inner.clone();
        platform
            .foreground_executor
            .spawn({
                let observed_closing = observed_closing.clone();
                let inner = inner.clone();
                async move {
                    observed_closing.set(inner.native_owner_closing.load(Ordering::Acquire));
                }
            })
            .detach();

        assert!(!inner.native_owner_closing.load(Ordering::Acquire));
        platform.quit();
        assert!(!inner.run_foreground_tasks());
        assert!(observed_closing.get());
    }
}

#[cfg(test)]
mod file_drop_batch_tests {
    use super::PendingFileDrop;
    use crate::{point, px};
    use std::path::PathBuf;

    #[test]
    fn pending_file_drop_keeps_complete_unique_batch() {
        let mut pending = PendingFileDrop::default();
        pending.push_hovered(PathBuf::from("a.mcpack"));
        pending.push_hovered(PathBuf::from("b.mcpack"));
        pending.push_hovered(PathBuf::from("a.mcpack"));
        pending.mark_dropped(PathBuf::from("c.mcpack"), point(px(10.0), px(20.0)));

        assert!(pending.is_ready_to_submit());
        assert_eq!(
            pending.paths.as_slice(),
            &[
                PathBuf::from("a.mcpack"),
                PathBuf::from("b.mcpack"),
                PathBuf::from("c.mcpack"),
            ]
        );
    }
}

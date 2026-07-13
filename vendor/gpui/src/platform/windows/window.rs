#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use ::util::ResultExt;
use anyhow::{Context as _, Result};
use futures::channel::oneshot::{self, Receiver};
use slotmap::Key;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::{
            Controls::*,
            WindowsAndMessaging::{
                HICON, ICON_BIG, ICON_SMALL, IDCANCEL, IDOK, IMAGE_ICON, IsIconic, IsZoomed,
                LR_DEFAULTSIZE, LR_SHARED, LoadImageW, SW_RESTORE, SendMessageW,
                SetForegroundWindow, ShowWindow, WM_SETICON,
            },
        },
    },
    core::*,
};

use crate::diagnostics::performance_metrics::{
    record_frame_request, record_renderer_backend, record_window_request_redraw,
};
use crate::platform::windows::with_dll_library;
use crate::platform::winit::{
    maximize_window, minimize_window, request_window_inner_size,
    restore_window as restore_winit_window, start_window_move as start_winit_window_move,
    start_window_resize as start_winit_window_resize, toggle_window_fullscreen,
    toggle_window_maximized,
};
use crate::platform::{NovaRenderer, NovaRendererAtlas};
use crate::*;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows, WindowExtWindows};
use winit::raw_window_handle as rwh;
use winit::raw_window_handle::HasWindowHandle as _;
use winit::window::Window as WinitWindow;

pub(crate) struct WindowsWindow(pub Rc<WindowsWindowInner>);

fn should_use_native_decorations(params: &WindowParams) -> bool {
    if params.kind == WindowKind::PopUp {
        return false;
    }

    !params
        .titlebar
        .as_ref()
        .is_some_and(|titlebar| titlebar.appears_transparent)
}

fn should_use_transparent_background(params: &WindowParams) -> bool {
    params.window_background != WindowBackgroundAppearance::Opaque
}

fn should_use_no_redirection_bitmap(
    disable_direct_composition: bool,
    transparent_background: bool,
    renderer_backend_candidates: &[RendererBackend],
) -> bool {
    !disable_direct_composition
        && transparent_background
        && renderer_backend_candidates.contains(&RendererBackend::NovaDx12)
}

fn renderer_backend_candidates(
    renderer_options: &RendererOptions,
    resolved_backend: RendererBackend,
    transparent: bool,
) -> Vec<RendererBackend> {
    let mut candidates = vec![resolved_backend];
    let should_try_fallbacks = renderer_options.backend == RendererBackend::Auto
        || (transparent
            && matches!(
                renderer_options.backend,
                RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
            ));
    if should_try_fallbacks {
        for backend in super::platform::windows_auto_renderer_backend_order() {
            if !candidates.contains(backend) {
                candidates.push(*backend);
            }
        }
    }
    candidates
}

fn accent_state_for_background(background_appearance: WindowBackgroundAppearance) -> u32 {
    match background_appearance {
        WindowBackgroundAppearance::Opaque => 0,
        WindowBackgroundAppearance::Transparent | WindowBackgroundAppearance::Blurred => 2,
    }
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: u32,
    data: *mut c_void,
    data_size: usize,
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

fn window_corner_preference_to_windows(
    preference: WindowCornerPreference,
) -> Option<CornerPreference> {
    match preference {
        WindowCornerPreference::SystemDefault => None,
        WindowCornerPreference::Rounded => Some(CornerPreference::Round),
        WindowCornerPreference::RoundedSmall => Some(CornerPreference::RoundSmall),
        WindowCornerPreference::Square => Some(CornerPreference::DoNotRound),
    }
}

impl Clone for WindowsWindow {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl WindowsWindow {
    fn default_resize_inset() -> Pixels {
        px(8.0)
    }

    fn native_is_maximized(&self) -> Option<bool> {
        let hwnd = self.native_hwnd()?;
        if hwnd.is_invalid() {
            return None;
        }
        // SAFETY: The HWND comes from the live winit window handle and was checked for null.
        Some(unsafe { IsZoomed(hwnd).as_bool() })
    }

    fn apply_process_default_window_icon(hwnd: HWND) {
        let Some(module) = (unsafe { GetModuleHandleW(None) }).ok() else {
            return;
        };
        let Some(icon) = (unsafe {
            LoadImageW(
                Some(module.into()),
                PCWSTR(1 as _),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_SHARED,
            )
        })
        .ok()
        .map(|handle| HICON(handle.0)) else {
            return;
        };

        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }

    pub(crate) fn window(&self) -> &WinitWindow {
        &self
            .0
            .winit_window
            .get()
            .expect("winit_window should be initialized")
    }

    pub(crate) fn window_id(&self) -> winit::window::WindowId {
        self.window().id()
    }

    pub(crate) fn native_hwnd(&self) -> Option<HWND> {
        let raw_handle = self.window().window_handle().ok()?.as_raw();
        match raw_handle {
            rwh::RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    pub(crate) fn try_borrow_state(
        &self,
    ) -> Result<std::cell::RefMut<'_, WindowsWindowState>, std::cell::BorrowMutError> {
        self.0.state.try_borrow_mut()
    }

    pub(crate) fn invoke_resize(&self, size: Size<Pixels>, scale_factor: f32) {
        let mut state = self.0.state.borrow_mut();
        if let Some(mut callback) = state.callbacks.resize.take() {
            drop(state);
            callback(size, scale_factor);
            self.0.state.borrow_mut().callbacks.resize = Some(callback);
        }
    }

    pub(crate) fn invoke_active_status_change(&self, is_active: bool) {
        let mut state = self.0.state.borrow_mut();
        if let Some(mut callback) = state.callbacks.active_status_change.take() {
            drop(state);
            callback(is_active);
            self.0.state.borrow_mut().callbacks.active_status_change = Some(callback);
        }
    }

    pub(crate) fn should_close(&self) -> Option<bool> {
        let mut state = self.0.state.borrow_mut();
        let mut callback = state.callbacks.should_close.take()?;
        drop(state);
        let should_close = callback();
        self.0.state.borrow_mut().callbacks.should_close = Some(callback);
        Some(should_close)
    }

    pub(crate) fn invoke_close(&self) {
        let callback = self.0.state.borrow_mut().callbacks.close.take();
        if let Some(callback) = callback {
            callback();
        }
    }

    pub(crate) fn invoke_request_frame(&self, options: RequestFrameOptions) {
        let mut state = self.0.state.borrow_mut();
        if let Some(mut callback) = state.callbacks.request_frame.take() {
            drop(state);
            callback(options);
            self.0.state.borrow_mut().callbacks.request_frame = Some(callback);
        }
    }

    pub(crate) fn take_pending_frame_request(&self) -> RequestFrameOptions {
        let state = self.0.state.borrow();
        let request = state.pending_frame_request.get();
        state
            .pending_frame_request
            .set(RequestFrameOptions::default());
        request
    }

    pub(crate) fn request_frame(&self, options: RequestFrameOptions) {
        if !self.0.queue_frame_request(options) {
            return;
        }

        // Keep frame delivery in the native event queue. On Windows, redraw work runs after
        // higher-priority input and window messages, so a continuously animating view cannot
        // monopolize the foreground executor. The frame watchdog remains the fallback when a
        // requested redraw is not delivered.
        record_window_request_redraw(self.0.handle.window_id().data().as_ffi());
        self.window().request_redraw();
    }

    fn request_first_presentable_frame(&self) {
        let options = RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        };
        let callback_registered = self.0.state.borrow().callbacks.request_frame.is_some();
        if !callback_registered {
            self.request_frame(options);
            return;
        }

        // Windows can suppress RedrawRequested for a hidden HWND. Once renderer initialization
        // completes, dispatch exactly this first presentable frame directly so visibility does
        // not wait on a native redraw that cannot arrive until the window is already visible.
        let pending = self.take_pending_frame_request();
        record_frame_request();
        self.invoke_request_frame(pending.merge(options));
    }

    pub(crate) fn clear_timed_out_frame_request(&self, _options: RequestFrameOptions) {
        let state = self.0.state.borrow();
        state
            .pending_frame_request
            .set(clear_pending_frame_request_after_timeout(
                state.pending_frame_request.get(),
            ));
    }

    pub(crate) fn restore_minimized_window(&self) {
        let Some(hwnd) = self.native_hwnd() else {
            return;
        };
        if hwnd.is_invalid() {
            return;
        }

        // SAFETY: The HWND comes from the live winit window handle and was checked for null.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }

    pub(crate) fn bring_to_foreground(&self) {
        let Some(hwnd) = self.native_hwnd() else {
            return;
        };
        if hwnd.is_invalid() {
            return;
        }

        // SAFETY: The HWND comes from the live winit window handle and was checked for null.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub struct WindowsWindowState {
    pub callbacks: Callbacks,
    pub mouse_position: Cell<Point<Pixels>>,
    pub modifiers: Cell<Modifiers>,
    pub capslock: Cell<Capslock>,
    pub hovered: Cell<bool>,
    pub logical_size: Cell<Size<Pixels>>,
    pub scale_factor: Cell<f32>,
    background_appearance: Cell<WindowBackgroundAppearance>,
    pending_frame_request: Cell<RequestFrameOptions>,
    pub click_state: RefCell<ClickState>,
}

pub enum WindowsRenderer {
    Nova(NovaRenderer),
}

impl WindowsRenderer {
    fn new(initialization: WindowsRendererInitialization) -> Result<Self> {
        let WindowsRendererInitialization {
            window,
            logical_size,
            scale_factor,
            disable_direct_composition,
            renderer_backend_candidates,
            renderer_options,
            window_id,
            transparent,
            atlas,
        } = initialization;
        let drawable_size = logical_size
            .to_device_pixels(scale_factor)
            .map(|axis| DevicePixels(axis.0.max(1)));
        let candidate_count = renderer_backend_candidates.len();
        let mut last_error = None;

        for (candidate_index, candidate) in renderer_backend_candidates.into_iter().enumerate() {
            match NovaRenderer::new_with_atlas(
                &window,
                candidate,
                &renderer_options,
                GpuSubmissionMode::Deferred,
                drawable_size,
                transparent,
                atlas.clone(),
            ) {
                Ok(renderer) => {
                    let gpu_specs = renderer.gpu_specs();
                    log::info!(
                        "Created Windows nova/{} renderer: gpu=\"{}\" driver=\"{}\" info=\"{}\" software={}",
                        candidate,
                        gpu_specs.device_name,
                        gpu_specs.driver_name,
                        gpu_specs.driver_info,
                        gpu_specs.is_software_emulated
                    );
                    record_renderer_backend(candidate);
                    let _ = (disable_direct_composition, window_id);
                    return Ok(Self::Nova(renderer));
                }
                Err(error) => {
                    let should_try_next = candidate_index + 1 < candidate_count;
                    if should_try_next {
                        log::warn!(
                            "Windows nova/{} renderer failed; trying next backend: {error:#}",
                            candidate
                        );
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }

        let _ = (disable_direct_composition, window_id);
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no Windows nova renderer candidates")))
    }

    pub fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        match self {
            Self::Nova(renderer) => renderer.resize(size),
        }
    }

    pub fn draw(&mut self, render_plan: FrameRenderPlan<'_>) -> Result<()> {
        match self {
            Self::Nova(renderer) => renderer.draw(render_plan),
        }
    }

    pub fn present_framebuffer_only(&mut self, render_plan: FrameRenderPlan<'_>) -> Result<()> {
        match self {
            Self::Nova(renderer) => renderer.present_framebuffer_only(render_plan),
        }
    }

    pub fn update_transparency(&mut self, is_transparent: bool) {
        match self {
            Self::Nova(renderer) => renderer.update_transparency(is_transparent),
        }
    }

    pub fn gpu_specs(&self) -> Result<GpuSpecs> {
        match self {
            Self::Nova(renderer) => Ok(renderer.gpu_specs()),
        }
    }
}

fn apply_window_background_appearance(
    hwnd: HWND,
    background_appearance: WindowBackgroundAppearance,
) {
    if hwnd.is_invalid() {
        return;
    }

    type SetWindowCompositionAttribute =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> i32;

    let result = with_dll_library(windows::core::s!("user32.dll"), |library| {
        // SAFETY: The DLL is loaded for the duration of this closure and the symbol name is fixed.
        let proc =
            unsafe { GetProcAddress(library, windows::core::s!("SetWindowCompositionAttribute")) };
        let Some(proc) = proc else {
            anyhow::bail!("SetWindowCompositionAttribute is unavailable");
        };
        // SAFETY: The symbol is dynamically resolved from user32.dll and this signature matches
        // winit's dark mode use and GPUI's previous Windows backend.
        let set_window_composition_attribute: SetWindowCompositionAttribute =
            unsafe { std::mem::transmute(proc) };
        let accent = AccentPolicy {
            state: accent_state_for_background(background_appearance),
            flags: 2,
            gradient_color: 0,
            animation_id: 0,
        };
        let mut data = WindowCompositionAttributeData {
            attribute: 0x13,
            data: &accent as *const _ as *mut c_void,
            data_size: std::mem::size_of::<AccentPolicy>(),
        };

        // SAFETY: `hwnd` is a live window handle, and `data` points to stack values that remain
        // valid for the duration of the synchronous call.
        let status = unsafe { set_window_composition_attribute(hwnd, &mut data) };
        if status == 0 {
            anyhow::bail!("SetWindowCompositionAttribute returned false");
        }
        Ok(())
    });

    if let Err(error) = result {
        log::debug!("applying Windows transparent background failed: {error:#}");
    }
}

impl Drop for WindowsRenderer {
    fn drop(&mut self) {
        let Self::Nova(renderer) = self;
        renderer.destroy();
    }
}

enum WindowsRendererState {
    Initializing,
    Ready(WindowsRenderer),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeWindowVisibilityAction {
    None,
    Show { focus: bool },
    Hide,
    Focus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsWindowPresentationState {
    mapped: bool,
    show_requested: bool,
    first_frame_presented: bool,
    native_visible: bool,
    focus_requested: bool,
}

impl WindowsWindowPresentationState {
    fn new(show_requested: bool, focus_requested: bool) -> Self {
        Self {
            mapped: false,
            show_requested,
            first_frame_presented: false,
            native_visible: false,
            focus_requested,
        }
    }

    fn map(&mut self) -> NativeWindowVisibilityAction {
        self.mapped = true;
        self.reconcile()
    }

    fn request_show(&mut self) -> NativeWindowVisibilityAction {
        self.show_requested = true;
        self.reconcile()
    }

    fn request_hide(&mut self) -> NativeWindowVisibilityAction {
        self.show_requested = false;
        self.focus_requested = false;
        self.reconcile()
    }

    fn request_activation(&mut self) -> NativeWindowVisibilityAction {
        self.show_requested = true;
        self.focus_requested = true;
        self.reconcile()
    }

    fn first_frame_presented(&mut self) -> NativeWindowVisibilityAction {
        self.first_frame_presented = true;
        self.reconcile()
    }

    fn reconcile(&mut self) -> NativeWindowVisibilityAction {
        let should_be_visible = self.mapped && self.show_requested && self.first_frame_presented;
        match (self.native_visible, should_be_visible) {
            (false, true) => {
                self.native_visible = true;
                let focus = std::mem::take(&mut self.focus_requested);
                NativeWindowVisibilityAction::Show { focus }
            }
            (true, false) => {
                self.native_visible = false;
                NativeWindowVisibilityAction::Hide
            }
            (true, true) if std::mem::take(&mut self.focus_requested) => {
                NativeWindowVisibilityAction::Focus
            }
            _ => NativeWindowVisibilityAction::None,
        }
    }
}

struct InitializedWindowsRenderer(WindowsRenderer);

// SAFETY: Windows renderer initialization exclusively owns its DX12 or Vulkan device, surface,
// swapchain, and resource handles. This wrapper is moved exactly once from the initialization
// worker to the foreground thread, and the renderer is never accessed concurrently on both
// threads. D3D12/DXGI and Vulkan handles do not require destruction on their creation thread.
unsafe impl Send for InitializedWindowsRenderer {}

struct WindowsRendererInitialization {
    window: WindowsRendererWindowHandle,
    logical_size: Size<Pixels>,
    scale_factor: f32,
    disable_direct_composition: bool,
    renderer_backend_candidates: Vec<RendererBackend>,
    renderer_options: RendererOptions,
    window_id: WindowId,
    transparent: bool,
    atlas: NovaRendererAtlas,
}

struct WindowsRendererWindowHandle {
    _window: Arc<WinitWindow>,
    raw_window_handle: rwh::RawWindowHandle,
}

impl WindowsRendererWindowHandle {
    fn new(window: Arc<WinitWindow>) -> Result<Self> {
        let raw_window_handle = window
            .window_handle()
            .context("capturing Windows renderer window handle")?
            .as_raw();
        anyhow::ensure!(
            matches!(raw_window_handle, rwh::RawWindowHandle::Win32(_)),
            "Windows renderer requires a Win32 window handle"
        );
        Ok(Self {
            _window: window,
            raw_window_handle,
        })
    }
}

// SAFETY: The raw Win32 handle is captured on the main thread and remains valid because the
// wrapper owns an Arc to the winit window. Background initialization only borrows the immutable
// HWND/HINSTANCE values to create a graphics surface; it never calls winit APIs off-thread.
unsafe impl Send for WindowsRendererWindowHandle {}

impl rwh::HasWindowHandle for WindowsRendererWindowHandle {
    fn window_handle(&self) -> std::result::Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        // SAFETY: The raw handle lifetime is bounded by self, which keeps the winit window alive.
        Ok(unsafe { rwh::WindowHandle::borrow_raw(self.raw_window_handle) })
    }
}

impl rwh::HasDisplayHandle for WindowsRendererWindowHandle {
    fn display_handle(&self) -> std::result::Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::windows())
    }
}

pub(crate) struct WindowsWindowInner {
    pub(crate) use_native_decorations: bool,
    pub(crate) state: RefCell<WindowsWindowState>,
    pub(crate) input_handler: RefCell<Option<PlatformInputHandler>>,
    pub(crate) handle: AnyWindowHandle,
    pub(crate) executor: ForegroundExecutor,
    renderer: RefCell<WindowsRendererState>,
    renderer_atlas: NovaRendererAtlas,
    presentation_state: Cell<WindowsWindowPresentationState>,
    pub(crate) pending_renderer_size: Cell<Option<Size<DevicePixels>>>,
    pub(crate) renderer_resize_retry_pending: Cell<bool>,
    pub(crate) winit_window: OnceCell<Arc<WinitWindow>>,
}

impl WindowsWindowInner {
    fn queue_frame_request(&self, options: RequestFrameOptions) -> bool {
        let state = self.state.borrow();
        let pending = state.pending_frame_request.get();
        let (pending, should_schedule_frame) = merge_frame_request(pending, options);
        state.pending_frame_request.set(pending);
        drop(state);
        record_frame_request();
        should_schedule_frame
    }

    fn window(&self) -> &WinitWindow {
        self.winit_window
            .get()
            .expect("winit_window should be initialized")
    }
}

fn merge_frame_request(
    pending: RequestFrameOptions,
    options: RequestFrameOptions,
) -> (RequestFrameOptions, bool) {
    let already_pending = pending.requires_frame();
    (
        pending.merge(options),
        !already_pending && options.requires_frame(),
    )
}

fn clear_pending_frame_request_after_timeout(pending: RequestFrameOptions) -> RequestFrameOptions {
    // A pending request is tied to one foreground callback. If that callback timed out,
    // merged flags in the same slot are stranded too.
    if pending.requires_frame() {
        RequestFrameOptions::default()
    } else {
        pending
    }
}

#[derive(Default)]
pub(crate) struct Callbacks {
    pub(crate) request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    pub(crate) input: Option<Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>>,
    pub(crate) active_status_change: Option<Box<dyn FnMut(bool)>>,
    pub(crate) hovered_status_change: Option<Box<dyn FnMut(bool)>>,
    pub(crate) resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    pub(crate) moved: Option<Box<dyn FnMut()>>,
    pub(crate) should_close: Option<Box<dyn FnMut() -> bool>>,
    pub(crate) close: Option<Box<dyn FnOnce()>>,
    pub(crate) hit_test_window_control: Option<Box<dyn FnMut() -> Option<WindowControlArea>>>,
    pub(crate) appearance_changed: Option<Box<dyn FnMut()>>,
}

impl WindowsWindow {
    pub(crate) fn new(
        event_loop: &ActiveEventLoop,
        handle: AnyWindowHandle,
        params: WindowParams,
        creation_info: WindowCreationInfo,
    ) -> Result<Self> {
        let WindowCreationInfo {
            background_executor,
            executor,
            disable_direct_composition,
            renderer_backend,
            renderer_options,
        } = creation_info;
        let title = params
            .titlebar
            .as_ref()
            .and_then(|titlebar| titlebar.title.as_ref())
            .map(|title| title.to_string())
            .unwrap_or_else(String::new);
        let native_icon = params.window_icon.as_ref().and_then(|icon| {
            winit::window::Icon::from_rgba(icon.rgba.as_ref().to_vec(), icon.width, icon.height)
                .log_err()
        });
        let transparent_background = should_use_transparent_background(&params);
        let use_native_decorations = should_use_native_decorations(&params);
        let client_corner_preference =
            window_corner_preference_to_windows(params.window_corner_preference);
        let renderer_backend_candidates = renderer_backend_candidates(
            &renderer_options,
            renderer_backend,
            transparent_background,
        );
        let presentation_state = WindowsWindowPresentationState::new(params.show, params.focus);

        let mut attributes = WinitWindow::default_attributes()
            .with_title(title)
            .with_resizable(params.is_resizable)
            .with_visible(false)
            .with_position(LogicalPosition::new(
                params.bounds.origin.x.0 as f64,
                params.bounds.origin.y.0 as f64,
            ))
            .with_inner_size(LogicalSize::new(
                params.bounds.size.width.0 as f64,
                params.bounds.size.height.0 as f64,
            ))
            .with_active(false)
            .with_transparent(transparent_background)
            .with_no_redirection_bitmap(should_use_no_redirection_bitmap(
                disable_direct_composition,
                transparent_background,
                &renderer_backend_candidates,
            ));
        if !use_native_decorations {
            attributes = attributes.with_undecorated_shadow(true);
            if let Some(corner_preference) = client_corner_preference {
                attributes = attributes.with_corner_preference(corner_preference);
            }
        }
        attributes = attributes.with_window_icon(native_icon);
        if let Some(min_size) = params.window_min_size {
            attributes = attributes.with_min_inner_size(LogicalSize::new(
                min_size.width.0 as f64,
                min_size.height.0 as f64,
            ));
        }
        attributes = attributes.with_decorations(use_native_decorations);

        let winit_window = event_loop
            .create_window(attributes)
            .context("creating winit window")?;
        let hwnd = Self::native_hwnd_from_winit_window(&winit_window);
        if let Some(hwnd) = hwnd {
            apply_window_background_appearance(hwnd, params.window_background);
        }
        let scale_factor = winit_window.scale_factor() as f32;
        let actual_inner_size = winit_window.inner_size();
        let actual_logical_size = Size {
            width: Pixels(actual_inner_size.width as f32 / scale_factor),
            height: Pixels(actual_inner_size.height as f32 / scale_factor),
        };
        if params.window_icon.is_none()
            && let Some(hwnd) = hwnd
        {
            Self::apply_process_default_window_icon(hwnd);
        }
        if !use_native_decorations {
            winit_window.set_undecorated_shadow(true);
            if let Some(corner_preference) = client_corner_preference {
                winit_window.set_corner_preference(corner_preference);
            }
        }
        let winit_window = Arc::new(winit_window);
        let renderer_atlas = NovaRendererAtlas::new();
        let renderer_initialization = WindowsRendererInitialization {
            window: WindowsRendererWindowHandle::new(winit_window.clone())?,
            logical_size: actual_logical_size,
            scale_factor,
            disable_direct_composition,
            renderer_backend_candidates,
            renderer_options,
            window_id: handle.window_id(),
            transparent: transparent_background,
            atlas: renderer_atlas.clone(),
        };
        let cell = OnceCell::new();
        cell.set(winit_window)
            .map_err(|_| anyhow::anyhow!("Windows winit window was initialized twice"))?;
        let window = Self(Rc::new(WindowsWindowInner {
            use_native_decorations,
            state: RefCell::new(WindowsWindowState {
                callbacks: Callbacks::default(),
                mouse_position: Cell::new(Point::default()),
                modifiers: Cell::new(Modifiers::default()),
                capslock: Cell::new(Capslock::default()),
                hovered: Cell::new(false),
                logical_size: Cell::new(actual_logical_size),
                scale_factor: Cell::new(scale_factor),
                background_appearance: Cell::new(params.window_background),
                pending_frame_request: Cell::new(RequestFrameOptions::default()),
                click_state: RefCell::new(ClickState::new()),
            }),
            input_handler: RefCell::new(None),
            handle,
            executor,
            renderer: RefCell::new(WindowsRendererState::Initializing),
            renderer_atlas,
            presentation_state: Cell::new(presentation_state),
            pending_renderer_size: Cell::new(None),
            renderer_resize_retry_pending: Cell::new(false),
            winit_window: cell,
        }));
        window.start_renderer_initialization(background_executor, renderer_initialization);
        Ok(window)
    }

    fn start_renderer_initialization(
        &self,
        background_executor: BackgroundExecutor,
        initialization: WindowsRendererInitialization,
    ) {
        let (sender, receiver) = oneshot::channel();
        background_executor
            .spawn(async move {
                let renderer = WindowsRenderer::new(initialization).map(InitializedWindowsRenderer);
                if sender.send(renderer).is_err() {
                    log::debug!("Windows renderer initialization receiver was dropped");
                }
            })
            .detach();

        let weak_window = Rc::downgrade(&self.0);
        self.0
            .executor
            .spawn(async move {
                let renderer = receiver.await.unwrap_or_else(|error| {
                    Err(anyhow::anyhow!(
                        "Windows renderer initialization task was cancelled: {error}"
                    ))
                });
                if let Some(window) = weak_window.upgrade() {
                    WindowsWindow(window)
                        .finish_renderer_initialization(renderer.map(|renderer| renderer.0));
                }
            })
            .detach();
    }

    fn finish_renderer_initialization(&self, renderer: Result<WindowsRenderer>) {
        match renderer {
            Ok(mut renderer) => {
                let transparent = self.0.state.borrow().background_appearance.get()
                    != WindowBackgroundAppearance::Opaque;
                renderer.update_transparency(transparent);
                *self.0.renderer.borrow_mut() = WindowsRendererState::Ready(renderer);
                self.0.renderer_resize_retry_pending.set(false);
                self.request_first_presentable_frame();
            }
            Err(error) => {
                *self.0.renderer.borrow_mut() = WindowsRendererState::Failed;
                log::error!("failed to initialize Windows renderer: {error:#}");
                self.window().set_visible(false);
                self.invoke_close();
            }
        }
    }

    fn update_presentation_state(
        &self,
        update: impl FnOnce(&mut WindowsWindowPresentationState) -> NativeWindowVisibilityAction,
    ) {
        let mut state = self.0.presentation_state.get();
        let action = update(&mut state);
        self.0.presentation_state.set(state);
        self.apply_native_visibility_action(action);
    }

    fn apply_native_visibility_action(&self, action: NativeWindowVisibilityAction) {
        match action {
            NativeWindowVisibilityAction::None => {}
            NativeWindowVisibilityAction::Show { focus } => {
                log::debug!(
                    "showing Windows window after a completed frame: window={}",
                    self.0.handle.window_id().data().as_ffi()
                );
                self.window().set_visible(true);
                self.restore_minimized_window();
                if focus {
                    self.window().focus_window();
                    self.bring_to_foreground();
                }
            }
            NativeWindowVisibilityAction::Hide => self.window().set_visible(false),
            NativeWindowVisibilityAction::Focus => {
                self.restore_minimized_window();
                self.window().focus_window();
                self.bring_to_foreground();
            }
        }
    }

    fn mark_first_frame_presented(&self) {
        self.update_presentation_state(WindowsWindowPresentationState::first_frame_presented);
    }

    fn native_hwnd_from_winit_window(window: &WinitWindow) -> Option<HWND> {
        let raw_handle = window.window_handle().ok()?.as_raw();
        match raw_handle {
            rwh::RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
            _ => None,
        }
    }

    pub(crate) fn queue_renderer_resize(&self, size: Size<DevicePixels>) {
        self.0.pending_renderer_size.set(Some(size));
        self.0.renderer_resize_retry_pending.set(false);
    }

    fn try_apply_queued_renderer_resize(&self) -> bool {
        let mut renderer_state = self.0.renderer.borrow_mut();
        let WindowsRendererState::Ready(renderer) = &mut *renderer_state else {
            return false;
        };
        let Some(size) = self.0.pending_renderer_size.take() else {
            return true;
        };

        self.0.renderer_resize_retry_pending.set(false);
        if let Err(error) = renderer.resize(size) {
            log::warn!("failed to resize Windows renderer: {error:#}");
            self.0.pending_renderer_size.set(Some(size));
            if !self.0.renderer_resize_retry_pending.replace(true) {
                self.request_frame(RequestFrameOptions::from_refresh());
            }
            return false;
        }

        self.0.renderer_resize_retry_pending.set(false);
        true
    }
}

impl rwh::HasWindowHandle for WindowsWindow {
    fn window_handle(&self) -> std::result::Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        self.window().window_handle()
    }
}

impl rwh::HasDisplayHandle for WindowsWindow {
    fn display_handle(&self) -> std::result::Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::windows())
    }
}

impl Drop for WindowsWindow {
    fn drop(&mut self) {}
}

impl PlatformWindow for WindowsWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        let state = self.0.state.borrow();
        let scale_factor = state.scale_factor.get();
        let logical_size = state.logical_size.get();
        let origin = self
            .window()
            .outer_position()
            .map(|position| Point {
                x: Pixels(position.x as f32 / scale_factor),
                y: Pixels(position.y as f32 / scale_factor),
            })
            .unwrap_or_default();

        Bounds {
            origin,
            size: logical_size,
        }
    }

    fn is_maximized(&self) -> bool {
        self.native_is_maximized()
            .unwrap_or_else(|| self.window().is_maximized())
    }

    fn is_minimized(&self) -> bool {
        self.window().is_minimized().unwrap_or(false)
    }

    fn window_bounds(&self) -> WindowBounds {
        let bounds = self.bounds();
        if self.window().fullscreen().is_some() {
            WindowBounds::Fullscreen(bounds)
        } else if self.is_maximized() {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }

    /// get the logical size of the app's drawable area.
    ///
    /// Currently, GPUI uses the logical size of the app to handle mouse interactions (such as
    /// whether the mouse collides with other elements of GPUI).
    fn content_size(&self) -> Size<Pixels> {
        self.0.state.borrow().logical_size.get()
    }

    fn resize(&mut self, size: Size<Pixels>) {
        request_window_inner_size(self.window(), size);
    }

    fn scale_factor(&self) -> f32 {
        self.0.state.borrow().scale_factor.get()
    }

    fn appearance(&self) -> WindowAppearance {
        match self.window().theme() {
            Some(winit::window::Theme::Light) => WindowAppearance::Light,
            Some(winit::window::Theme::Dark) => WindowAppearance::Dark,
            None => WindowAppearance::default(),
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        WindowsDisplay::from_window_monitor(self.window())
            .map(|display| Rc::new(display) as Rc<dyn PlatformDisplay>)
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.0.state.borrow().mouse_position.get()
    }

    fn modifiers(&self) -> Modifiers {
        self.0.state.borrow().modifiers.get()
    }

    fn capslock(&self) -> Capslock {
        self.0.state.borrow().capslock.get()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        if let Ok(mut slot) = self.0.input_handler.try_borrow_mut() {
            *slot = Some(input_handler);
        } else {
            log::warn!("input handler is already borrowed while setting a new handler");
        }
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        match self.0.input_handler.try_borrow_mut() {
            Ok(mut slot) => slot.take(),
            Err(_) => {
                log::warn!("input handler is already borrowed while taking the handler");
                None
            }
        }
    }

    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<Receiver<usize>> {
        let (done_tx, done_rx) = oneshot::channel();
        let msg = msg.to_string();
        let detail_string = detail.map(|detail| detail.to_string());
        let prompt_text = msg.clone();
        let handle = self.native_hwnd().unwrap_or_default();
        let answers = answers.to_vec();
        self.0
            .executor
            .spawn(async move {
                let mut config = TASKDIALOGCONFIG::default();
                config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as _;
                config.hwndParent = handle;
                let title;
                let main_icon;
                match level {
                    crate::PromptLevel::Info => {
                        title = windows::core::w!("Info");
                        main_icon = TD_INFORMATION_ICON;
                    }
                    crate::PromptLevel::Warning => {
                        title = windows::core::w!("Warning");
                        main_icon = TD_WARNING_ICON;
                    }
                    crate::PromptLevel::Critical => {
                        title = windows::core::w!("Critical");
                        main_icon = TD_ERROR_ICON;
                    }
                };
                config.pszWindowTitle = title;
                config.Anonymous1.pszMainIcon = main_icon;
                let instruction = HSTRING::from(msg);
                config.pszMainInstruction = PCWSTR::from_raw(instruction.as_ptr());
                let hints_encoded;
                if let Some(ref hints) = detail_string {
                    hints_encoded = HSTRING::from(hints);
                    config.pszContent = PCWSTR::from_raw(hints_encoded.as_ptr());
                };
                let mut button_id_map = Vec::with_capacity(answers.len());
                let mut buttons = Vec::new();
                let mut btn_encoded = Vec::new();
                for (index, btn) in answers.iter().enumerate() {
                    let encoded = HSTRING::from(btn.label().as_ref());
                    let button_id = match btn {
                        PromptButton::Ok(_) => IDOK.0,
                        PromptButton::Cancel(_) => IDCANCEL.0,
                        // the first few low integer values are reserved for known buttons
                        // so for simplicity we just go backwards from -1
                        PromptButton::Other(_) => -(index as i32) - 1,
                    };
                    button_id_map.push(button_id);
                    buttons.push(TASKDIALOG_BUTTON {
                        nButtonID: button_id,
                        pszButtonText: PCWSTR::from_raw(encoded.as_ptr()),
                    });
                    btn_encoded.push(encoded);
                }
                config.cButtons = buttons.len() as _;
                config.pButtons = buttons.as_ptr();

                config.pfCallback = None;
                let fallback_content = detail_string
                    .as_deref()
                    .map(|detail| format!("{prompt_text}\n\n{detail}"))
                    .unwrap_or_else(|| prompt_text.clone());
                let res = show_task_dialog_or_message_box(&config, "Prompt", &fallback_content)
                    .unwrap_or_default();

                if let Some(clicked) = button_id_map.iter().position(|&button_id| button_id == res)
                {
                    let _ = done_tx.send(clicked);
                }
            })
            .detach();

        Some(done_rx)
    }

    fn activate(&self) {
        self.update_presentation_state(WindowsWindowPresentationState::request_activation);
        self.request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        });
    }

    fn is_active(&self) -> bool {
        self.0.window().has_focus()
    }

    fn is_hovered(&self) -> bool {
        self.0.state.borrow().hovered.get()
    }

    fn set_title(&mut self, title: &str) {
        self.window().set_title(title);
    }

    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        let transparent = background_appearance != WindowBackgroundAppearance::Opaque;
        self.window().set_transparent(transparent);
        self.0
            .state
            .borrow()
            .background_appearance
            .set(background_appearance);
        if let Some(hwnd) = self.native_hwnd() {
            apply_window_background_appearance(hwnd, background_appearance);
        }
        if let WindowsRendererState::Ready(renderer) = &mut *self.0.renderer.borrow_mut() {
            renderer.update_transparency(transparent);
        }
    }

    fn show(&self) {
        self.update_presentation_state(WindowsWindowPresentationState::request_show);
        self.request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        });
    }

    fn hide_window(&self) {
        self.update_presentation_state(WindowsWindowPresentationState::request_hide);
    }

    fn minimize(&self) {
        minimize_window(self.window());
    }

    fn maximize(&self) {
        maximize_window(self.window());
    }

    fn restore(&self) {
        restore_winit_window(self.window());
        self.restore_minimized_window();
        self.window().request_redraw();
    }

    fn zoom(&self) {
        toggle_window_maximized(self.window());
    }

    fn toggle_fullscreen(&self) {
        toggle_window_fullscreen(self.window());
    }

    fn is_fullscreen(&self) -> bool {
        self.window().fullscreen().is_some()
    }

    fn request_frame(&self, options: RequestFrameOptions) {
        WindowsWindow::request_frame(self, options);
    }

    fn frame_request_timed_out(&self, options: RequestFrameOptions) {
        self.clear_timed_out_frame_request(options);
    }

    fn start_window_move(&self) {
        if let Err(error) = start_winit_window_move(self.window()) {
            log::debug!("winit drag_window failed: {error}");
        }
    }

    fn start_window_resize(&self, edge: ResizeEdge) {
        if let Err(error) = start_winit_window_resize(self.window(), edge) {
            log::debug!("winit drag_resize_window failed: {error}");
        }
    }

    fn window_decorations(&self) -> Decorations {
        if self.0.use_native_decorations {
            Decorations::Server
        } else {
            Decorations::Client {
                tiling: Tiling::default(),
            }
        }
    }

    fn default_client_inset(&self) -> Option<Pixels> {
        (!self.0.use_native_decorations).then(Self::default_resize_inset)
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.state.borrow_mut().callbacks.request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.state.borrow_mut().callbacks.input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.state.borrow_mut().callbacks.active_status_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.state.borrow_mut().callbacks.hovered_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.state.borrow_mut().callbacks.resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.state.borrow_mut().callbacks.moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.state.borrow_mut().callbacks.should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.state.borrow_mut().callbacks.close = Some(callback);
    }

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.0.state.borrow_mut().callbacks.hit_test_window_control = Some(callback);
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.state.borrow_mut().callbacks.appearance_changed = Some(callback);
    }

    fn draw(&self, render_plan: FrameRenderPlan<'_>) {
        if !self.try_apply_queued_renderer_resize() {
            return;
        }
        let draw_result = {
            let mut renderer_state = self.0.renderer.borrow_mut();
            let WindowsRendererState::Ready(renderer) = &mut *renderer_state else {
                return;
            };
            renderer.draw(render_plan)
        };
        match draw_result {
            Ok(()) => self.mark_first_frame_presented(),
            Err(error) => log::error!("failed to draw Windows frame: {error:#}"),
        }
    }

    fn present_framebuffer_only(&self, render_plan: FrameRenderPlan<'_>) {
        if !self.try_apply_queued_renderer_resize() {
            return;
        }
        let present_result = {
            let mut renderer_state = self.0.renderer.borrow_mut();
            let WindowsRendererState::Ready(renderer) = &mut *renderer_state else {
                return;
            };
            renderer.present_framebuffer_only(render_plan)
        };
        match present_result {
            Ok(()) => self.mark_first_frame_presented(),
            Err(error) => log::error!("failed to present Windows framebuffer: {error:#}"),
        }
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.renderer_atlas.platform_atlas()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        let renderer = self.0.renderer.borrow();
        let WindowsRendererState::Ready(renderer) = &*renderer else {
            return None;
        };
        renderer.gpu_specs().log_err()
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {
        // There is no such thing on Windows.
    }

    fn map_window(&mut self) -> anyhow::Result<()> {
        self.update_presentation_state(WindowsWindowPresentationState::map);
        self.request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        });
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClickState {
    button: MouseButton,
    last_click: Instant,
    last_position: Point<DevicePixels>,
    double_click_spatial_tolerance_width: i32,
    double_click_spatial_tolerance_height: i32,
    double_click_interval: Duration,
    pub(crate) current_count: usize,
}

impl ClickState {
    pub fn new() -> Self {
        ClickState {
            button: MouseButton::Left,
            last_click: Instant::now(),
            last_position: Point::default(),
            double_click_spatial_tolerance_width: 6,
            double_click_spatial_tolerance_height: 6,
            double_click_interval: Duration::from_millis(500),
            current_count: 0,
        }
    }

    /// update self and return the needed click count
    pub fn update(&mut self, button: MouseButton, new_position: Point<DevicePixels>) -> usize {
        if self.button == button && self.is_double_click(new_position) {
            self.current_count += 1;
        } else {
            self.current_count = 1;
        }
        self.last_click = Instant::now();
        self.last_position = new_position;
        self.button = button;

        self.current_count
    }

    #[inline]
    fn is_double_click(&self, new_position: Point<DevicePixels>) -> bool {
        let diff = self.last_position - new_position;

        self.last_click.elapsed() < self.double_click_interval
            && diff.x.0.abs() <= self.double_click_spatial_tolerance_width
            && diff.y.0.abs() <= self.double_click_spatial_tolerance_height
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClickState, NativeWindowVisibilityAction, WindowsWindowPresentationState,
        clear_pending_frame_request_after_timeout, merge_frame_request,
        renderer_backend_candidates, should_use_native_decorations,
        should_use_no_redirection_bitmap,
    };
    use crate::{
        DevicePixels, MouseButton, RendererBackend, RendererOptions, RequestFrameOptions,
        TitlebarOptions, WindowBackgroundAppearance, WindowCornerPreference, WindowKind,
        WindowParams, point,
    };
    use std::time::Duration;

    #[test]
    fn test_double_click_interval() {
        let mut state = ClickState::new();
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            2
        );
        state.last_click -= Duration::from_millis(700);
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(0))),
            1
        );
    }

    #[test]
    fn test_double_click_spatial_tolerance() {
        let mut state = ClickState::new();
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(-3), DevicePixels(0))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Left, point(DevicePixels(0), DevicePixels(3))),
            2
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(3), DevicePixels(2))),
            1
        );
        assert_eq!(
            state.update(MouseButton::Right, point(DevicePixels(10), DevicePixels(0))),
            1
        );
    }

    #[test]
    fn transparent_titlebar_disables_native_decorations() {
        let params = WindowParams {
            bounds: Default::default(),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: None,
                transparent_caption_height: None,
            }),
            window_icon: None,
            kind: WindowKind::Normal,
            is_movable: true,
            is_resizable: true,
            is_minimizable: true,
            focus: true,
            show: true,
            display_id: None,
            window_background: WindowBackgroundAppearance::Transparent,
            window_min_size: None,
            window_corner_preference: WindowCornerPreference::SystemDefault,
        };

        assert!(!should_use_native_decorations(&params));
    }

    #[test]
    fn no_redirection_bitmap_is_enabled_for_dx12_transparent_candidates() {
        assert!(should_use_no_redirection_bitmap(
            false,
            true,
            &[RendererBackend::NovaDx12]
        ));
        assert!(!should_use_no_redirection_bitmap(
            true,
            true,
            &[RendererBackend::NovaDx12]
        ));
        assert!(!should_use_no_redirection_bitmap(
            false,
            false,
            &[RendererBackend::NovaDx12]
        ));
        assert!(!should_use_no_redirection_bitmap(
            false,
            true,
            &[RendererBackend::NovaVulkan]
        ));
        assert!(should_use_no_redirection_bitmap(
            false,
            true,
            &[RendererBackend::NovaDx12, RendererBackend::NovaVulkan]
        ));
        assert!(should_use_no_redirection_bitmap(
            false,
            true,
            &[RendererBackend::NovaVulkan, RendererBackend::NovaDx12]
        ));
    }

    #[test]
    fn explicit_dx12_opaque_renderer_candidates_do_not_fallback() {
        let options = RendererOptions::with_backend(RendererBackend::NovaDx12);

        assert_eq!(
            renderer_backend_candidates(&options, RendererBackend::NovaDx12, false),
            vec![RendererBackend::NovaDx12]
        );
    }

    #[test]
    fn explicit_dx12_transparent_renderer_candidates_try_vulkan_when_available() {
        let options = RendererOptions::with_backend(RendererBackend::NovaDx12);
        let candidates = renderer_backend_candidates(&options, RendererBackend::NovaDx12, true);

        assert_eq!(candidates.first().copied(), Some(RendererBackend::NovaDx12));
        #[cfg(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan"))]
        assert!(candidates.contains(&RendererBackend::NovaVulkan));
    }

    #[test]
    fn explicit_vulkan_transparent_renderer_candidates_try_dx12_when_available() {
        let options = RendererOptions::with_backend(RendererBackend::NovaVulkan);
        let candidates = renderer_backend_candidates(&options, RendererBackend::NovaVulkan, true);

        assert_eq!(
            candidates.first().copied(),
            Some(RendererBackend::NovaVulkan)
        );
        assert!(candidates.contains(&RendererBackend::NovaDx12));
    }

    #[test]
    fn auto_renderer_candidates_try_vulkan_after_dx12_when_available() {
        let options = RendererOptions::with_backend(RendererBackend::Auto);
        let candidates = renderer_backend_candidates(&options, RendererBackend::NovaDx12, true);

        assert_eq!(candidates.first().copied(), Some(RendererBackend::NovaDx12));
        #[cfg(any(feature = "nova-gfx-vulkan", feature = "windows-vulkan"))]
        assert!(candidates.contains(&RendererBackend::NovaVulkan));
    }

    #[test]
    fn default_pending_request_is_empty() {
        assert_eq!(
            RequestFrameOptions::default(),
            RequestFrameOptions::default()
        );
    }

    #[test]
    fn pending_request_merges_force_render_and_presentation() {
        let first = RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        };
        let second = RequestFrameOptions {
            require_presentation: false,
            force_render: true,
        };

        let (merged, should_schedule_frame) = merge_frame_request(first, second);

        assert_eq!(
            merged,
            RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            }
        );
        assert!(!should_schedule_frame);
    }

    #[test]
    fn resize_refresh_request_forces_render_when_presentation_is_pending() {
        let pending = RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        };
        let resize_refresh = RequestFrameOptions::from_refresh();

        let (merged, should_schedule_frame) = merge_frame_request(pending, resize_refresh);

        assert_eq!(
            merged,
            RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            }
        );
        assert!(!should_schedule_frame);
    }

    #[test]
    fn first_pending_request_requests_redraw() {
        let (merged, should_request_redraw) = merge_frame_request(
            RequestFrameOptions::default(),
            RequestFrameOptions::from_refresh(),
        );

        assert_eq!(merged, RequestFrameOptions::from_refresh());
        assert!(should_request_redraw);
    }

    #[test]
    fn timed_out_request_clears_merged_pending_request() {
        let timed_out = RequestFrameOptions::from_refresh();
        let pending = timed_out.merge(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        });

        assert_eq!(
            clear_pending_frame_request_after_timeout(pending),
            RequestFrameOptions::default()
        );
    }

    #[test]
    fn mapped_window_waits_for_first_presented_frame() {
        let mut state = WindowsWindowPresentationState::new(true, true);

        assert_eq!(state.map(), NativeWindowVisibilityAction::None);
        assert!(!state.native_visible);
    }

    #[test]
    fn first_presented_frame_reveals_and_focuses_requested_window() {
        let mut state = WindowsWindowPresentationState::new(true, true);
        assert_eq!(state.map(), NativeWindowVisibilityAction::None);

        assert_eq!(
            state.first_frame_presented(),
            NativeWindowVisibilityAction::Show { focus: true }
        );
        assert!(state.native_visible);
        assert!(!state.focus_requested);
    }

    #[test]
    fn hidden_window_stays_hidden_after_first_presented_frame() {
        let mut state = WindowsWindowPresentationState::new(false, false);
        assert_eq!(state.map(), NativeWindowVisibilityAction::None);

        assert_eq!(
            state.first_frame_presented(),
            NativeWindowVisibilityAction::None
        );
        assert!(!state.native_visible);
    }

    #[test]
    fn pre_rendered_hidden_window_reveals_immediately_when_shown() {
        let mut state = WindowsWindowPresentationState::new(false, false);
        assert_eq!(state.map(), NativeWindowVisibilityAction::None);
        assert_eq!(
            state.first_frame_presented(),
            NativeWindowVisibilityAction::None
        );

        assert_eq!(
            state.request_show(),
            NativeWindowVisibilityAction::Show { focus: false }
        );
    }

    #[test]
    fn activation_before_first_frame_defers_focus_until_reveal() {
        let mut state = WindowsWindowPresentationState::new(false, false);
        assert_eq!(state.map(), NativeWindowVisibilityAction::None);
        assert_eq!(
            state.request_activation(),
            NativeWindowVisibilityAction::None
        );

        assert_eq!(
            state.first_frame_presented(),
            NativeWindowVisibilityAction::Show { focus: true }
        );
    }
}

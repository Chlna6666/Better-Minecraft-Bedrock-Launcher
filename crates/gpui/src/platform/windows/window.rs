#![deny(unsafe_op_in_unsafe_fn)]
#![expect(
    unsafe_code,
    reason = "the native window boundary calls Win32 APIs with HWND-owned state"
)]

use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{HashMap, HashSet},
    ffi::c_void,
    rc::{Rc, Weak},
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
        Graphics::{
            Dwm::{
                DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL,
                DwmSetWindowAttribute,
            },
            Gdi::{
                CreateRoundRectRgn, DeleteObject, HGDIOBJ, RDW_INVALIDATE, RDW_NOERASE,
                RDW_UPDATENOW, RedrawWindow, SetWindowRgn,
            },
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::{
            Controls::*,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                HICON, ICON_BIG, ICON_SMALL, IDCANCEL, IDOK, IMAGE_ICON, IsIconic, IsZoomed,
                KillTimer, LR_DEFAULTSIZE, LR_SHARED, LoadImageW, SW_RESTORE, SendMessageW,
                SetForegroundWindow, SetTimer, ShowWindow, USER_TIMER_MINIMUM, WM_ENTERSIZEMOVE,
                WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SETICON, WM_SIZE, WM_TIMER,
                WM_WINDOWPOSCHANGED,
            },
        },
    },
    core::*,
};

use crate::diagnostics::performance_metrics::{
    record_frame_request, record_gpu_adapter_diagnostics, record_renderer_backend,
    record_window_request_redraw,
};
use crate::platform::windows::with_dll_library;
use crate::platform::winit::{
    begin_windows_native_size_move, end_windows_native_size_move, maximize_window, minimize_window,
    request_window_inner_size, restore_window as restore_winit_window,
    start_window_move as start_winit_window_move, start_window_resize as start_winit_window_resize,
    toggle_window_fullscreen, toggle_window_maximized,
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

const SIZE_MOVE_LOOP_SUBCLASS_ID: usize = 0x4750_5549;
const SIZE_MOVE_LOOP_TIMER_ID: usize = 0x4750_5549;

thread_local! {
    static WINDOWS_IN_SIZE_MOVE_LOOP: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
    static WINDOWS_BY_HWND: RefCell<HashMap<isize, Weak<WindowsWindowInner>>> =
        RefCell::new(HashMap::new());
}

fn register_native_window(hwnd: HWND, window: &WindowsWindow) {
    WINDOWS_BY_HWND.with(|windows| {
        windows
            .borrow_mut()
            .insert(hwnd.0 as isize, Rc::downgrade(&window.0));
    });
}

fn unregister_native_window(hwnd: HWND) {
    WINDOWS_BY_HWND.with(|windows| {
        windows.borrow_mut().remove(&(hwnd.0 as isize));
    });
}

fn native_window(hwnd: HWND) -> Option<WindowsWindow> {
    WINDOWS_BY_HWND.with(|windows| {
        let mut windows = windows.borrow_mut();
        let window = windows
            .get(&(hwnd.0 as isize))
            .and_then(Weak::upgrade)
            .map(WindowsWindow);
        if window.is_none() {
            windows.remove(&(hwnd.0 as isize));
        }
        window
    })
}

fn enter_size_move_loop(hwnd: HWND) {
    let entered =
        WINDOWS_IN_SIZE_MOVE_LOOP.with(|windows| windows.borrow_mut().insert(hwnd.0 as isize));
    if entered {
        begin_windows_native_size_move();
    }
}

fn leave_size_move_loop(hwnd: HWND) {
    let left =
        WINDOWS_IN_SIZE_MOVE_LOOP.with(|windows| windows.borrow_mut().remove(&(hwnd.0 as isize)));
    if left {
        end_windows_native_size_move();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SizeMoveLoopAction {
    Start,
    Tick,
    Finish,
    SyncExtent,
    SuppressErase,
    Destroy,
    Forward,
}

fn size_move_loop_action(message: u32, timer_id: usize) -> SizeMoveLoopAction {
    match message {
        WM_ENTERSIZEMOVE => SizeMoveLoopAction::Start,
        WM_TIMER if timer_id == SIZE_MOVE_LOOP_TIMER_ID => SizeMoveLoopAction::Tick,
        WM_EXITSIZEMOVE => SizeMoveLoopAction::Finish,
        WM_SIZE | WM_WINDOWPOSCHANGED => SizeMoveLoopAction::SyncExtent,
        WM_ERASEBKGND => SizeMoveLoopAction::SuppressErase,
        WM_NCDESTROY => SizeMoveLoopAction::Destroy,
        _ => SizeMoveLoopAction::Forward,
    }
}

fn redraw_size_move_frame(hwnd: HWND) {
    // `RDW_UPDATENOW` sends WM_PAINT while Win32 is inside its modal size/move loop. Winit then
    // emits `RedrawRequested`, which consumes one latest-wins resize generation and presents it.
    if !unsafe {
        RedrawWindow(
            Some(hwnd),
            None,
            None,
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_NOERASE,
        )
    }
    .as_bool()
    {
        log::warn!("failed to redraw GPUI window from the native size/move loop");
    }
}

fn dispatch_size_move_frame(hwnd: HWND) {
    let Some(window) = native_window(hwnd) else {
        redraw_size_move_frame(hwnd);
        return;
    };

    // Mature Win32 loops such as SDL drive their live-resize update directly from this timer.
    // Going through WM_PAINT alone is insufficient because winit may buffer RedrawRequested while
    // its event-loop runner is already borrowed by another window event.
    window.sync_current_native_size();
    window.dispatch_pending_update();
}

unsafe extern "system" fn size_move_loop_subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    match size_move_loop_action(message, wparam.0) {
        SizeMoveLoopAction::Start => {
            // Track the actual Win32 modal loop as well as GPUI-initiated drag calls. Native
            // decorated windows can enter this path without calling winit's drag helpers.
            enter_size_move_loop(hwnd);
            let timer = unsafe {
                SetTimer(
                    Some(hwnd),
                    SIZE_MOVE_LOOP_TIMER_ID,
                    USER_TIMER_MINIMUM,
                    None,
                )
            };
            if timer == 0 {
                log::warn!("failed to start GPUI native size/move redraw timer");
            }
        }
        SizeMoveLoopAction::Tick => {
            dispatch_size_move_frame(hwnd);
            return windows::Win32::Foundation::LRESULT(0);
        }
        SizeMoveLoopAction::Finish => {
            if let Err(error) = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_LOOP_TIMER_ID) } {
                log::warn!("failed to stop GPUI native size/move redraw timer: {error}");
            }
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            // Resume normal frame pacing before flushing the final coalesced extent. Otherwise
            // the final request is made while the global native-size/move guard still suspends
            // VSync, leaving the client at its new size with an older frame covering only part of
            // it until some unrelated event requests another frame.
            leave_size_move_loop(hwnd);
            dispatch_size_move_frame(hwnd);
            return result;
        }
        SizeMoveLoopAction::SyncExtent => {
            // Let the default/winit chain commit the non-client and client rectangles first, then
            // publish the authoritative client extent. Do not invoke GPUI callbacks from this
            // native stack: maximize/restore can deliver WM_SIZE synchronously while the click
            // handler still owns the high-level Window RefCell. `queue_resize` requests a frame,
            // and the winit/VSync pump consumes it after the native callback unwinds.
            let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
            if let Some(window) = native_window(hwnd) {
                window.sync_current_native_size();
            }
            return result;
        }
        // The GPU surface owns the complete client area. Letting DefWindowProc erase an enlarged
        // update region exposes the class background brush before the next swapchain present.
        SizeMoveLoopAction::SuppressErase => {
            return windows::Win32::Foundation::LRESULT(1);
        }
        SizeMoveLoopAction::Destroy => {
            unregister_native_window(hwnd);
            leave_size_move_loop(hwnd);
            let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_LOOP_TIMER_ID) };
            if !unsafe {
                RemoveWindowSubclass(
                    hwnd,
                    Some(size_move_loop_subclass_proc),
                    SIZE_MOVE_LOOP_SUBCLASS_ID,
                )
            }
            .as_bool()
            {
                log::debug!("GPUI native size/move window subclass was already removed");
            }
        }
        SizeMoveLoopAction::Forward => {}
    }

    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn install_size_move_loop_subclass(hwnd: HWND) {
    if !unsafe {
        SetWindowSubclass(
            hwnd,
            Some(size_move_loop_subclass_proc),
            SIZE_MOVE_LOOP_SUBCLASS_ID,
            0,
        )
    }
    .as_bool()
    {
        log::warn!("failed to install GPUI native size/move window subclass");
    }
}

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
    resolved_backend: RendererBackend,
) -> bool {
    !disable_direct_composition
        && transparent_background
        && resolved_backend == RendererBackend::NovaDx12
}

fn renderer_backend_candidates(
    renderer_options: &RendererOptions,
    resolved_backend: RendererBackend,
    transparent: bool,
) -> Vec<RendererBackend> {
    let mut candidates = vec![resolved_backend];
    let should_try_fallbacks = renderer_options.adapter_name.is_none()
        && (renderer_options.backend == RendererBackend::Auto
            || (transparent
                && matches!(
                    renderer_options.backend,
                    RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
                )));
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

fn fallback_corner_radius(hwnd: HWND, preference: WindowCornerPreference) -> Option<Pixels> {
    let (dwm_preference, radius) = match preference {
        WindowCornerPreference::SystemDefault => return None,
        WindowCornerPreference::Rounded => (DWMWCP_ROUND, Some(px(8.0))),
        WindowCornerPreference::RoundedSmall => (DWMWCP_ROUNDSMALL, Some(px(4.0))),
        WindowCornerPreference::Square => (DWMWCP_DONOTROUND, None),
    };
    // Windows 11 accepts this DWM attribute. Windows 10 returns an unsupported-attribute error,
    // in which case a window region supplies the same visible corner without an alpha surface.
    let applied = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &raw const dwm_preference as *const c_void,
            std::mem::size_of_val(&dwm_preference) as u32,
        )
    }
    .is_ok();
    (!applied).then_some(radius).flatten()
}

fn apply_fallback_corner_region(
    hwnd: HWND,
    size: Size<DevicePixels>,
    scale_factor: f32,
    radius: Pixels,
    maximized: bool,
) {
    if maximized {
        // SAFETY: The HWND is live and a null region restores the full rectangular window.
        unsafe { SetWindowRgn(hwnd, None, true) };
        return;
    }
    let width = size.width.0.max(1);
    let height = size.height.0.max(1);
    let diameter = (radius.0 * scale_factor * 2.0).round().max(1.0) as i32;
    // The right and bottom edges are exclusive. Including one extra device pixel keeps the last
    // row and column inside the region while retaining the requested corner radius.
    let region = unsafe { CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter) };
    if region.is_invalid() {
        log::warn!("failed to create Windows fallback rounded-corner region");
        return;
    }
    // On success Windows owns the region handle. Delete it only if ownership was not transferred.
    if unsafe { SetWindowRgn(hwnd, Some(region), true) } == 0
        && !unsafe { DeleteObject(HGDIOBJ(region.0)) }.as_bool()
    {
        log::warn!("failed to dispose an unused Windows rounded-corner region");
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
        record_window_request_redraw(self.0.handle.window_id().data().as_ffi());
        if !self.0.vsync_scheduler.request_frame() {
            self.window().request_redraw();
        }
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
            match NovaRenderer::with_atlas(
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
                    record_gpu_adapter_diagnostics(&gpu_specs.device_name, &gpu_specs.driver_name);
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

    pub fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        match self {
            Self::Nova(renderer) => renderer.try_resize(size),
        }
    }

    /// Stretches the previous frame over the new client size while a resize is pending.
    pub(crate) fn stretch_for_pending_resize(&mut self, size: Size<DevicePixels>) {
        match self {
            Self::Nova(renderer) => renderer.stretch_surface_for_pending_resize(size),
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

    pub fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        match self {
            Self::Nova(renderer) => renderer.trim_gpui_memory(level),
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
    pending_resize: Cell<Option<PendingWindowsResize>>,
    frame_dispatch_in_progress: Cell<bool>,
    vsync_scheduler: Arc<super::vsync::VSyncScheduler>,
    pub(crate) pending_renderer_size: Cell<Option<Size<DevicePixels>>>,
    pub(crate) renderer_resize_retry_pending: Cell<bool>,
    fallback_corner_radius: Option<Pixels>,
    pub(crate) winit_window: OnceCell<Arc<WinitWindow>>,
}

#[derive(Clone, Copy)]
pub(crate) struct PendingWindowsResize {
    pub(crate) logical_size: Size<Pixels>,
    pub(crate) drawable_size: Size<DevicePixels>,
    pub(crate) scale_factor: f32,
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
            vsync_scheduler,
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
                renderer_backend,
            ));
        if !use_native_decorations {
            // Do not enable winit's undecorated-shadow workaround here. On Windows it shifts the
            // client rectangle by one physical pixel in WM_NCCALCSIZE; the resulting DWM/client
            // mismatch softens the whole surface and makes live resize visibly lag behind it.
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
            install_size_move_loop_subclass(hwnd);
            apply_window_background_appearance(hwnd, params.window_background);
        }
        let scale_factor = winit_window.scale_factor() as f32;
        let actual_inner_size = winit_window.inner_size();
        let actual_logical_size = Size {
            width: Pixels(actual_inner_size.width as f32 / scale_factor),
            height: Pixels(actual_inner_size.height as f32 / scale_factor),
        };
        let fallback_corner_radius = if use_native_decorations {
            None
        } else {
            hwnd.and_then(|hwnd| fallback_corner_radius(hwnd, params.window_corner_preference))
        };
        if let (Some(hwnd), Some(radius)) = (hwnd, fallback_corner_radius) {
            apply_fallback_corner_region(
                hwnd,
                Size {
                    width: DevicePixels(actual_inner_size.width as i32),
                    height: DevicePixels(actual_inner_size.height as i32),
                },
                scale_factor,
                radius,
                winit_window.is_maximized(),
            );
        }
        if params.window_icon.is_none()
            && let Some(hwnd) = hwnd
        {
            Self::apply_process_default_window_icon(hwnd);
        }
        if !use_native_decorations {
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
            pending_resize: Cell::new(None),
            frame_dispatch_in_progress: Cell::new(false),
            vsync_scheduler,
            pending_renderer_size: Cell::new(None),
            renderer_resize_retry_pending: Cell::new(false),
            fallback_corner_radius,
            winit_window: cell,
        }));
        if let Some(hwnd) = hwnd {
            register_native_window(hwnd, &window);
        }
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

    pub(crate) fn queue_resize(&self, resize: PendingWindowsResize) {
        // Redraw and idle dispatch consume this slot, so an event burst keeps only its latest size
        // without imposing a separate timer cadence on interactive resizing.
        self.0.pending_resize.set(Some(resize));
        if let (Some(hwnd), Some(radius)) = (self.native_hwnd(), self.0.fallback_corner_radius) {
            apply_fallback_corner_region(
                hwnd,
                resize.drawable_size,
                resize.scale_factor,
                radius,
                self.native_is_maximized().unwrap_or(false),
            );
        }
        // Compositor-backed swapchains do not scale on their own, so stretch the previous
        // frame over the new client size right now, before the next frame applies the
        // resize to the swapchain buffers. This mirrors the scaling DXGI performs for
        // HWND flip swapchains and keeps the live-resize gap transparent instead of black.
        if let Ok(mut renderer_state) = self.0.renderer.try_borrow_mut() {
            if let WindowsRendererState::Ready(renderer) = &mut *renderer_state {
                renderer.stretch_for_pending_resize(resize.drawable_size);
            }
        }
        // Do not rely on WM_PAINT to wake a DirectComposition/no-redirection window.
        self.request_frame(RequestFrameOptions::from_refresh());
    }

    pub(crate) fn sync_size(
        &self,
        physical_size: winit::dpi::PhysicalSize<u32>,
        scale_factor: f32,
    ) {
        if physical_size.width == 0 || physical_size.height == 0 {
            return;
        }

        let logical_size = Size {
            width: Pixels(physical_size.width as f32 / scale_factor),
            height: Pixels(physical_size.height as f32 / scale_factor),
        };
        let Ok(state) = self.try_borrow_state() else {
            log::warn!("window state is already borrowed while synchronizing Windows size");
            return;
        };
        if state.logical_size.get() == logical_size && state.scale_factor.get() == scale_factor {
            return;
        }
        state.logical_size.set(logical_size);
        state.scale_factor.set(scale_factor);
        drop(state);

        self.queue_resize(PendingWindowsResize {
            logical_size,
            drawable_size: Size {
                width: DevicePixels(physical_size.width as i32),
                height: DevicePixels(physical_size.height as i32),
            },
            scale_factor,
        });
    }

    fn sync_current_native_size(&self) {
        let physical_size = self.window().inner_size();
        let scale_factor = self.window().scale_factor() as f32;
        self.sync_size(physical_size, scale_factor);
    }

    pub(crate) fn is_in_native_size_move_loop(&self) -> bool {
        let Some(hwnd) = self.native_hwnd() else {
            return false;
        };
        WINDOWS_IN_SIZE_MOVE_LOOP.with(|windows| windows.borrow().contains(&(hwnd.0 as isize)))
    }

    pub(crate) fn dispatch_pending_update(&self) {
        // A frame callback may synchronously pump another native message. Leave any newly queued
        // resize/frame request in its latest-wins slot for the next timer or redraw instead of
        // taking it while the callback is temporarily removed from `Callbacks`.
        if self.0.frame_dispatch_in_progress.replace(true) {
            return;
        }
        self.dispatch_pending_resize();
        let options = self.take_pending_frame_request();
        if options.requires_frame() {
            self.invoke_request_frame(options);
        }
        self.0.frame_dispatch_in_progress.set(false);
    }

    pub(crate) fn dispatch_pending_resize(&self) {
        let Some(resize) = self.0.pending_resize.take() else {
            return;
        };
        self.0.pending_renderer_size.set(Some(resize.drawable_size));
        self.0.renderer_resize_retry_pending.set(false);
        self.invoke_resize(resize.logical_size, resize.scale_factor);
        self.request_frame(RequestFrameOptions::from_refresh());
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
        match renderer.try_resize(size) {
            Ok(true) => {}
            Ok(false) => {
                self.0.pending_renderer_size.set(Some(size));
                if !self.0.renderer_resize_retry_pending.replace(true) {
                    self.request_frame(RequestFrameOptions::from_refresh());
                }
                return false;
            }
            Err(error) => {
                log::warn!("failed to resize Windows renderer: {error:#}");
                self.0.pending_renderer_size.set(Some(size));
                if !self.0.renderer_resize_retry_pending.replace(true) {
                    self.request_frame(RequestFrameOptions::from_refresh());
                }
                return false;
            }
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

    fn set_window_origin(&mut self, origin: Point<Pixels>) -> bool {
        let scale_factor = f64::from(self.0.state.borrow().scale_factor.get());
        self.window()
            .set_outer_position(winit::dpi::PhysicalPosition::new(
                f64::from(origin.x.0) * scale_factor,
                f64::from(origin.y.0) * scale_factor,
            ));
        true
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

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.0.state.borrow().background_appearance.get()
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

    fn draw(&self, render_plan: FrameRenderPlan<'_>) -> PlatformFrameResult {
        if !self.try_apply_queued_renderer_resize() {
            return PlatformFrameResult::Deferred;
        }
        let draw_result = {
            let mut renderer_state = self.0.renderer.borrow_mut();
            let WindowsRendererState::Ready(renderer) = &mut *renderer_state else {
                return PlatformFrameResult::Deferred;
            };
            renderer.draw(render_plan)
        };
        match draw_result {
            Ok(()) => {
                self.mark_first_frame_presented();
                PlatformFrameResult::Submitted
            }
            Err(error) => {
                log::error!("failed to draw Windows frame: {error:#}");
                self.request_frame(RequestFrameOptions::from_refresh());
                PlatformFrameResult::Deferred
            }
        }
    }

    fn present_framebuffer_only(&self, render_plan: FrameRenderPlan<'_>) -> PlatformFrameResult {
        if !self.try_apply_queued_renderer_resize() {
            return PlatformFrameResult::Deferred;
        }
        let present_result = {
            let mut renderer_state = self.0.renderer.borrow_mut();
            let WindowsRendererState::Ready(renderer) = &mut *renderer_state else {
                return PlatformFrameResult::Deferred;
            };
            renderer.present_framebuffer_only(render_plan)
        };
        match present_result {
            Ok(()) => {
                self.mark_first_frame_presented();
                PlatformFrameResult::Submitted
            }
            Err(error) => {
                log::error!("failed to present Windows framebuffer: {error:#}");
                self.request_frame(RequestFrameOptions::from_refresh());
                PlatformFrameResult::Deferred
            }
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

    fn trim_gpui_memory(&self, level: GpuiMemoryTrimLevel) {
        let mut renderer = self.0.renderer.borrow_mut();
        let WindowsRendererState::Ready(renderer) = &mut *renderer else {
            return;
        };
        renderer.trim_gpui_memory(level);
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
        ClickState, NativeWindowVisibilityAction, SIZE_MOVE_LOOP_TIMER_ID, SizeMoveLoopAction,
        WindowsWindowPresentationState, clear_pending_frame_request_after_timeout,
        merge_frame_request, renderer_backend_candidates, should_use_native_decorations,
        should_use_no_redirection_bitmap, size_move_loop_action,
    };
    use crate::{
        DevicePixels, MouseButton, RendererBackend, RendererOptions, RequestFrameOptions,
        TitlebarOptions, WindowBackgroundAppearance, WindowCornerPreference, WindowKind,
        WindowParams, point,
    };
    use std::time::Duration;
    use windows::Win32::UI::WindowsAndMessaging::{
        WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_NCDESTROY, WM_SIZE, WM_TIMER,
        WM_WINDOWPOSCHANGED,
    };

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
    fn no_redirection_bitmap_is_enabled_only_for_resolved_dx12_transparent_windows() {
        assert!(should_use_no_redirection_bitmap(
            false,
            true,
            RendererBackend::NovaDx12,
        ));
        assert!(!should_use_no_redirection_bitmap(
            true,
            true,
            RendererBackend::NovaDx12,
        ));
        assert!(!should_use_no_redirection_bitmap(
            false,
            false,
            RendererBackend::NovaDx12,
        ));
        assert!(!should_use_no_redirection_bitmap(
            false,
            true,
            RendererBackend::NovaVulkan,
        ));
    }

    #[test]
    fn native_size_move_messages_drive_one_timer_and_a_final_flush() {
        assert_eq!(
            size_move_loop_action(WM_ENTERSIZEMOVE, 0),
            SizeMoveLoopAction::Start
        );
        assert_eq!(
            size_move_loop_action(WM_TIMER, SIZE_MOVE_LOOP_TIMER_ID),
            SizeMoveLoopAction::Tick
        );
        assert_eq!(
            size_move_loop_action(WM_TIMER, SIZE_MOVE_LOOP_TIMER_ID + 1),
            SizeMoveLoopAction::Forward
        );
        assert_eq!(
            size_move_loop_action(WM_EXITSIZEMOVE, 0),
            SizeMoveLoopAction::Finish
        );
        assert_eq!(
            size_move_loop_action(WM_SIZE, 0),
            SizeMoveLoopAction::SyncExtent
        );
        assert_eq!(
            size_move_loop_action(WM_WINDOWPOSCHANGED, 0),
            SizeMoveLoopAction::SyncExtent
        );
        assert_eq!(
            size_move_loop_action(WM_ERASEBKGND, 0),
            SizeMoveLoopAction::SuppressErase
        );
        assert_eq!(
            size_move_loop_action(WM_NCDESTROY, 0),
            SizeMoveLoopAction::Destroy
        );
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
    fn explicit_adapter_does_not_fallback_to_another_backend() {
        let options = RendererOptions {
            adapter_name: Some("AMD Radeon 780M".to_string()),
            ..RendererOptions::with_backend(RendererBackend::NovaVulkan)
        };

        assert_eq!(
            renderer_backend_candidates(&options, RendererBackend::NovaVulkan, true),
            vec![RendererBackend::NovaVulkan]
        );
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

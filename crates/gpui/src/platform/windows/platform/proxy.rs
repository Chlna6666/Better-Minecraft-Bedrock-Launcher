use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use parking_lot::Mutex;
use winit::{event_loop::EventLoopProxy, raw_window_handle as rwh, window::Window as WinitWindow};

use super::{
    WindowsNativeCommand, WindowsNativeEvent, WindowsNativeWindow, WindowsPlatformInner,
    WindowsUserEvent, WindowsWindow, WindowsWindowAction,
};
use crate::{
    ActivePresentationFrame, AnyWindowHandle, Bounds, Capslock, DispatchEventResult,
    ForegroundExecutor, GpuSpecs, GpuiMemoryTrimLevel, Modifiers, Pixels, PlatformAtlas,
    PlatformFrameRequest, PlatformFrameRequestSender, PlatformFrameResult, PlatformInput,
    PlatformInputHandler, PlatformWindow, Point, PromptButton, PromptLevel, ResizeEdge,
    SceneAnimationCompletionSender, Size, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowVisibility,
};

pub(crate) struct QueuedScene {
    pub(crate) packet: crate::PresentationPacket,
    pub(crate) framebuffer_only: bool,
}

#[derive(Default)]
pub(crate) struct SceneMailbox {
    pending: Option<QueuedScene>,
    wake_queued: bool,
}

impl SceneMailbox {
    fn queue(&mut self, mut packet: crate::PresentationPacket, framebuffer_only: bool) -> bool {
        if let Some(previous) = self.pending.take() {
            packet.merge_pending_damage_from(&previous.packet);
        }
        self.pending = Some(QueuedScene {
            packet,
            framebuffer_only,
        });
        if self.wake_queued {
            false
        } else {
            self.wake_queued = true;
            true
        }
    }

    pub(crate) fn take(&mut self) -> Option<QueuedScene> {
        let pending = self.pending.take();
        self.wake_queued = false;
        pending
    }

    fn cancel(&mut self) {
        self.pending = None;
        self.wake_queued = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BackdropBlurDamagePlan, DirtyRegion, PartialPresentMode, Point, ScaledPixels, Scene,
        bounds, size,
    };

    fn packet(scene: Arc<Scene>, x: f32) -> crate::PresentationPacket {
        let mut dirty_region = DirtyRegion::empty();
        dirty_region.push(bounds(
            Point {
                x: ScaledPixels(x),
                y: ScaledPixels(0.0),
            },
            size(ScaledPixels(10.0), ScaledPixels(10.0)),
        ));
        crate::PresentationPacket::new(
            scene,
            [],
            [],
            Instant::now(),
            1.0,
            dirty_region,
            BackdropBlurDamagePlan::default(),
            PartialPresentMode::FullRedraw,
        )
    }

    #[test]
    fn mailbox_keeps_latest_scene_and_accumulates_unsubmitted_damage() {
        let first_scene = Arc::new(Scene::default());
        let latest_scene = Arc::new(Scene::default());
        let mut mailbox = SceneMailbox::default();

        assert!(mailbox.queue(packet(first_scene, 0.0), false));
        assert!(!mailbox.queue(packet(latest_scene.clone(), 20.0), true));
        let queued = mailbox.take().expect("mailbox contains the latest scene");

        assert!(Arc::ptr_eq(&queued.packet.scene, &latest_scene));
        assert!(queued.framebuffer_only);
        assert_eq!(
            queued.packet.dirty_region.union_bounds(),
            Some(bounds(
                Point {
                    x: ScaledPixels(0.0),
                    y: ScaledPixels(0.0),
                },
                size(ScaledPixels(30.0), ScaledPixels(10.0)),
            )),
        );
        assert!(mailbox.queue(packet(Arc::new(Scene::default()), 40.0), false));
    }
}

#[derive(Default)]
struct Callbacks {
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active: Option<Box<dyn FnMut(bool)>>,
    visibility: Option<Box<dyn FnMut(WindowVisibility)>>,
    hovered: Option<Box<dyn FnMut(bool)>>,
    resized: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    closed: Option<Box<dyn FnOnce()>>,
    hit_test: Option<Box<dyn FnMut() -> Option<WindowControlArea>>>,
    appearance: Option<Box<dyn FnMut()>>,
}

#[derive(Clone)]
pub(super) struct WindowsWindowProxy {
    id: winit::window::WindowId,
    handle: AnyWindowHandle,
    platform: Weak<WindowsPlatformInner>,
    window: Arc<WinitWindow>,
    atlas: Arc<dyn PlatformAtlas>,
    snapshot: Rc<RefCell<WindowsNativeWindow>>,
    callbacks: Rc<RefCell<Callbacks>>,
    input_handler: Rc<RefCell<Option<PlatformInputHandler>>>,
    event_loop: EventLoopProxy<WindowsUserEvent>,
    pending_scene: Arc<Mutex<SceneMailbox>>,
    first_frame_presented: Rc<Cell<bool>>,
}

impl WindowsWindowProxy {
    pub(super) fn new(
        snapshot: WindowsNativeWindow,
        handle: AnyWindowHandle,
        platform: Weak<WindowsPlatformInner>,
        event_loop: EventLoopProxy<WindowsUserEvent>,
        executor: ForegroundExecutor,
    ) -> Self {
        let (sender, receiver) = flume::unbounded();
        let proxy = Self {
            id: snapshot.window_id,
            handle,
            platform,
            window: snapshot.window.clone(),
            atlas: snapshot.atlas.clone(),
            snapshot: Rc::new(RefCell::new(snapshot)),
            callbacks: Rc::default(),
            input_handler: Rc::default(),
            event_loop,
            pending_scene: Arc::default(),
            first_frame_presented: Rc::new(Cell::new(false)),
        };
        proxy.send(WindowsNativeCommand::SetEventSender {
            window_id: proxy.id,
            sender,
        });
        let events = proxy.clone();
        executor
            .spawn(async move {
                while let Ok(event) = receiver.recv_async().await {
                    events.dispatch_event(event);
                }
            })
            .detach();
        proxy
    }

    fn send(&self, command: WindowsNativeCommand) -> bool {
        let Some(platform) = self.platform.upgrade() else {
            return false;
        };
        if platform.native_owner_closing.load(Ordering::Acquire) {
            return false;
        }
        if let Err(error) = self
            .event_loop
            .send_event(WindowsUserEvent::NativeCommand(command))
        {
            if platform.native_owner_closing.load(Ordering::Acquire) {
                return false;
            }
            platform.native_owner_closing.store(true, Ordering::Release);
            log::error!("failed to send Windows native owner command: {error:?}");
            return false;
        }
        true
    }

    fn action(&self, action: WindowsWindowAction) {
        self.send(WindowsNativeCommand::WindowAction {
            window_id: self.id,
            action,
        });
    }

    fn call<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut WindowsWindow) -> R + Send + 'static,
    ) -> Option<R> {
        let (reply, receiver) = std::sync::mpsc::sync_channel(1);
        if !self.send(WindowsNativeCommand::WindowCall {
            window_id: self.id,
            call: Box::new(move |window| {
                let _ = reply.send(window.map(operation));
            }),
        }) {
            return None;
        }
        receiver.recv().ok().flatten()
    }

    fn queue_scene(
        &self,
        packet: crate::PresentationPacket,
        framebuffer_only: bool,
    ) -> PlatformFrameResult {
        let needs_wakeup = self.pending_scene.lock().queue(packet, framebuffer_only);
        if needs_wakeup
            && !self.send(WindowsNativeCommand::CommitLatestScene {
                window_id: self.id,
                mailbox: self.pending_scene.clone(),
            })
        {
            self.pending_scene.lock().cancel();
            return PlatformFrameResult::Deferred;
        }
        PlatformFrameResult::Queued
    }

    fn dispatch_event(&self, event: WindowsNativeEvent) {
        match event {
            WindowsNativeEvent::Input(input) => {
                {
                    let mut snapshot = self.snapshot.borrow_mut();
                    match &input {
                        PlatformInput::MouseDown(event) => {
                            snapshot.mouse_position = event.position;
                            snapshot.modifiers = event.modifiers;
                        }
                        PlatformInput::MouseUp(event) => {
                            snapshot.mouse_position = event.position;
                            snapshot.modifiers = event.modifiers;
                        }
                        PlatformInput::MouseMove(event) => {
                            snapshot.mouse_position = event.position;
                            snapshot.modifiers = event.modifiers;
                        }
                        PlatformInput::ScrollWheel(event) => {
                            snapshot.mouse_position = event.position;
                            snapshot.modifiers = event.modifiers;
                        }
                        PlatformInput::ModifiersChanged(event) => {
                            snapshot.modifiers = event.modifiers;
                            snapshot.capslock = event.capslock;
                        }
                        _ => {}
                    }
                }
                let callback = self.callbacks.borrow_mut().input.take();
                if let Some(mut callback) = callback {
                    callback(input);
                    self.callbacks.borrow_mut().input = Some(callback);
                }
            }
            WindowsNativeEvent::Active(active) => {
                self.snapshot.borrow_mut().active = active;
                if let Some(platform) = self.platform.upgrade() {
                    let mut state = platform.state.borrow_mut();
                    if active {
                        state.active_window_handle = Some(self.handle);
                    } else if state.active_window_handle == Some(self.handle) {
                        state.active_window_handle = None;
                    }
                }
                let callback = self.callbacks.borrow_mut().active.take();
                if let Some(mut callback) = callback {
                    callback(active);
                    self.callbacks.borrow_mut().active = Some(callback);
                }
            }
            WindowsNativeEvent::Visibility(visibility) => {
                self.snapshot.borrow_mut().visibility = visibility;
                let callback = self.callbacks.borrow_mut().visibility.take();
                if let Some(mut callback) = callback {
                    callback(visibility);
                    self.callbacks.borrow_mut().visibility = Some(callback);
                }
            }
            WindowsNativeEvent::Hovered(hovered) => {
                self.snapshot.borrow_mut().hovered = hovered;
                let callback = self.callbacks.borrow_mut().hovered.take();
                if let Some(mut callback) = callback {
                    callback(hovered);
                    self.callbacks.borrow_mut().hovered = Some(callback);
                }
            }
            WindowsNativeEvent::Resized(size, scale) => {
                let window_state = self.call(|window| {
                    (
                        window.bounds(),
                        window.window_bounds(),
                        window.is_maximized(),
                        window.is_minimized(),
                        window.is_fullscreen(),
                    )
                });
                {
                    let mut snapshot = self.snapshot.borrow_mut();
                    snapshot.content_size = size;
                    snapshot.scale_factor = scale;
                    if let Some((bounds, window_bounds, maximized, minimized, fullscreen)) =
                        window_state
                    {
                        snapshot.bounds = bounds;
                        snapshot.window_bounds = window_bounds;
                        snapshot.maximized = maximized;
                        snapshot.minimized = minimized;
                        snapshot.fullscreen = fullscreen;
                    }
                }
                let callback = self.callbacks.borrow_mut().resized.take();
                if let Some(mut callback) = callback {
                    callback(size, scale);
                    self.callbacks.borrow_mut().resized = Some(callback);
                }
            }
            WindowsNativeEvent::Moved => {
                if let Some((bounds, window_bounds)) =
                    self.call(|window| (window.bounds(), window.window_bounds()))
                {
                    let mut snapshot = self.snapshot.borrow_mut();
                    snapshot.bounds = bounds;
                    snapshot.window_bounds = window_bounds;
                }
                let callback = self.callbacks.borrow_mut().moved.take();
                if let Some(mut callback) = callback {
                    callback();
                    self.callbacks.borrow_mut().moved = Some(callback);
                }
            }
            WindowsNativeEvent::AppearanceChanged => {
                if let Some(appearance) = self.call(|window| window.appearance()) {
                    self.snapshot.borrow_mut().appearance = appearance;
                }
                let callback = self.callbacks.borrow_mut().appearance.take();
                if let Some(mut callback) = callback {
                    callback();
                    self.callbacks.borrow_mut().appearance = Some(callback);
                }
            }
            WindowsNativeEvent::CloseRequested => {
                let mut callback = self.callbacks.borrow_mut().should_close.take();
                let should_close = callback.as_mut().is_none_or(|callback| callback());
                self.callbacks.borrow_mut().should_close = callback;
                if should_close {
                    self.send(WindowsNativeCommand::CloseWindow { window_id: self.id });
                }
            }
            WindowsNativeEvent::Closed => {
                if let Some(platform) = self.platform.upgrade() {
                    let mut state = platform.state.borrow_mut();
                    if state.active_window_handle == Some(self.handle) {
                        state.active_window_handle = None;
                    }
                }
                let callback = self.callbacks.borrow_mut().closed.take();
                if let Some(callback) = callback {
                    callback();
                }
            }
        }
    }
}

impl rwh::HasWindowHandle for WindowsWindowProxy {
    fn window_handle(&self) -> std::result::Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        self.window.window_handle()
    }
}

impl rwh::HasDisplayHandle for WindowsWindowProxy {
    fn display_handle(&self) -> std::result::Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Ok(rwh::DisplayHandle::windows())
    }
}

impl PlatformWindow for WindowsWindowProxy {
    fn bounds(&self) -> Bounds<Pixels> {
        self.snapshot.borrow().bounds
    }
    fn is_maximized(&self) -> bool {
        self.snapshot.borrow().maximized
    }
    fn is_minimized(&self) -> bool {
        self.snapshot.borrow().minimized
    }
    fn visibility(&self) -> WindowVisibility {
        self.snapshot.borrow().visibility
    }
    fn window_bounds(&self) -> WindowBounds {
        self.snapshot.borrow().window_bounds
    }
    fn content_size(&self) -> Size<Pixels> {
        self.snapshot.borrow().content_size
    }
    fn resize(&mut self, size: Size<Pixels>) {
        self.action(WindowsWindowAction::Resize(size));
    }
    fn set_window_origin(&mut self, origin: Point<Pixels>) -> bool {
        self.call(move |window| window.set_window_origin(origin))
            .unwrap_or(false)
    }
    fn scale_factor(&self) -> f32 {
        self.snapshot.borrow().scale_factor
    }
    fn appearance(&self) -> WindowAppearance {
        self.snapshot.borrow().appearance
    }
    fn display(&self) -> Option<Rc<dyn crate::PlatformDisplay>> {
        self.snapshot
            .borrow()
            .display
            .clone()
            .map(|display| Rc::new(display) as Rc<dyn crate::PlatformDisplay>)
    }
    fn mouse_position(&self) -> Point<Pixels> {
        self.snapshot.borrow().mouse_position
    }
    fn modifiers(&self) -> Modifiers {
        self.snapshot.borrow().modifiers
    }
    fn capslock(&self) -> Capslock {
        self.snapshot.borrow().capslock
    }
    fn set_input_handler(&mut self, handler: PlatformInputHandler) {
        *self.input_handler.borrow_mut() = Some(handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.input_handler.borrow_mut().take()
    }
    fn prompt(
        &self,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        let message = message.to_owned();
        let detail = detail.map(str::to_owned);
        let answers = answers.to_vec();
        self.call(move |window| window.prompt(level, &message, detail.as_deref(), &answers))
            .flatten()
    }
    fn activate(&self) {
        self.action(WindowsWindowAction::Activate);
    }
    fn is_active(&self) -> bool {
        self.snapshot.borrow().active
    }
    fn is_hovered(&self) -> bool {
        self.snapshot.borrow().hovered
    }
    fn set_title(&mut self, title: &str) {
        self.action(WindowsWindowAction::SetTitle(title.to_owned()));
    }
    fn set_background_appearance(&self, appearance: WindowBackgroundAppearance) {
        self.snapshot.borrow_mut().background_appearance = appearance;
        self.action(WindowsWindowAction::SetBackgroundAppearance(appearance));
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.snapshot.borrow().background_appearance
    }
    fn background_capabilities(&self) -> crate::WindowBackgroundCapabilities {
        self.call(|window| window.background_capabilities())
            .unwrap_or_default()
    }
    fn effective_background_appearance(&self) -> WindowBackgroundAppearance {
        self.call(|window| window.effective_background_appearance())
            .unwrap_or(crate::WindowBackgroundAppearance::Transparent)
    }
    fn show(&self) {
        self.action(WindowsWindowAction::Show);
    }
    fn hide_window(&self) {
        self.action(WindowsWindowAction::Hide);
    }
    fn minimize(&self) {
        self.action(WindowsWindowAction::Minimize);
    }
    fn maximize(&self) {
        self.action(WindowsWindowAction::Maximize);
    }
    fn restore(&self) {
        self.action(WindowsWindowAction::Restore);
    }
    fn zoom(&self) {
        self.action(WindowsWindowAction::Zoom);
    }
    fn start_window_move(&self) {
        self.action(WindowsWindowAction::StartMove);
    }
    fn start_window_resize(&self, edge: ResizeEdge) {
        self.action(WindowsWindowAction::StartResize(edge));
    }
    fn toggle_fullscreen(&self) {
        self.action(WindowsWindowAction::ToggleFullscreen);
    }
    fn is_fullscreen(&self) -> bool {
        self.snapshot.borrow().fullscreen
    }
    fn request_frame(&self, request: PlatformFrameRequest) {
        self.action(WindowsWindowAction::RequestFrame(request));
    }
    fn set_frame_interval(&self, interval: Option<std::time::Duration>) {
        self.send(WindowsNativeCommand::WindowCall {
            window_id: self.id,
            call: Box::new(move |window| {
                if let Some(window) = window {
                    window.set_frame_interval(interval);
                }
            }),
        });
    }
    fn frame_request_timed_out(&self, request: PlatformFrameRequest) {
        self.action(WindowsWindowAction::FrameRequestTimedOut(request));
    }
    fn set_frame_request_sender(&self, sender: PlatformFrameRequestSender) {
        self.send(WindowsNativeCommand::SetFrameRequestSender {
            window_id: self.id,
            sender,
        });
    }
    fn set_presentation_animation_completion_sender(&self, sender: SceneAnimationCompletionSender) {
        self.send(WindowsNativeCommand::SetAnimationCompletionSender {
            window_id: self.id,
            sender,
        });
    }
    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.callbacks.borrow_mut().input = Some(callback);
    }
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.callbacks.borrow_mut().active = Some(callback);
        let active = self.snapshot.borrow().active;
        self.dispatch_event(WindowsNativeEvent::Active(active));
    }
    fn on_visibility_change(&self, callback: Box<dyn FnMut(WindowVisibility)>) {
        self.callbacks.borrow_mut().visibility = Some(callback);
    }
    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.callbacks.borrow_mut().hovered = Some(callback);
    }
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.callbacks.borrow_mut().resized = Some(callback);
    }
    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().moved = Some(callback);
    }
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.callbacks.borrow_mut().should_close = Some(callback);
    }
    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.callbacks.borrow_mut().closed = Some(callback);
    }
    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.callbacks.borrow_mut().hit_test = Some(callback);
    }
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().appearance = Some(callback);
    }
    fn draw(&self, packet: crate::PresentationPacket) -> PlatformFrameResult {
        if self.first_frame_presented.get() {
            return self.queue_scene(packet, false);
        }

        let (reply, receiver) = std::sync::mpsc::channel();
        if !self.send(WindowsNativeCommand::CommitScene {
            window_id: self.id,
            packet,
            reply,
        }) {
            return PlatformFrameResult::Deferred;
        }
        let result = receiver.recv().unwrap_or(PlatformFrameResult::Deferred);
        if result == PlatformFrameResult::Submitted {
            self.first_frame_presented.set(true);
        }
        result
    }
    fn present_framebuffer_only(
        &self,
        mut packet: crate::PresentationPacket,
    ) -> PlatformFrameResult {
        if self.first_frame_presented.get() {
            return self.queue_scene(packet, true);
        }

        self.call(move |window| {
            packet.frame_time = Instant::now();
            window.present_framebuffer_only(packet)
        })
        .inspect(|result| {
            if *result == PlatformFrameResult::Submitted {
                self.first_frame_presented.set(true);
            }
        })
        .unwrap_or(PlatformFrameResult::Deferred)
    }
    fn present_active_frame(
        &self,
        now: Instant,
        timing: Option<crate::platform::frame::ActivePresentationTiming>,
    ) -> Result<Option<ActivePresentationFrame>> {
        self.call(move |window| window.present_active_frame(now, timing))
            .ok_or_else(|| anyhow!("native Windows window is closed"))?
    }
    fn has_active_presentation_animations(&self) -> bool {
        self.call(|window| window.has_active_presentation_animations())
            .unwrap_or(false)
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.call(|window| window.gpu_specs()).flatten()
    }
    fn trim_gpui_memory(&self, level: GpuiMemoryTrimLevel) {
        self.send(WindowsNativeCommand::WindowCall {
            window_id: self.id,
            call: Box::new(move |window| {
                if let Some(window) = window {
                    window.trim_gpui_memory(level);
                }
            }),
        });
    }
    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
    fn window_decorations(&self) -> crate::window::Decorations {
        self.snapshot.borrow().decorations
    }
    fn default_client_inset(&self) -> Option<Pixels> {
        self.snapshot.borrow().default_client_inset
    }
    fn map_window(&mut self) -> Result<()> {
        self.call(|window| window.map_window())
            .ok_or_else(|| anyhow!("native Windows window is closed"))?
    }
}

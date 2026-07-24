use crate::{
    AnyWindowHandle, AtlasKey, AtlasTextureId, AtlasTile, Bounds, DispatchEventResult, GpuSpecs,
    Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow,
    Point, PromptButton, RequestFrameOptions, Size, TestPlatform, TileId, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowParams,
};
use collections::HashMap;
use parking_lot::Mutex;
use std::{
    cell::Cell,
    rc::{Rc, Weak},
    sync::{self, Arc},
    thread,
    time::Duration,
};
use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};

pub(crate) struct TestWindowState {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) handle: AnyWindowHandle,
    display: Rc<dyn PlatformDisplay>,
    pub(crate) title: Option<String>,
    pub(crate) edited: bool,
    pub(crate) shown: bool,
    pub(crate) focus_on_map: bool,
    platform: Weak<TestPlatform>,
    sprite_atlas: Arc<dyn PlatformAtlas>,
    pub(crate) should_close_handler: Option<Box<dyn FnMut() -> bool>>,
    hit_test_window_control_callback: Option<Box<dyn FnMut() -> Option<WindowControlArea>>>,
    input_callback: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change_callback: Option<Box<dyn FnMut(bool)>>,
    hover_status_change_callback: Option<Box<dyn FnMut(bool)>>,
    resize_callback: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved_callback: Option<Box<dyn FnMut()>>,
    request_frame_callback: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    request_frame_count: Rc<Cell<usize>>,
    last_requested_frame: Rc<Cell<Option<RequestFrameOptions>>>,
    start_window_move_count: Rc<Cell<usize>>,
    present_framebuffer_only_count: Rc<Cell<usize>>,
    draw_delay: Duration,
    input_handler: Option<PlatformInputHandler>,
    is_maximized: bool,
    is_fullscreen: bool,
}

#[derive(Clone)]
pub(crate) struct TestWindow(pub(crate) Rc<Mutex<TestWindowState>>);

impl HasWindowHandle for TestWindow {
    fn window_handle(
        &self,
    ) -> Result<winit::raw_window_handle::WindowHandle<'_>, winit::raw_window_handle::HandleError>
    {
        unimplemented!("Test Windows are not backed by a real platform window")
    }
}

impl HasDisplayHandle for TestWindow {
    fn display_handle(
        &self,
    ) -> Result<winit::raw_window_handle::DisplayHandle<'_>, winit::raw_window_handle::HandleError>
    {
        unimplemented!("Test Windows are not backed by a real platform window")
    }
}

impl TestWindow {
    pub fn new(
        handle: AnyWindowHandle,
        params: WindowParams,
        platform: Weak<TestPlatform>,
        display: Rc<dyn PlatformDisplay>,
    ) -> Self {
        Self(Rc::new(Mutex::new(TestWindowState {
            bounds: params.bounds,
            display,
            platform,
            handle,
            sprite_atlas: Arc::new(TestAtlas::new()),
            title: Default::default(),
            edited: false,
            shown: false,
            focus_on_map: params.focus,
            should_close_handler: None,
            hit_test_window_control_callback: None,
            input_callback: None,
            active_status_change_callback: None,
            hover_status_change_callback: None,
            resize_callback: None,
            moved_callback: None,
            request_frame_callback: None,
            request_frame_count: Rc::new(Cell::new(0)),
            last_requested_frame: Rc::new(Cell::new(None)),
            start_window_move_count: Rc::new(Cell::new(0)),
            present_framebuffer_only_count: Rc::new(Cell::new(0)),
            draw_delay: Duration::ZERO,
            input_handler: None,
            is_maximized: false,
            is_fullscreen: false,
        })))
    }

    pub fn simulate_resize(&mut self, size: Size<Pixels>) {
        let scale_factor = self.scale_factor();
        let mut lock = self.0.lock();
        let Some(mut callback) = lock.resize_callback.take() else {
            return;
        };
        lock.bounds.size = size;
        drop(lock);
        callback(size, scale_factor);
        self.0.lock().resize_callback = Some(callback);
    }

    pub(crate) fn simulate_active_status_change(&self, is_active: bool) {
        let mut lock = self.0.lock();
        let Some(mut callback) = lock.active_status_change_callback.take() else {
            return;
        };
        drop(lock);
        callback(is_active);
        self.0.lock().active_status_change_callback = Some(callback);
    }

    pub fn simulate_input(&mut self, event: PlatformInput) -> bool {
        let mut lock = self.0.lock();
        let Some(mut callback) = lock.input_callback.take() else {
            return false;
        };
        drop(lock);
        let result = callback(event);
        self.0.lock().input_callback = Some(callback);
        !result.propagate
    }

    pub(crate) fn requested_frame_count(&self) -> usize {
        self.0.lock().request_frame_count.get()
    }

    pub(crate) fn last_requested_frame(&self) -> Option<RequestFrameOptions> {
        self.0.lock().last_requested_frame.get()
    }

    pub(crate) fn start_window_move_count(&self) -> usize {
        self.0.lock().start_window_move_count.get()
    }

    pub(crate) fn simulate_request_frame(&self, options: RequestFrameOptions) {
        let mut lock = self.0.lock();
        let Some(mut callback) = lock.request_frame_callback.take() else {
            return;
        };
        drop(lock);
        callback(options);
        self.0.lock().request_frame_callback = Some(callback);
    }

    pub(crate) fn present_framebuffer_only_count(&self) -> usize {
        self.0.lock().present_framebuffer_only_count.get()
    }

    pub(crate) fn set_draw_delay(&self, delay: Duration) {
        self.0.lock().draw_delay = delay;
    }

    pub(crate) fn is_shown(&self) -> bool {
        self.0.lock().shown
    }

    pub(crate) fn focus_on_map(&self) -> bool {
        self.0.lock().focus_on_map
    }
}

impl PlatformWindow for TestWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.lock().bounds
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn is_maximized(&self) -> bool {
        self.0.lock().is_maximized
    }

    fn is_minimized(&self) -> bool {
        !self.0.lock().shown
    }

    fn content_size(&self) -> Size<Pixels> {
        self.bounds().size
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let mut lock = self.0.lock();
        lock.bounds.size = size;
    }

    fn scale_factor(&self) -> f32 {
        2.0
    }

    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }

    fn display(&self) -> Option<std::rc::Rc<dyn crate::PlatformDisplay>> {
        Some(self.0.lock().display.clone())
    }

    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }

    fn modifiers(&self) -> crate::Modifiers {
        crate::Modifiers::default()
    }

    fn capslock(&self) -> crate::Capslock {
        crate::Capslock::default()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.lock().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.lock().input_handler.take()
    }

    fn prompt(
        &self,
        _level: crate::PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        Some(
            self.0
                .lock()
                .platform
                .upgrade()
                .expect("platform dropped")
                .prompt(msg, detail, answers),
        )
    }

    fn activate(&self) {
        self.0
            .lock()
            .platform
            .upgrade()
            .unwrap()
            .set_active_window(Some(self.clone()))
    }

    fn is_active(&self) -> bool {
        self.0
            .lock()
            .platform
            .upgrade()
            .and_then(|platform| platform.active_window.borrow().clone())
            .is_some_and(|window| Rc::ptr_eq(&window.0, &self.0))
    }

    fn is_hovered(&self) -> bool {
        false
    }

    fn set_title(&mut self, title: &str) {
        self.0.lock().title = Some(title.to_owned());
    }

    fn set_app_id(&mut self, _app_id: &str) {}

    fn set_background_appearance(&self, _background: WindowBackgroundAppearance) {}

    fn set_edited(&mut self, is_edited: bool) {
        self.0.lock().edited = is_edited;
    }

    fn show_character_palette(&self) {
        unimplemented!()
    }

    fn minimize(&self) {
        let mut lock = self.0.lock();
        lock.shown = false;
    }

    fn maximize(&self) {
        let mut lock = self.0.lock();
        lock.is_maximized = true;
    }

    fn restore(&self) {
        let mut lock = self.0.lock();
        lock.is_maximized = false;
        lock.is_fullscreen = false;
    }

    fn zoom(&self) {
        let mut lock = self.0.lock();
        lock.is_maximized = !lock.is_maximized;
    }

    fn toggle_fullscreen(&self) {
        let mut lock = self.0.lock();
        lock.is_fullscreen = !lock.is_fullscreen;
    }

    fn is_fullscreen(&self) -> bool {
        self.0.lock().is_fullscreen
    }

    fn request_frame(&self, options: RequestFrameOptions) {
        let mut lock = self.0.lock();
        lock.request_frame_count
            .set(lock.request_frame_count.get() + 1);
        lock.last_requested_frame.set(Some(options));
    }

    fn frame_request_timed_out(&self, _options: RequestFrameOptions) {}

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.lock().request_frame_callback = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(crate::PlatformInput) -> DispatchEventResult>) {
        self.0.lock().input_callback = Some(callback)
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.lock().active_status_change_callback = Some(callback)
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.lock().hover_status_change_callback = Some(callback)
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.lock().resize_callback = Some(callback)
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.lock().moved_callback = Some(callback)
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.lock().should_close_handler = Some(callback);
    }

    fn on_close(&self, _callback: Box<dyn FnOnce()>) {}

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.0.lock().hit_test_window_control_callback = Some(callback);
    }

    fn on_appearance_changed(&self, _callback: Box<dyn FnMut()>) {}

    fn draw(&self, _render_plan: crate::FrameRenderPlan<'_>) {
        let draw_delay = self.0.lock().draw_delay;
        if !draw_delay.is_zero() {
            thread::sleep(draw_delay);
        }
    }

    fn present_framebuffer_only(&self, _render_plan: crate::FrameRenderPlan<'_>) {
        let lock = self.0.lock();
        lock.present_framebuffer_only_count
            .set(lock.present_framebuffer_only_count.get() + 1);
    }

    fn sprite_atlas(&self) -> sync::Arc<dyn crate::PlatformAtlas> {
        self.0.lock().sprite_atlas.clone()
    }

    fn as_test(&mut self) -> Option<&mut TestWindow> {
        Some(self)
    }

    fn show_window_menu(&self, _position: Point<Pixels>) {
        unimplemented!()
    }

    fn start_window_move(&self) {
        let lock = self.0.lock();
        lock.start_window_move_count
            .set(lock.start_window_move_count.get() + 1);
    }

    fn map_window(&mut self) -> anyhow::Result<()> {
        let mut lock = self.0.lock();
        lock.shown = true;
        let focus_on_map = lock.focus_on_map;
        lock.focus_on_map = false;
        drop(lock);
        if focus_on_map {
            self.activate();
        }
        Ok(())
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
}

pub(crate) struct TestAtlasState {
    next_id: u32,
    tiles: HashMap<AtlasKey, AtlasTile>,
}

pub(crate) struct TestAtlas(Mutex<TestAtlasState>);

impl TestAtlas {
    pub fn new() -> Self {
        TestAtlas(Mutex::new(TestAtlasState {
            next_id: 0,
            tiles: HashMap::default(),
        }))
    }
}

impl PlatformAtlas for TestAtlas {
    fn ensure_tile_with<'a>(
        &self,
        key: &crate::AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<
            Option<(Size<crate::DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<crate::AtlasTile>> {
        let mut state = self.0.lock();
        if let Some(tile) = state.tiles.get(key) {
            return Ok(Some(tile.clone()));
        }
        drop(state);

        let Some((size, _)) = build()? else {
            return Ok(None);
        };

        let mut state = self.0.lock();
        state.next_id += 1;
        let texture_id = state.next_id;
        state.next_id += 1;
        let tile_id = state.next_id;

        state.tiles.insert(
            key.clone(),
            crate::AtlasTile {
                texture_id: AtlasTextureId {
                    index: texture_id,
                    kind: key.texture_kind(),
                },
                tile_id: TileId(tile_id),
                padding: 0,
                bounds: crate::Bounds {
                    origin: Point::default(),
                    size,
                },
            },
        );

        Ok(Some(state.tiles[key].clone()))
    }

    fn refresh_tile_with<'a>(
        &self,
        key: &crate::AtlasKey,
        build: &mut dyn FnMut() -> anyhow::Result<
            Option<(Size<crate::DevicePixels>, std::borrow::Cow<'a, [u8]>)>,
        >,
    ) -> anyhow::Result<Option<crate::AtlasTile>> {
        let Some((size, _)) = build()? else {
            return Ok(None);
        };
        let mut state = self.0.lock();
        if let Some(tile) = state.tiles.get_mut(key)
            && tile.bounds.size == size
        {
            return Ok(Some(tile.clone()));
        }
        drop(state);
        self.remove(key);
        self.ensure_tile_with(key, &mut || {
            Ok(Some((size, std::borrow::Cow::Borrowed(&[]))))
        })
    }

    fn clear_glyphs(&self) {
        let mut state = self.0.lock();
        state
            .tiles
            .retain(|key, _| !matches!(key, AtlasKey::Glyph(_)));
    }

    fn remove(&self, key: &AtlasKey) {
        let mut state = self.0.lock();
        state.tiles.remove(key);
    }
}

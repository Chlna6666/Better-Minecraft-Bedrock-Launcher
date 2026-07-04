use std::{
    any::{Any, TypeId},
    cell::RefCell,
    mem,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, anyhow};
use futures::{Future, FutureExt, future::LocalBoxFuture};
use parking_lot::RwLock;
use slotmap::SlotMap;

use super::{AppCell, KeystrokeObserver};
use ::util::debug_panic;
use collections::{FxHashMap, FxHashSet, VecDeque};
use http_client::HttpClient;

#[cfg(any(feature = "inspector", debug_assertions))]
use crate::InspectorElementRegistry;
use crate::{
    ActionRegistry, AnyDrag, AnyEntity, AnyView, AnyWindowHandle, AssetSource, AsyncApp,
    BackgroundExecutor, Context, DefaultFontConfig, DispatchPhase, Effect, Entity, EntityId,
    EntityMap, EventEmitter, FocusMap, FontSource, ForegroundExecutor, ImagePipelineConfig, Keymap,
    LayoutId, Platform, PlatformKeyboardLayout, PlatformKeyboardMapper, PromptBuilder, Render,
    SubscriberSet, Subscription, SvgRenderer, SystemWindowTabController, Task, TextStyle,
    TextSystem, Window, WindowHandle, WindowId, WindowInvalidator,
    colors::{Colors, GlobalColors},
    init_app_menus, record_coalesced_refresh_effect,
};

mod observers;
mod window;

/// The duration for which futures returned from [Context::on_app_quit] can run before the application fully quits.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);

type Handler = Box<dyn FnMut(&mut App) -> bool + 'static>;
type Listener = Box<dyn FnMut(&dyn Any, &mut App) -> bool + 'static>;
type QuitHandler = Box<dyn FnOnce(&mut App) -> LocalBoxFuture<'static, ()> + 'static>;
type WindowClosedHandler = Box<dyn FnMut(&mut App)>;
type ReleaseListener = Box<dyn FnOnce(&mut dyn Any, &mut App) + 'static>;
type NewEntityListener = Box<dyn FnMut(AnyEntity, &mut Option<&mut Window>, &mut App) + 'static>;

/// Contains the state of the full application, and passed as a reference to a variety of callbacks.
/// Other [Context] derefs to this type.
/// You need a reference to an `App` to access the state of a [Entity].
pub struct App {
    pub(crate) this: Weak<AppCell>,
    pub(crate) platform: Rc<dyn Platform>,
    pub(in crate::app) text_system: Arc<TextSystem>,
    pub(crate) default_text_style: TextStyle,
    pub(crate) default_window_icon: Option<crate::WindowIconSource>,
    pub(crate) image_pipeline_config: ImagePipelineConfig,
    flushing_effects: bool,
    pending_updates: usize,
    pub(in crate::app) pending_refresh_windows: bool,
    pub(crate) actions: Rc<ActionRegistry>,
    pub(crate) active_drag: Option<AnyDrag>,
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) loading_assets: FxHashMap<(TypeId, u64), Box<dyn Any>>,
    pub(in crate::app) asset_source: Arc<dyn AssetSource>,
    pub(crate) svg_renderer: SvgRenderer,
    pub(in crate::app) http_client: Arc<dyn HttpClient>,
    pub(crate) globals_by_type: FxHashMap<TypeId, Box<dyn Any>>,
    pub(crate) entities: EntityMap,
    pub(crate) window_update_stack: Vec<WindowId>,
    pub(crate) new_entity_observers: SubscriberSet<TypeId, NewEntityListener>,
    pub(crate) windows: SlotMap<WindowId, Option<Box<Window>>>,
    pub(crate) window_handles: FxHashMap<WindowId, AnyWindowHandle>,
    pub(crate) focus_handles: Arc<FocusMap>,
    pub(crate) keymap: Rc<RefCell<Keymap>>,
    pub(crate) keyboard_layout: Box<dyn PlatformKeyboardLayout>,
    pub(crate) keyboard_mapper: Rc<dyn PlatformKeyboardMapper>,
    pub(crate) global_action_listeners:
        FxHashMap<TypeId, Vec<Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Self)>>>,
    pub(in crate::app) pending_effects: VecDeque<Effect>,
    pub(crate) pending_notifications: FxHashSet<EntityId>,
    pub(crate) pending_global_notifications: FxHashSet<TypeId>,
    pub(in crate::app) notifying_global_observers: FxHashSet<TypeId>,
    pub(in crate::app) global_notification_counts: FxHashMap<TypeId, usize>,
    pub(crate) observers: SubscriberSet<EntityId, Handler>,
    // TypeId is the type of the event that the listener callback expects
    pub(crate) event_listeners: SubscriberSet<EntityId, (TypeId, Listener)>,
    pub(crate) keystroke_observers: SubscriberSet<(), KeystrokeObserver>,
    pub(crate) keystroke_interceptors: SubscriberSet<(), KeystrokeObserver>,
    pub(crate) keyboard_layout_observers: SubscriberSet<(), Handler>,
    pub(crate) release_listeners: SubscriberSet<EntityId, ReleaseListener>,
    pub(crate) global_observers: SubscriberSet<TypeId, Handler>,
    pub(crate) quit_observers: SubscriberSet<(), QuitHandler>,
    pub(crate) restart_observers: SubscriberSet<(), Handler>,
    pub(crate) restart_path: Option<PathBuf>,
    pub(crate) window_closed_observers: SubscriberSet<(), WindowClosedHandler>,
    pub(crate) layout_id_buffer: Vec<LayoutId>, // We recycle this memory across layout requests.
    pub(crate) propagate_event: bool,
    pub(crate) prompt_builder: Option<PromptBuilder>,
    pub(crate) window_invalidators_by_entity:
        FxHashMap<EntityId, FxHashMap<WindowId, WindowInvalidator>>,
    pub(crate) tracked_entities: FxHashMap<WindowId, FxHashSet<EntityId>>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) inspector_renderer: Option<crate::InspectorRenderer>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) inspector_element_registry: InspectorElementRegistry,
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub(crate) name: Option<&'static str>,
    quitting: bool,
}

impl App {
    #[allow(clippy::new_ret_no_self)]
    pub(crate) fn new_app(
        platform: Rc<dyn Platform>,
        asset_source: Arc<dyn AssetSource>,
        http_client: Arc<dyn HttpClient>,
    ) -> Rc<AppCell> {
        let executor = platform.background_executor();
        let foreground_executor = platform.foreground_executor();
        assert!(
            executor.is_main_thread(),
            "must construct App on main thread"
        );

        let text_system = Arc::new(TextSystem::new(platform.text_system()));
        let entities = EntityMap::new();
        let keyboard_layout = platform.keyboard_layout();
        let keyboard_mapper = platform.keyboard_mapper();

        let app = Rc::new_cyclic(|this| AppCell {
            app: RefCell::new(App {
                this: this.clone(),
                platform: platform.clone(),
                text_system,
                default_text_style: TextStyle::default(),
                default_window_icon: None,
                image_pipeline_config: ImagePipelineConfig::default(),
                actions: Rc::new(ActionRegistry::default()),
                flushing_effects: false,
                pending_updates: 0,
                pending_refresh_windows: false,
                active_drag: None,
                background_executor: executor,
                foreground_executor,
                svg_renderer: SvgRenderer::new(asset_source.clone()),
                loading_assets: Default::default(),
                asset_source,
                http_client,
                globals_by_type: FxHashMap::default(),
                entities,
                new_entity_observers: SubscriberSet::new(),
                windows: SlotMap::with_key(),
                window_update_stack: Vec::new(),
                window_handles: FxHashMap::default(),
                focus_handles: Arc::new(RwLock::new(SlotMap::with_key())),
                keymap: Rc::new(RefCell::new(Keymap::default())),
                keyboard_layout,
                keyboard_mapper,
                global_action_listeners: FxHashMap::default(),
                pending_effects: VecDeque::new(),
                pending_notifications: FxHashSet::default(),
                pending_global_notifications: FxHashSet::default(),
                notifying_global_observers: FxHashSet::default(),
                global_notification_counts: FxHashMap::default(),
                observers: SubscriberSet::new(),
                tracked_entities: FxHashMap::default(),
                window_invalidators_by_entity: FxHashMap::default(),
                event_listeners: SubscriberSet::new(),
                release_listeners: SubscriberSet::new(),
                keystroke_observers: SubscriberSet::new(),
                keystroke_interceptors: SubscriberSet::new(),
                keyboard_layout_observers: SubscriberSet::new(),
                global_observers: SubscriberSet::new(),
                quit_observers: SubscriberSet::new(),
                restart_observers: SubscriberSet::new(),
                restart_path: None,
                window_closed_observers: SubscriberSet::new(),
                layout_id_buffer: Default::default(),
                propagate_event: true,
                prompt_builder: Some(PromptBuilder::Default),
                #[cfg(any(feature = "inspector", debug_assertions))]
                inspector_renderer: None,
                #[cfg(any(feature = "inspector", debug_assertions))]
                inspector_element_registry: InspectorElementRegistry::default(),
                quitting: false,

                #[cfg(any(test, feature = "test-support", debug_assertions))]
                name: None,
            }),
        });

        init_app_menus(platform.as_ref(), &app.borrow());
        SystemWindowTabController::init(&mut app.borrow_mut());

        platform.on_keyboard_layout_change(Box::new({
            let app = Rc::downgrade(&app);
            move || {
                if let Some(app) = app.upgrade() {
                    let cx = &mut app.borrow_mut();
                    cx.keyboard_layout = cx.platform.keyboard_layout();
                    cx.keyboard_mapper = cx.platform.keyboard_mapper();
                    cx.keyboard_layout_observers
                        .clone()
                        .retain(&(), move |callback| (callback)(cx));
                }
            }
        }));

        platform.on_quit(Box::new({
            let cx = app.clone();
            move || {
                cx.borrow_mut().shutdown();
            }
        }));

        app
    }

    /// Quit the application gracefully. Handlers registered with [`Context::on_app_quit`]
    /// will be given 100ms to complete before exiting.
    pub fn shutdown(&mut self) {
        let mut futures = Vec::new();

        for observer in self.quit_observers.remove(&()) {
            futures.push(observer(self));
        }

        self.windows.clear();
        self.window_handles.clear();
        self.flush_effects();
        self.quitting = true;

        let futures = futures::future::join_all(futures);
        if self
            .background_executor
            .block_with_timeout(SHUTDOWN_TIMEOUT, futures)
            .is_err()
        {
            log::error!("timed out waiting on app_will_quit");
        }

        self.quitting = false;
    }

    /// Get the id of the current keyboard layout
    pub fn keyboard_layout(&self) -> &dyn PlatformKeyboardLayout {
        self.keyboard_layout.as_ref()
    }

    /// Get the current keyboard mapper.
    pub fn keyboard_mapper(&self) -> &Rc<dyn PlatformKeyboardMapper> {
        &self.keyboard_mapper
    }

    /// Invokes a handler when the current keyboard layout changes
    pub fn on_keyboard_layout_change<F>(&self, mut callback: F) -> Subscription
    where
        F: 'static + FnMut(&mut App),
    {
        let (subscription, activate) = self.keyboard_layout_observers.insert(
            (),
            Box::new(move |cx| {
                callback(cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Gracefully quit the application via the platform's standard routine.
    pub fn quit(&self) {
        self.platform.quit();
    }

    /// Schedules all windows in the application to be redrawn. This can be called
    /// multiple times in an update cycle and still result in a single redraw.
    pub fn refresh_windows(&mut self) {
        if self.pending_refresh_windows {
            record_coalesced_refresh_effect();
            return;
        }
        self.pending_refresh_windows = true;
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

    pub(crate) fn update<R>(&mut self, update: impl FnOnce(&mut Self) -> R) -> R {
        self.start_update();
        let result = update(self);
        self.finish_update();
        result
    }

    pub(crate) fn start_update(&mut self) {
        self.pending_updates += 1;
    }

    pub(crate) fn finish_update(&mut self) {
        if !self.flushing_effects && self.pending_updates == 1 {
            self.flushing_effects = true;
            self.flush_effects();
            self.flushing_effects = false;
        }
        self.pending_updates -= 1;
    }

    /// Creates an `AsyncApp`, which can be cloned and has a static lifetime
    /// so it can be held across `await` points.
    pub fn to_async(&self) -> AsyncApp {
        AsyncApp {
            app: self.this.clone(),
            background_executor: self.background_executor.clone(),
            foreground_executor: self.foreground_executor.clone(),
        }
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Obtains a reference to the executor, which can be used to spawn futures.
    pub fn foreground_executor(&self) -> &ForegroundExecutor {
        if self.quitting {
            panic!("Can't spawn on main thread after on_app_quit")
        };
        &self.foreground_executor
    }

    /// Spawns the future returned by the given function on the main thread. The closure will be invoked
    /// with [AsyncApp], which allows the application state to be accessed across await points.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        if self.quitting {
            debug_panic!("Can't spawn on main thread after on_app_quit")
        };

        let mut cx = self.to_async();

        self.foreground_executor
            .spawn(async move { f(&mut cx).await })
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&mut self, f: impl FnOnce(&mut App) + 'static) {
        self.push_effect(Effect::Defer {
            callback: Box::new(f),
        });
    }

    /// Accessor for the application's asset source, which is provided when constructing the `App`.
    pub fn asset_source(&self) -> &Arc<dyn AssetSource> {
        &self.asset_source
    }

    /// Accessor for the text system.
    pub fn text_system(&self) -> &Arc<TextSystem> {
        &self.text_system
    }

    /// Updates the application-wide default font family and synchronizes existing windows.
    pub fn set_default_font(&mut self, config: DefaultFontConfig) -> Result<()> {
        let mut fonts = Vec::new();
        let mut font_paths = Vec::new();
        for source in config.sources {
            match source {
                FontSource::SystemFamily(_) => {}
                FontSource::Path(path) => font_paths.push(path),
                FontSource::Embedded(bytes) => fonts.push(bytes),
            }
        }

        if !font_paths.is_empty() {
            self.text_system.add_font_paths(font_paths)?;
        }
        if !fonts.is_empty() {
            self.text_system.add_fonts(fonts)?;
        }
        self.text_system
            .preload_font_family(config.family.clone())?;
        self.text_system
            .set_system_font_family(config.family.clone());
        self.default_text_style.font_family = config.family;
        let default_text_style = self.default_text_style.clone();
        for window in self.windows.values_mut().flatten() {
            window.set_default_text_style(default_text_style.clone());
        }
        self.refresh_windows();
        Ok(())
    }

    /// Register a callback to be invoked when the application is about to quit.
    /// It is not possible to cancel the quit event at this point.
    pub fn on_app_quit<Fut>(
        &self,
        mut on_quit: impl FnMut(&mut App) -> Fut + 'static,
    ) -> Subscription
    where
        Fut: 'static + Future<Output = ()>,
    {
        let (subscription, activate) = self.quit_observers.insert(
            (),
            Box::new(move |cx| {
                let future = on_quit(cx);
                future.boxed_local()
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be invoked when the application is about to restart.
    ///
    /// These callbacks are called before any `on_app_quit` callbacks.
    pub fn on_app_restart(&self, mut on_restart: impl 'static + FnMut(&mut App)) -> Subscription {
        let (subscription, activate) = self.restart_observers.insert(
            (),
            Box::new(move |cx| {
                on_restart(cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Register a callback to be invoked when a window is closed
    /// The window is no longer accessible at the point this callback is invoked.
    pub fn on_window_closed(&self, mut on_closed: impl FnMut(&mut App) + 'static) -> Subscription {
        let (subscription, activate) = self.window_closed_observers.insert((), Box::new(on_closed));
        activate();
        subscription
    }

    #[expect(dead_code, reason = "used by diagnostics in debug and test builds")]
    pub(crate) fn entity_type_name(&self, entity_id: EntityId) -> &'static str {
        self.entities
            .type_name_for_id(entity_id)
            .unwrap_or("(dropped)")
    }

    /// Returns the name for this [`App`].
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn name(&self) -> Option<&'static str> {
        self.name
    }

    /// Sets the renderer for the inspector.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn set_inspector_renderer(&mut self, f: crate::InspectorRenderer) {
        self.inspector_renderer = Some(f);
    }

    /// Registers a renderer specific to an inspector state.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn register_inspector_element<T: 'static, R: crate::IntoElement>(
        &mut self,
        f: impl 'static + Fn(crate::InspectorElementId, &T, &mut Window, &mut App) -> R,
    ) {
        self.inspector_element_registry.register(f);
    }

    /// Initializes gpui's default colors for the application.
    ///
    /// These colors can be accessed through `cx.default_colors()`.
    pub fn init_colors(&mut self) {
        self.set_global(GlobalColors(Arc::new(Colors::default())));
    }
}

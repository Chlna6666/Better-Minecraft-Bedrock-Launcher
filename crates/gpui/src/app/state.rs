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
use futures::{Future, FutureExt, Stream, StreamExt, future::LocalBoxFuture};
use parking_lot::RwLock;
use slotmap::SlotMap;

use super::{AppCell, KeystrokeObserver, application::load_default_font_config};
use ::util::debug_panic;
use collections::{FxHashMap, FxHashSet, VecDeque};
use http_client::HttpClient;

#[cfg(any(feature = "inspector", debug_assertions))]
use crate::InspectorElementRegistry;
use crate::{
    ActionRegistry, AnyDrag, AnyEntity, AnyView, AnyWindowHandle, AssetSource, AsyncApp,
    BackgroundExecutor, Context, DefaultFontConfig, DispatchPhase, Effect, Entity, EntityId,
    EntityMap, EventEmitter, FocusMap, FontFallbacks, ForegroundExecutor, ImagePipelineConfig,
    Keymap, LayoutId, Platform, PlatformKeyboardLayout, PlatformKeyboardMapper, PromptBuilder,
    Render, SharedString, SubscriberSet, Subscription, SvgRenderer, Task, TextStyle, TextSystem,
    Window, WindowHandle, WindowId, WindowInvalidator, WindowTabRegistry,
    colors::{Colors, GlobalColors},
    init_app_menus, record_coalesced_refresh_effect,
};

mod events;
mod windows;

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
/// You need a reference to an [App] to access the state of a [Entity].
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
    /// Generation tokens for transient asset tasks whose cache entries retire on completion.
    /// The token prevents an older completion from deleting a newer task that reused the same key.
    pub(crate) transient_asset_generations: FxHashMap<(TypeId, u64), u64>,
    pub(crate) next_transient_asset_generation: u64,
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
        let image_pipeline_config = ImagePipelineConfig::default();
        crate::assets::configure_global_bitmap_pool(image_pipeline_config.bitmap_pool_bytes);

        let app = Rc::new_cyclic(|this| AppCell {
            app: RefCell::new(App {
                this: this.clone(),
                platform: platform.clone(),
                text_system,
                default_text_style: TextStyle::default(),
                default_window_icon: None,
                image_pipeline_config,
                actions: Rc::new(ActionRegistry::default()),
                flushing_effects: false,
                pending_updates: 0,
                pending_refresh_windows: false,
                active_drag: None,
                background_executor: executor,
                foreground_executor,
                svg_renderer: SvgRenderer::new(asset_source.clone()),
                loading_assets: Default::default(),
                transient_asset_generations: Default::default(),
                next_transient_asset_generation: 0,
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
                global_notification_counts: FxHashMap::default(),
                notifying_global_observers: FxHashSet::default(),
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
        WindowTabRegistry::init(&mut app.borrow_mut());

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

    /// Accessor for the application's background executor.
    pub fn background_executor(&self) -> &BackgroundExecutor {
        &self.background_executor
    }

    /// Accessor for the application's foreground executor.
    pub fn foreground_executor(&self) -> &ForegroundExecutor {
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

    /// Consume an asynchronous stream on the foreground executor and apply
    /// every item to application state in stream order. Each consumer is an
    /// independent foreground task and yields after every item so other ready
    /// streams, input, and rendering can make progress. The task ends when the
    /// stream closes or the application is released.
    ///
    /// Stream production may be asynchronous. The apply callback runs
    /// synchronously on the foreground executor and must remain bounded and
    /// non-blocking.
    ///
    /// The returned task must be stored or detached.
    #[track_caller]
    pub fn spawn_stream<S, F>(&self, stream: S, mut apply: F) -> Task<()>
    where
        S: Stream + 'static,
        F: FnMut(S::Item, &mut App) + 'static,
    {
        self.spawn(async move |cx| {
            futures::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                if cx.update(|cx| apply(item, cx)).is_err() {
                    break;
                }
                super::stream::yield_to_foreground_executor().await;
            }
        })
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
        let loaded = load_default_font_config(&self.text_system, config)?;
        self.default_text_style.font_family = loaded.family;
        self.synchronize_default_text_style();
        Ok(())
    }

    /// Changes the application-wide default family without replacing configured fallbacks.
    pub fn set_default_font_family(&mut self, family: impl Into<SharedString>) -> Result<()> {
        let family = family.into();
        self.text_system.preload_font_family(family.clone())?;
        self.text_system.set_system_font_family(family.clone());
        self.default_text_style.font_family = family;
        self.synchronize_default_text_style();
        Ok(())
    }

    /// Changes the application-wide fallback families and synchronizes existing windows.
    pub fn set_default_font_fallbacks<I, S>(&mut self, families: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<SharedString>,
    {
        let fallbacks =
            FontFallbacks::from_families(families.into_iter().map(Into::into).collect());
        self.text_system
            .set_fallback_font_families(fallbacks.fallback_list().iter().cloned().collect());
        self.default_text_style.font_fallbacks = (!fallbacks.is_empty()).then_some(fallbacks);
        self.synchronize_default_text_style();
    }

    fn synchronize_default_text_style(&mut self) {
        self.text_system
            .set_default_font(self.default_text_style.font());
        for window in self.windows.values_mut().flatten() {
            window.set_default_text_style(self.default_text_style.clone());
            window.refresh();
        }
    }

    /// Get the entity pointed to by this entity. Panics if the entity has been released
    #[track_caller]
    pub fn read_entity<T, R>(&self, handle: &Entity<T>, read: impl FnOnce(&T, &App) -> R) -> R
    where
        T: 'static,
    {
        handle.read(self, read)
    }

    /// Get mutable access to the entity pointed to by this entity. Panics if the entity has been released
    #[track_caller]
    pub fn update_entity<T, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R
    where
        T: 'static,
    {
        handle.update(self, update)
    }

    #[allow(dead_code)]
    pub(crate) fn clear_entities(&mut self) {
        self.entities.clear();
    }

    pub(crate) fn release_all(&mut self) {
        self.entities.release_all();
        for callback in mem::take(&mut self.release_listeners).consume() {
            let entity_id = callback.0;
            if let Some((entity, _)) = self.entities.get(entity_id) {
                callback.1(entity.downgrade().as_mut(), self);
            }
        }
    }

    fn has_pending_tasks(&self) -> bool {
        self.background_executor.has_pending_tasks() || self.foreground_executor.has_pending_tasks()
    }

    fn shutdown(&mut self) {
        self.quitting = true;
        self.platform.quit();
    }

    fn activate(&mut self, active: bool) {
        for window in self.windows.values_mut().flatten() {
            window.activate(active);
        }
    }

    pub(crate) fn update(&mut self, f: impl FnOnce(&mut Self)) {
        self.pending_updates += 1;
        f(self);
        self.pending_updates -= 1;
        if self.pending_updates == 0 {
            self.flush_effects();
        }
    }

    pub(crate) fn push_effect(&mut self, effect: Effect) {
        self.pending_effects.push_back(effect);
    }

    fn flush_effects(&mut self) {
        if self.flushing_effects {
            return;
        }
        self.flushing_effects = true;
        while let Some(effect) = self.pending_effects.pop_front() {
            match effect {
                Effect::Notify { entity_id } => {
                    self.pending_notifications.insert(entity_id);
                }
                Effect::NotifyGlobal { global_type } => {
                    self.pending_global_notifications.insert(global_type);
                }
                Effect::RefreshWindows => {
                    self.pending_refresh_windows = false;
                    for window in self.windows.values_mut().flatten() {
                        window.refresh();
                    }
                }
                Effect::Defer { callback } => callback(self),
            }
        }
        self.flushing_effects = false;

        self.notify_observers();
    }

    fn notify_observers(&mut self) {
        self.notify_global_observers();

        let mut pending_notifications = mem::take(&mut self.pending_notifications);
        let subscribers = self.observers.clone();
        pending_notifications.retain(|entity_id| {
            subscribers.retain(entity_id, |callback| callback(self));
            self.entities.refresh(entity_id);
            false
        });
        self.pending_notifications = pending_notifications;
    }

    fn notify_global_observers(&mut self) {
        let mut pending_notifications = mem::take(&mut self.pending_global_notifications);
        let subscribers = self.global_observers.clone();
        pending_notifications.retain(|global_type| {
            subscribers.retain(global_type, |callback| callback(self));
            false
        });
        self.pending_global_notifications = pending_notifications;
    }
}

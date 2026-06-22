use std::{
    any::{TypeId, type_name},
    borrow::Cow,
    cell::{BorrowMutError, Ref, RefCell, RefMut},
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{Arc, atomic::Ordering::SeqCst},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow};
use derive_more::{Deref, DerefMut};
use futures::{
    Future, FutureExt,
    channel::oneshot,
    future::{LocalBoxFuture, Shared},
};
use itertools::Itertools;
use parking_lot::RwLock;
use slotmap::SlotMap;

pub use async_context::*;
use collections::{FxHashMap, FxHashSet, HashMap, VecDeque};
pub use context::*;
pub use entity_map::*;
use http_client::{HttpClient, Url};
use smallvec::SmallVec;
#[cfg(any(test, feature = "test-support"))]
pub use test_context::*;
use util::{ResultExt, debug_panic};

#[cfg(any(feature = "inspector", debug_assertions))]
use crate::InspectorElementRegistry;
use crate::{
    Action, ActionBuildError, ActionRegistry, Any, AnyView, AnyWindowHandle, AppContext, Asset,
    AssetSource, BackgroundExecutor, Bounds, ClipboardItem, CursorStyle, DispatchPhase, DisplayId,
    EventEmitter, FocusHandle, FocusMap, ForegroundExecutor, Global, GpuiImageTargetAssetUsage,
    GpuiImageUsageScope, GpuiImageUsageScopeHandle, GpuiMemoryPolicy, GpuiMemorySnapshot,
    GpuiMemoryTrimLevel, ImagePipelineConfig, ImageUsageKind, KeyBinding, KeyContext, Keymap,
    Keystroke, LayoutId, Menu, MenuItem, OwnedMenu, PathPromptOptions, Pixels, Platform,
    PlatformDisplay, PlatformKeyboardLayout, PlatformKeyboardMapper, Point, Priority,
    PromptBuilder, PromptButton, PromptHandle, PromptLevel, Render, RenderImage,
    RenderablePromptHandle, RendererBackend, RendererOptions, Reservation, Resource,
    ScreenCaptureSource, SharedString, SubscriberSet, Subscription, SvgRenderer, Task, TextStyle,
    TextSystem, Window, WindowAppearance, WindowHandle, WindowId, WindowInvalidator,
    colors::{Colors, GlobalColors},
    current_platform,
    elements::{
        AnyImageCache, BoundedImageCache, BoundedImageCacheConfig, CompressedImgResourceLoader,
        EncodedImageDecoder, ImageCacheError, ImageDecoder, TargetSizeImgResourceLoader,
    },
    hash, init_app_menus, performance_metrics_snapshot, record_coalesced_refresh,
    record_coalesced_refresh_effect, record_foreground_effect_yield, record_inactive_dirty_defer,
    record_renderer_backend, trim_element_arena,
};

mod async_context;
mod context;
mod entity_map;
#[cfg(any(test, feature = "test-support"))]
mod test_context;

/// The duration for which futures returned from [Context::on_app_quit] can run before the application fully quits.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_GLOBAL_OBSERVER_NOTIFICATIONS_PER_FLUSH: usize = 64;
const FOREGROUND_EFFECT_FLUSH_BUDGET: Duration = Duration::from_millis(2);
const FOREGROUND_EFFECT_BATCH_LIMIT: usize = 64;
const STARTUP_TEXT_SYSTEM_WARM_UP_DELAY: Duration = Duration::from_millis(250);

/// Retained image asset totals in GPUI's global asset cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalImageAssetCacheSnapshot {
    /// Decoded bytes retained by uncached resource images.
    pub resource_decoded_bytes: usize,
    /// Number of completed uncached resource image assets.
    pub resource_count: usize,
    /// Decoded bytes retained by inline image assets.
    pub inline_decoded_bytes: usize,
    /// Number of completed inline image assets.
    pub inline_count: usize,
    /// Compressed image bytes retained for target-size decodes.
    pub compressed_bytes: usize,
    /// Number of completed compressed image assets.
    pub compressed_count: usize,
    /// Decoded bytes retained by target-size image assets.
    pub target_decoded_bytes: usize,
    /// Number of completed target-size image assets.
    pub target_count: usize,
}

/// Temporary(?) wrapper around [`RefCell<App>`] to help us debug any double borrows.
/// Strongly consider removing after stabilization.
#[doc(hidden)]
pub struct AppCell {
    app: RefCell<App>,
}

impl AppCell {
    #[doc(hidden)]
    #[track_caller]
    pub fn borrow(&self) -> AppRef<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRef(self.app.borrow())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn borrow_mut(&self) -> AppRefMut<'_> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        AppRefMut(self.app.borrow_mut())
    }

    #[doc(hidden)]
    #[track_caller]
    pub fn try_borrow_mut(&self) -> Result<AppRefMut<'_>, BorrowMutError> {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("borrowed {thread_id:?}");
        }
        Ok(AppRefMut(self.app.try_borrow_mut()?))
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRef<'a>(Ref<'a, App>);

impl Drop for AppRef<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped borrow from {thread_id:?}");
        }
    }
}

#[doc(hidden)]
#[derive(Deref, DerefMut)]
pub struct AppRefMut<'a>(RefMut<'a, App>);

impl Drop for AppRefMut<'_> {
    fn drop(&mut self) {
        if option_env!("TRACK_THREAD_BORROWS").is_some() {
            let thread_id = std::thread::current().id();
            eprintln!("dropped {thread_id:?}");
        }
    }
}

/// A reference to a GPUI application, typically constructed in the `main` function of your app.
/// You won't interact with this type much outside of initial configuration and startup.
pub struct Application(Rc<AppCell>);

/// A font source used by [`DefaultFontConfig`].
#[derive(Clone)]
pub enum FontSource {
    /// Use a font family already installed on the operating system.
    SystemFamily(SharedString),
    /// Load a font file from disk before the application starts.
    Path(PathBuf),
    /// Load font bytes embedded in the application binary.
    Embedded(Cow<'static, [u8]>),
}

/// Application-wide default text style configuration.
#[derive(Clone)]
pub struct DefaultFontConfig {
    /// The family name used by `TextStyle::default()` and new windows.
    pub family: SharedString,
    /// Fonts to register before the default family is used.
    pub sources: Vec<FontSource>,
}

impl DefaultFontConfig {
    /// Use a system font family as the application default.
    pub fn system_family(family: impl Into<SharedString>) -> Self {
        Self {
            family: family.into(),
            sources: Vec::new(),
        }
    }

    /// Use embedded font bytes as the application default family.
    pub fn embedded(
        family: impl Into<SharedString>,
        fonts: impl IntoIterator<Item = Cow<'static, [u8]>>,
    ) -> Self {
        Self {
            family: family.into(),
            sources: fonts.into_iter().map(FontSource::Embedded).collect(),
        }
    }

    /// Add a font file to load before selecting the default family.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.sources.push(FontSource::Path(path.into()));
        self
    }

    /// Add embedded font bytes to load before selecting the default family.
    pub fn with_embedded(mut self, bytes: impl Into<Cow<'static, [u8]>>) -> Self {
        self.sources.push(FontSource::Embedded(bytes.into()));
        self
    }
}

/// Represents an application before it is fully launched. Once your app is
/// configured, you'll start the app with `App::run`.
impl Application {
    /// Builds an app with the given asset source.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self::new_with_renderer_options(RendererOptions::default())
    }

    /// Builds an app with the requested renderer options in a single platform
    /// initialization pass.
    pub fn new_with_renderer_options(options: RendererOptions) -> Self {
        #[cfg(any(test, feature = "test-support"))]
        log::info!("GPUI was compiled in test mode");

        let options = options.resolve();
        record_renderer_backend(options.backend);
        Self(App::new_app(
            current_platform(false, options),
            Arc::new(()),
            Arc::new(NullHttpClient),
        ))
    }

    /// Builds an app with the requested renderer backend in a single platform
    /// initialization pass.
    pub fn new_with_renderer_backend(backend: RendererBackend) -> Self {
        Self::new_with_renderer_options(RendererOptions::with_backend(backend))
    }

    /// Build an app in headless mode. This prevents opening windows,
    /// but makes it possible to run an application in an context like
    /// SSH, where GUI applications are not allowed.
    pub fn headless() -> Self {
        record_renderer_backend(RendererBackend::Auto);
        Self(App::new_app(
            current_platform(true, RendererOptions::default()),
            Arc::new(()),
            Arc::new(NullHttpClient),
        ))
    }

    /// Sets the preferred renderer backend before the application is launched.
    ///
    /// `GPUI_RENDERER` takes precedence when it is set so local debugging and
    /// release diagnostics can override builder configuration without code changes.
    pub fn with_renderer_backend(self, backend: RendererBackend) -> Self {
        self.with_renderer_options(RendererOptions::with_backend(backend))
    }

    /// Sets renderer options before the application is launched.
    pub fn with_renderer_options(self, options: RendererOptions) -> Self {
        let options = options.resolve();
        record_renderer_backend(options.backend);
        self.0.borrow_mut().platform = current_platform(false, options);
        self
    }

    /// Sets the application-wide default font family and registers any provided font sources.
    pub fn try_with_default_font(self, config: DefaultFontConfig) -> Result<Self> {
        let family = config.family.clone();
        self.apply_default_font(config)?;
        self.0
            .borrow()
            .text_system
            .log_application_default_font(&family);
        Ok(self)
    }

    /// Sets the application-wide default font family, falling back to the platform font on error.
    pub fn with_default_font_or_platform_default(self, config: DefaultFontConfig) -> Self {
        let family = config.family.clone();
        if let Err(error) = self.apply_default_font(config) {
            self.0
                .borrow()
                .text_system
                .log_application_default_font_fallback(&family, &error);
        } else {
            self.0
                .borrow()
                .text_system
                .log_application_default_font(&family);
        }
        self
    }

    /// Registers the default native window icon for the application.
    ///
    /// Windows opened without an explicit [`crate::WindowOptions::window_icon`] will inherit it.
    pub fn with_default_window_icon(self, icon: crate::WindowIconSource) -> Self {
        self.0.borrow_mut().default_window_icon = Some(icon);
        self
    }

    fn apply_default_font(&self, config: DefaultFontConfig) -> Result<()> {
        let mut fonts = Vec::new();
        let mut font_paths = Vec::new();
        let preload_family = config
            .sources
            .iter()
            .any(|source| !matches!(source, FontSource::SystemFamily(_)));
        for source in config.sources {
            match source {
                FontSource::SystemFamily(_) => {}
                FontSource::Path(path) => font_paths.push(path),
                FontSource::Embedded(bytes) => fonts.push(bytes),
            }
        }

        let mut context_lock = self.0.borrow_mut();
        if !font_paths.is_empty() {
            context_lock.text_system.add_font_paths(font_paths)?;
        }
        if !fonts.is_empty() {
            context_lock.text_system.add_fonts(fonts)?;
        }
        if preload_family {
            context_lock
                .text_system
                .preload_font_family(config.family.clone())?;
        }
        context_lock
            .text_system
            .set_system_font_family(config.family.clone());
        context_lock.default_text_style.font_family = config.family;
        drop(context_lock);
        Ok(())
    }

    /// Sets the application-wide default font family without registering font data.
    pub fn try_with_default_font_family(self, family: impl Into<SharedString>) -> Result<Self> {
        self.try_with_default_font(DefaultFontConfig::system_family(family))
    }

    /// Sets application-wide image pipeline limits and animated image behavior.
    pub fn with_image_pipeline_config(self, config: ImagePipelineConfig) -> Self {
        {
            let mut app = self.0.borrow_mut();
            app.image_pipeline_config = config;
            let mut policy = app.gpui_memory_policy;
            policy.image_cache_max_bytes = config.max_decoded_bytes;
            app.set_gpui_memory_policy(policy);
        }
        self
    }

    /// Sets framework-wide GPUI memory retention limits.
    pub fn with_gpui_memory_policy(self, policy: GpuiMemoryPolicy) -> Self {
        {
            self.0.borrow_mut().set_gpui_memory_policy(policy);
        }
        self
    }

    /// Assign
    pub fn with_assets(self, asset_source: impl AssetSource) -> Self {
        let mut context_lock = self.0.borrow_mut();
        let asset_source = Arc::new(asset_source);
        context_lock.asset_source = asset_source.clone();
        context_lock.svg_renderer = SvgRenderer::new(asset_source);
        drop(context_lock);
        self
    }

    /// Sets the HTTP client for the application.
    pub fn with_http_client(self, http_client: Arc<dyn HttpClient>) -> Self {
        let mut context_lock = self.0.borrow_mut();
        context_lock.http_client = http_client;
        drop(context_lock);
        self
    }

    /// Start the application. The provided callback will be called once the
    /// app is fully launched.
    pub fn run<F>(self, on_finish_launching: F)
    where
        F: 'static + FnOnce(&mut App),
    {
        let this = self.0.clone();
        let platform = {
            let app = self.0.borrow();
            app.text_system.log_platform_default_font_once();
            app.platform.clone()
        };
        platform.run(Box::new(move || {
            let cx = &mut *this.borrow_mut();
            on_finish_launching(cx);
        }));
    }

    /// Register a handler to be invoked when the platform instructs the application
    /// to open one or more URLs.
    pub fn on_open_urls<F>(&self, mut callback: F) -> &Self
    where
        F: 'static + FnMut(Vec<String>),
    {
        self.0.borrow().platform.on_open_urls(Box::new(callback));
        self
    }

    /// Invokes a handler when an already-running application is launched.
    /// On macOS, this can occur when the application icon is double-clicked or the app is launched via the dock.
    pub fn on_reopen<F>(&self, mut callback: F) -> &Self
    where
        F: 'static + FnMut(&mut App),
    {
        let this = Rc::downgrade(&self.0);
        self.0.borrow_mut().platform.on_reopen(Box::new(move || {
            if let Some(app) = this.upgrade() {
                callback(&mut app.borrow_mut());
            }
        }));
        self
    }

    /// Returns a handle to the [`BackgroundExecutor`] associated with this app, which can be used to spawn futures in the background.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.0.borrow().background_executor.clone()
    }

    /// Returns a handle to the [`ForegroundExecutor`] associated with this app, which can be used to spawn futures in the foreground.
    pub fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.borrow().foreground_executor.clone()
    }

    /// Returns a reference to the [`TextSystem`] associated with this app.
    pub fn text_system(&self) -> Arc<TextSystem> {
        self.0.borrow().text_system.clone()
    }

    /// Returns the file URL of the executable with the specified name in the application bundle
    pub fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.0.borrow().path_for_auxiliary_executable(name)
    }
}

type Handler = Box<dyn FnMut(&mut App) -> bool + 'static>;
type Listener = Box<dyn FnMut(&dyn Any, &mut App) -> bool + 'static>;
pub(crate) type KeystrokeObserver =
    Box<dyn FnMut(&KeystrokeEvent, &mut Window, &mut App) -> bool + 'static>;
type QuitHandler = Box<dyn FnOnce(&mut App) -> LocalBoxFuture<'static, ()> + 'static>;
type WindowClosedHandler = Box<dyn FnMut(&mut App)>;
type ReleaseListener = Box<dyn FnOnce(&mut dyn Any, &mut App) + 'static>;
type NewEntityListener = Box<dyn FnMut(AnyEntity, &mut Option<&mut Window>, &mut App) + 'static>;

#[doc(hidden)]
#[derive(Clone, PartialEq, Eq)]
pub struct SystemWindowTab {
    pub id: WindowId,
    pub title: SharedString,
    pub handle: AnyWindowHandle,
    pub last_active_at: Instant,
}

impl SystemWindowTab {
    /// Create a new instance of the window tab.
    pub fn new(title: SharedString, handle: AnyWindowHandle) -> Self {
        Self {
            id: handle.id,
            title,
            handle,
            last_active_at: Instant::now(),
        }
    }
}

/// A controller for managing window tabs.
#[derive(Default)]
pub struct SystemWindowTabController {
    visible: Option<bool>,
    tab_groups: FxHashMap<usize, Vec<SystemWindowTab>>,
}

impl Global for SystemWindowTabController {}

impl SystemWindowTabController {
    /// Create a new instance of the window tab controller.
    pub fn new() -> Self {
        Self {
            visible: None,
            tab_groups: FxHashMap::default(),
        }
    }

    /// Initialize the global window tab controller.
    pub fn init(cx: &mut App) {
        cx.set_global(SystemWindowTabController::new());
    }

    /// Get all tab groups.
    pub fn tab_groups(&self) -> &FxHashMap<usize, Vec<SystemWindowTab>> {
        &self.tab_groups
    }

    /// Get the next tab group window handle.
    pub fn get_next_tab_group_window(cx: &mut App, id: WindowId) -> Option<&AnyWindowHandle> {
        let controller = cx.global::<SystemWindowTabController>();
        let current_group = controller
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| group));

        let current_group = current_group?;
        let mut group_ids: Vec<_> = controller.tab_groups.keys().collect();
        let idx = group_ids.iter().position(|g| *g == current_group)?;
        let next_idx = (idx + 1) % group_ids.len();

        controller
            .tab_groups
            .get(group_ids[next_idx])
            .and_then(|tabs| {
                tabs.iter()
                    .max_by_key(|tab| tab.last_active_at)
                    .or_else(|| tabs.first())
                    .map(|tab| &tab.handle)
            })
    }

    /// Get the previous tab group window handle.
    pub fn get_prev_tab_group_window(cx: &mut App, id: WindowId) -> Option<&AnyWindowHandle> {
        let controller = cx.global::<SystemWindowTabController>();
        let current_group = controller
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| group));

        let current_group = current_group?;
        let mut group_ids: Vec<_> = controller.tab_groups.keys().collect();
        let idx = group_ids.iter().position(|g| *g == current_group)?;
        let prev_idx = if idx == 0 {
            group_ids.len() - 1
        } else {
            idx - 1
        };

        controller
            .tab_groups
            .get(group_ids[prev_idx])
            .and_then(|tabs| {
                tabs.iter()
                    .max_by_key(|tab| tab.last_active_at)
                    .or_else(|| tabs.first())
                    .map(|tab| &tab.handle)
            })
    }

    /// Get all tabs in the same window.
    pub fn tabs(&self, id: WindowId) -> Option<&Vec<SystemWindowTab>> {
        let tab_group = self
            .tab_groups
            .iter()
            .find_map(|(group, tabs)| tabs.iter().find(|tab| tab.id == id).map(|_| *group))?;

        self.tab_groups.get(&tab_group)
    }

    /// Initialize the visibility of the system window tab controller.
    pub fn init_visible(cx: &mut App, visible: bool) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        if controller.visible.is_none() {
            controller.visible = Some(visible);
        }
    }

    /// Get the visibility of the system window tab controller.
    pub fn is_visible(&self) -> bool {
        self.visible.unwrap_or(false)
    }

    /// Set the visibility of the system window tab controller.
    pub fn set_visible(cx: &mut App, visible: bool) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        controller.visible = Some(visible);
    }

    /// Update the last active of a window.
    pub fn update_last_active(cx: &mut App, id: WindowId) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        for windows in controller.tab_groups.values_mut() {
            for tab in windows.iter_mut() {
                if tab.id == id {
                    tab.last_active_at = Instant::now();
                }
            }
        }
    }

    /// Update the position of a tab within its group.
    pub fn update_tab_position(cx: &mut App, id: WindowId, ix: usize) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        for (_, windows) in controller.tab_groups.iter_mut() {
            if let Some(current_pos) = windows.iter().position(|tab| tab.id == id) {
                if ix < windows.len() && current_pos != ix {
                    let window_tab = windows.remove(current_pos);
                    windows.insert(ix, window_tab);
                }
                break;
            }
        }
    }

    /// Update the title of a tab.
    pub fn update_tab_title(cx: &mut App, id: WindowId, title: SharedString) {
        let controller = cx.global::<SystemWindowTabController>();
        let tab = controller
            .tab_groups
            .values()
            .flat_map(|windows| windows.iter())
            .find(|tab| tab.id == id);

        if tab.map_or(true, |t| t.title == title) {
            return;
        }

        let mut controller = cx.global_mut::<SystemWindowTabController>();
        for windows in controller.tab_groups.values_mut() {
            for tab in windows.iter_mut() {
                if tab.id == id {
                    tab.title = title;
                    return;
                }
            }
        }
    }

    /// Insert a tab into a tab group.
    pub fn add_tab(cx: &mut App, id: WindowId, tabs: Vec<SystemWindowTab>) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tab) = tabs.clone().into_iter().find(|tab| tab.id == id) else {
            return;
        };

        let mut expected_tab_ids: Vec<_> = tabs
            .iter()
            .filter(|tab| tab.id != id)
            .map(|tab| tab.id)
            .sorted()
            .collect();

        let mut tab_group_id = None;
        for (group_id, group_tabs) in &controller.tab_groups {
            let tab_ids: Vec<_> = group_tabs.iter().map(|tab| tab.id).sorted().collect();
            if tab_ids == expected_tab_ids {
                tab_group_id = Some(*group_id);
                break;
            }
        }

        if let Some(tab_group_id) = tab_group_id {
            if let Some(tabs) = controller.tab_groups.get_mut(&tab_group_id) {
                tabs.push(tab);
            }
        } else {
            let new_group_id = controller.tab_groups.len();
            controller.tab_groups.insert(new_group_id, tabs);
        }
    }

    /// Remove a tab from a tab group.
    pub fn remove_tab(cx: &mut App, id: WindowId) -> Option<SystemWindowTab> {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        let mut removed_tab = None;

        controller.tab_groups.retain(|_, tabs| {
            if let Some(pos) = tabs.iter().position(|tab| tab.id == id) {
                removed_tab = Some(tabs.remove(pos));
            }
            !tabs.is_empty()
        });

        removed_tab
    }

    /// Move a tab to a new tab group.
    pub fn move_tab_to_new_window(cx: &mut App, id: WindowId) {
        let mut removed_tab = Self::remove_tab(cx, id);
        let mut controller = cx.global_mut::<SystemWindowTabController>();

        if let Some(tab) = removed_tab {
            let new_group_id = controller.tab_groups.keys().max().map_or(0, |k| k + 1);
            controller.tab_groups.insert(new_group_id, vec![tab]);
        }
    }

    /// Merge all tab groups into a single group.
    pub fn merge_all_windows(cx: &mut App, id: WindowId) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        let Some(initial_tabs) = controller.tabs(id) else {
            return;
        };

        let mut all_tabs = initial_tabs.clone();
        for tabs in controller.tab_groups.values() {
            all_tabs.extend(
                tabs.iter()
                    .filter(|tab| !initial_tabs.contains(tab))
                    .cloned(),
            );
        }

        controller.tab_groups.clear();
        controller.tab_groups.insert(0, all_tabs);
    }

    /// Selects the next tab in the tab group in the trailing direction.
    pub fn select_next_tab(cx: &mut App, id: WindowId) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tabs) = controller.tabs(id) else {
            return;
        };

        let current_index = tabs.iter().position(|tab| tab.id == id).unwrap();
        let next_index = (current_index + 1) % tabs.len();

        let _ = &tabs[next_index].handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    }

    /// Selects the previous tab in the tab group in the leading direction.
    pub fn select_previous_tab(cx: &mut App, id: WindowId) {
        let mut controller = cx.global_mut::<SystemWindowTabController>();
        let Some(tabs) = controller.tabs(id) else {
            return;
        };

        let current_index = tabs.iter().position(|tab| tab.id == id).unwrap();
        let previous_index = if current_index == 0 {
            tabs.len() - 1
        } else {
            current_index - 1
        };

        let _ = &tabs[previous_index].handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    }
}

/// Contains the state of the full application, and passed as a reference to a variety of callbacks.
/// Other [Context] derefs to this type.
/// You need a reference to an `App` to access the state of a [Entity].
pub struct App {
    pub(crate) this: Weak<AppCell>,
    pub(crate) platform: Rc<dyn Platform>,
    text_system: Arc<TextSystem>,
    pub(crate) default_text_style: TextStyle,
    pub(crate) default_window_icon: Option<crate::WindowIconSource>,
    pub(crate) image_pipeline_config: ImagePipelineConfig,
    gpui_memory_policy: GpuiMemoryPolicy,
    default_image_cache: Option<Entity<BoundedImageCache>>,
    default_image_cache_config: BoundedImageCacheConfig,
    image_usage_scopes: FxHashMap<GpuiImageUsageScope, FxHashSet<(TypeId, u64)>>,
    image_asset_scopes: FxHashMap<(TypeId, u64), FxHashSet<GpuiImageUsageScope>>,
    image_usage_kinds: FxHashMap<(TypeId, u64), ImageUsageKind>,
    target_image_assets: FxHashMap<u64, GpuiImageTargetAssetUsage>,
    image_asset_touch_clock: u64,
    flushing_effects: bool,
    pending_updates: usize,
    pending_refresh_windows: bool,
    text_system_warm_up_scheduled: bool,
    pub(crate) actions: Rc<ActionRegistry>,
    pub(crate) active_drag: Option<AnyDrag>,
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) loading_assets: FxHashMap<(TypeId, u64), Box<dyn Any>>,
    asset_source: Arc<dyn AssetSource>,
    pub(crate) svg_renderer: SvgRenderer,
    http_client: Arc<dyn HttpClient>,
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
    pending_effects: VecDeque<Effect>,
    pub(crate) pending_notifications: FxHashSet<EntityId>,
    pub(crate) pending_global_notifications: FxHashSet<TypeId>,
    notifying_global_observers: FxHashSet<TypeId>,
    global_notification_counts: FxHashMap<TypeId, usize>,
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

fn bounded_cache_config_from_policy(policy: GpuiMemoryPolicy) -> BoundedImageCacheConfig {
    BoundedImageCacheConfig {
        max_items: 512,
        max_bytes: policy.image_cache_max_bytes,
    }
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

        let text_system = Arc::new(TextSystem::new(platform.text_system().clone()));
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
                gpui_memory_policy: GpuiMemoryPolicy::default(),
                default_image_cache: None,
                default_image_cache_config: bounded_cache_config_from_policy(
                    GpuiMemoryPolicy::default(),
                ),
                image_usage_scopes: FxHashMap::default(),
                image_asset_scopes: FxHashMap::default(),
                image_usage_kinds: FxHashMap::default(),
                target_image_assets: FxHashMap::default(),
                image_asset_touch_clock: 0,
                actions: Rc::new(ActionRegistry::default()),
                flushing_effects: false,
                pending_updates: 0,
                pending_refresh_windows: false,
                text_system_warm_up_scheduled: false,
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

    /// Arrange a callback to be invoked when the given entity calls `notify` on its respective context.
    pub fn observe<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        self.observe_internal(entity, move |e, cx| {
            on_notify(e, cx);
            true
        })
    }

    pub(crate) fn detect_accessed_entities<R>(
        &mut self,
        callback: impl FnOnce(&mut App) -> R,
    ) -> (R, FxHashSet<EntityId>) {
        let accessed_entities_start = self.entities.accessed_entities.borrow().clone();
        let result = callback(self);
        let accessed_entities_end = self.entities.accessed_entities.borrow().clone();
        let entities_accessed_in_callback = accessed_entities_end
            .difference(&accessed_entities_start)
            .copied()
            .collect::<FxHashSet<EntityId>>();
        (result, entities_accessed_in_callback)
    }

    pub(crate) fn record_entities_accessed(
        &mut self,
        window_handle: AnyWindowHandle,
        invalidator: WindowInvalidator,
        entities: &FxHashSet<EntityId>,
    ) {
        let mut tracked_entities =
            std::mem::take(self.tracked_entities.entry(window_handle.id).or_default());
        for entity in tracked_entities.iter() {
            self.window_invalidators_by_entity
                .entry(*entity)
                .and_modify(|windows| {
                    windows.remove(&window_handle.id);
                });
        }
        for entity in entities.iter() {
            self.window_invalidators_by_entity
                .entry(*entity)
                .or_default()
                .insert(window_handle.id, invalidator.clone());
        }
        tracked_entities.clear();
        tracked_entities.extend(entities.iter().copied());
        self.tracked_entities
            .insert(window_handle.id, tracked_entities);
    }

    pub(crate) fn new_observer(&mut self, key: EntityId, value: Handler) -> Subscription {
        let (subscription, activate) = self.observers.insert(key, value);
        self.defer(move |_| activate());
        subscription
    }

    pub(crate) fn observe_internal<W>(
        &mut self,
        entity: &Entity<W>,
        mut on_notify: impl FnMut(Entity<W>, &mut App) -> bool + 'static,
    ) -> Subscription
    where
        W: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        self.new_observer(
            entity_id,
            Box::new(move |cx| {
                if let Some(entity) = handle.upgrade() {
                    on_notify(entity, cx)
                } else {
                    false
                }
            }),
        )
    }

    /// Arrange for the given callback to be invoked whenever the given entity emits an event of a given type.
    /// The callback is provided a handle to the emitting entity and a reference to the emitted event.
    pub fn subscribe<T, Event>(
        &mut self,
        entity: &Entity<T>,
        mut on_event: impl FnMut(Entity<T>, &Event, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static + EventEmitter<Event>,
        Event: 'static,
    {
        self.subscribe_internal(entity, move |entity, event, cx| {
            on_event(entity, event, cx);
            true
        })
    }

    pub(crate) fn new_subscription(
        &mut self,
        key: EntityId,
        value: (TypeId, Listener),
    ) -> Subscription {
        let (subscription, activate) = self.event_listeners.insert(key, value);
        self.defer(move |_| activate());
        subscription
    }
    pub(crate) fn subscribe_internal<T, Evt>(
        &mut self,
        entity: &Entity<T>,
        mut on_event: impl FnMut(Entity<T>, &Evt, &mut App) -> bool + 'static,
    ) -> Subscription
    where
        T: 'static + EventEmitter<Evt>,
        Evt: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        self.new_subscription(
            entity_id,
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    let event: &Evt = event.downcast_ref().expect("invalid event type");
                    if let Some(entity) = handle.upgrade() {
                        on_event(entity, event, cx)
                    } else {
                        false
                    }
                }),
            ),
        )
    }

    /// Returns handles to all open windows in the application.
    /// Each handle could be downcast to a handle typed for the root view of that window.
    /// To find all windows of a given type, you could filter on
    pub fn windows(&self) -> Vec<AnyWindowHandle> {
        self.windows
            .keys()
            .flat_map(|window_id| self.window_handles.get(&window_id).copied())
            .collect()
    }

    /// Returns the window handles ordered by their appearance on screen, front to back.
    ///
    /// The first window in the returned list is the active/topmost window of the application.
    ///
    /// This method returns None if the platform doesn't implement the method yet.
    pub fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.platform.window_stack()
    }

    /// Returns a handle to the window that is currently focused at the platform level, if one exists.
    pub fn active_window(&self) -> Option<AnyWindowHandle> {
        self.platform.active_window()
    }

    /// Opens a new window with the given option and the root view returned by the given function.
    /// The function is invoked with a `Window`, which can be used to interact with window-specific
    /// functionality.
    pub fn open_window<V: 'static + Render>(
        &mut self,
        options: crate::WindowOptions,
        build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> anyhow::Result<WindowHandle<V>> {
        self.update(|cx| {
            let id = cx.windows.insert(None);
            let handle = WindowHandle::new(id);
            match Window::new(handle.into(), options, cx) {
                Ok(mut window) => {
                    cx.window_update_stack.push(id);
                    let root_view = build_root_view(&mut window, cx);
                    cx.window_update_stack.pop();
                    window.root.replace(root_view.into());
                    window.defer(cx, |window: &mut Window, cx| window.appearance_changed(cx));

                    // allow a window to draw at least once before returning
                    // this didn't cause any issues on non windows platforms as it seems we always won the race to on_request_frame
                    // on windows we quite frequently lose the race and return a window that has never rendered, which leads to a crash
                    // where DispatchTree::root_node_id asserts on empty nodes
                    let clear = window.draw(cx, crate::window::SLOW_FRAME_REQUEST);
                    clear.clear();

                    cx.window_handles.insert(id, window.handle);
                    cx.windows.get_mut(id).unwrap().replace(Box::new(window));
                    Ok(handle)
                }
                Err(e) => {
                    cx.windows.remove(id);
                    Err(e)
                }
            }
        })
    }

    /// Instructs the platform to activate the application by bringing it to the foreground.
    pub fn activate(&self, ignoring_other_apps: bool) {
        self.platform.activate(ignoring_other_apps);
    }

    /// Hide the application at the platform level.
    pub fn hide(&self) {
        self.platform.hide();
    }

    /// Hide other applications at the platform level.
    pub fn hide_other_apps(&self) {
        self.platform.hide_other_apps();
    }

    /// Unhide other applications at the platform level.
    pub fn unhide_other_apps(&self) {
        self.platform.unhide_other_apps();
    }

    /// Returns the list of currently active displays.
    pub fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.platform.displays()
    }

    /// Returns the primary display that will be used for new windows.
    pub fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.platform.primary_display()
    }

    /// Returns whether `screen_capture_sources` may work.
    pub fn is_screen_capture_supported(&self) -> bool {
        self.platform.is_screen_capture_supported()
    }

    /// Returns a list of available screen capture sources.
    pub fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        self.platform.screen_capture_sources()
    }

    /// Returns the display with the given ID, if one exists.
    pub fn find_display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        self.displays()
            .iter()
            .find(|display| display.id() == id)
            .cloned()
    }

    /// Returns the appearance of the application's windows.
    pub fn window_appearance(&self) -> WindowAppearance {
        self.platform.window_appearance()
    }

    /// Writes data to the primary selection buffer.
    /// Only available on Linux.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn write_to_primary(&self, item: ClipboardItem) {
        self.platform.write_to_primary(item)
    }

    /// Writes data to the platform clipboard.
    pub fn write_to_clipboard(&self, item: ClipboardItem) {
        self.platform.write_to_clipboard(item)
    }

    /// Reads data from the primary selection buffer.
    /// Only available on Linux.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.platform.read_from_primary()
    }

    /// Reads data from the platform clipboard.
    pub fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.platform.read_from_clipboard()
    }

    /// Writes credentials to the platform keychain.
    pub fn write_credentials(
        &self,
        url: &str,
        username: &str,
        password: &[u8],
    ) -> Task<Result<()>> {
        self.platform.write_credentials(url, username, password)
    }

    /// Reads credentials from the platform keychain.
    pub fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        self.platform.read_credentials(url)
    }

    /// Deletes credentials from the platform keychain.
    pub fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        self.platform.delete_credentials(url)
    }

    /// Directs the platform's default browser to open the given URL.
    pub fn open_url(&self, url: &str) {
        self.platform.open_url(url);
    }

    /// Registers the given URL scheme (e.g. `zed` for `zed://` urls) to be
    /// opened by the current app.
    ///
    /// On some platforms (e.g. macOS) you may be able to register URL schemes
    /// as part of app distribution, but this method exists to let you register
    /// schemes at runtime.
    pub fn register_url_scheme(&self, scheme: &str) -> Task<Result<()>> {
        self.platform.register_url_scheme(scheme)
    }

    /// Returns the full pathname of the current app bundle.
    ///
    /// Returns an error if the app is not being run from a bundle.
    pub fn app_path(&self) -> Result<PathBuf> {
        self.platform.app_path()
    }

    /// On Linux, returns the name of the compositor in use.
    ///
    /// Returns an empty string on other platforms.
    pub fn compositor_name(&self) -> &'static str {
        self.platform.compositor_name()
    }

    /// Returns the file URL of the executable with the specified name in the application bundle
    pub fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        self.platform.path_for_auxiliary_executable(name)
    }

    /// Displays a platform modal for selecting paths.
    ///
    /// When one or more paths are selected, they'll be relayed asynchronously via the returned oneshot channel.
    /// If cancelled, a `None` will be relayed instead.
    /// May return an error on Linux if the file picker couldn't be opened.
    pub fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.platform.prompt_for_paths(options)
    }

    /// Displays a platform modal for selecting a new path where a file can be saved.
    ///
    /// The provided directory will be used to set the initial location.
    /// When a path is selected, it is relayed asynchronously via the returned oneshot channel.
    /// If cancelled, a `None` will be relayed instead.
    /// May return an error on Linux if the file picker couldn't be opened.
    pub fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.platform.prompt_for_new_path(directory, suggested_name)
    }

    /// Reveals the specified path at the platform level, such as in Finder on macOS.
    pub fn reveal_path(&self, path: &Path) {
        self.platform.reveal_path(path)
    }

    /// Opens the specified path with the system's default application.
    pub fn open_with_system(&self, path: &Path) {
        self.platform.open_with_system(path)
    }

    /// Returns whether the user has configured scrollbars to auto-hide at the platform level.
    pub fn should_auto_hide_scrollbars(&self) -> bool {
        self.platform.should_auto_hide_scrollbars()
    }

    /// Restarts the application.
    pub fn restart(&mut self) {
        self.restart_observers
            .clone()
            .retain(&(), |observer| observer(self));
        self.platform.restart(self.restart_path.take())
    }

    /// Sets the path to use when restarting the application.
    pub fn set_restart_path(&mut self, path: PathBuf) {
        self.restart_path = Some(path);
    }

    /// Returns the HTTP client for the application.
    pub fn http_client(&self) -> Arc<dyn HttpClient> {
        self.http_client.clone()
    }

    /// Sets the HTTP client for the application.
    pub fn set_http_client(&mut self, new_client: Arc<dyn HttpClient>) {
        self.http_client = new_client;
    }

    /// Returns the SVG renderer used by the application.
    pub fn svg_renderer(&self) -> SvgRenderer {
        self.svg_renderer.clone()
    }

    pub(crate) fn push_effect(&mut self, effect: Effect) {
        match &effect {
            Effect::Notify { emitter } => {
                if !self.pending_notifications.insert(*emitter) {
                    return;
                }
            }
            Effect::NotifyGlobalObservers { global_type } => {
                if self.notifying_global_observers.contains(global_type) {
                    self.defer_global_notification(*global_type);
                    return;
                }
                if !self.pending_global_notifications.insert(*global_type) {
                    return;
                }
            }
            _ => {}
        };

        self.pending_effects.push_back(effect);
    }

    fn defer_global_notification(&mut self, global_type: TypeId) {
        if !self.pending_global_notifications.insert(global_type) {
            return;
        }

        let Some(app) = self.this.upgrade() else {
            return;
        };
        let foreground_executor = self.foreground_executor.clone();
        foreground_executor
            .spawn(async move {
                let mut app = app.borrow_mut();
                app.update(|app| {
                    app.pending_global_notifications.remove(&global_type);
                    app.push_effect(Effect::NotifyGlobalObservers { global_type });
                });
            })
            .detach();
    }

    /// Called at the end of [`App::update`] to complete any side effects
    /// such as notifying observers, emitting events, etc. Effects can themselves
    /// cause effects, so we continue looping until all effects are processed.
    fn flush_effects(&mut self) {
        let flush_started_at = Instant::now();
        let mut processed_effects = 0usize;
        let mut notify_count = 0usize;
        let mut global_notify_count = 0usize;
        let mut emit_count = 0usize;
        let mut refresh_count = 0usize;
        let mut defer_count = 0usize;
        let mut created_count = 0usize;
        loop {
            self.release_dropped_entities();
            self.release_dropped_focus_handles();
            if let Some(effect) = self.pending_effects.pop_front() {
                processed_effects += 1;
                match effect {
                    Effect::Notify { emitter } => {
                        notify_count += 1;
                        self.apply_notify_effect(emitter);
                    }

                    Effect::Emit {
                        emitter,
                        event_type,
                        event,
                    } => {
                        emit_count += 1;
                        self.apply_emit_effect(emitter, event_type, event)
                    }

                    Effect::RefreshWindows => {
                        refresh_count += 1;
                        self.apply_refresh_effect();
                    }

                    Effect::NotifyGlobalObservers { global_type } => {
                        global_notify_count += 1;
                        self.apply_notify_global_observers_effect(global_type);
                    }

                    Effect::Defer { callback } => {
                        defer_count += 1;
                        self.apply_defer_effect(callback);
                    }
                    Effect::EntityCreated {
                        entity,
                        tid,
                        window,
                    } => {
                        created_count += 1;
                        self.apply_entity_created_effect(entity, tid, window);
                    }
                }

                if !self.pending_effects.is_empty()
                    && (processed_effects >= FOREGROUND_EFFECT_BATCH_LIMIT
                        || flush_started_at.elapsed() >= FOREGROUND_EFFECT_FLUSH_BUDGET)
                {
                    record_foreground_effect_yield(self.pending_effects.len());
                    self.request_dirty_window_frames();
                    self.defer_effect_flush();
                    break;
                }
            } else {
                self.request_dirty_window_frames();

                #[cfg(any(test, feature = "test-support"))]
                for window in self
                    .windows
                    .values()
                    .filter_map(|window| {
                        let window = window.as_deref()?;
                        window.invalidator.is_dirty().then_some(window.handle)
                    })
                    .collect::<Vec<_>>()
                {
                    self.update_window(window, |_, window, cx| {
                        window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear()
                    })
                    .unwrap();
                }

                if self.pending_effects.is_empty() {
                    let elapsed = flush_started_at.elapsed();
                    let total = notify_count
                        + global_notify_count
                        + emit_count
                        + refresh_count
                        + defer_count
                        + created_count;
                    if total > 0 {
                        if elapsed >= Duration::from_millis(16) || total >= 64 {
                            log::warn!(
                                "gpui flush_effects heavy: elapsed={:?} total={} notify={} global_notify={} emit={} refresh={} defer={} created={}",
                                elapsed,
                                total,
                                notify_count,
                                global_notify_count,
                                emit_count,
                                refresh_count,
                                defer_count,
                                created_count
                            );
                        } else if log::log_enabled!(log::Level::Debug) {
                            log::debug!(
                                "gpui flush_effects: elapsed={:?} total={} notify={} global_notify={} emit={} refresh={} defer={} created={}",
                                elapsed,
                                total,
                                notify_count,
                                global_notify_count,
                                emit_count,
                                refresh_count,
                                defer_count,
                                created_count
                            );
                        }
                    }
                    self.pending_notifications.clear();
                    self.global_notification_counts.clear();
                    break;
                }
            }
        }
    }

    fn defer_effect_flush(&self) {
        let Some(app) = self.this.upgrade() else {
            return;
        };
        let foreground_executor = self.foreground_executor.clone();
        foreground_executor
            .spawn(async move {
                let mut app = app.borrow_mut();
                app.update(|_| {});
            })
            .detach();
    }

    fn request_dirty_window_frames(&mut self) {
        for window in self
            .windows
            .values()
            .filter_map(|window| {
                let window = window.as_deref()?;
                if !window.invalidator.is_dirty() {
                    return None;
                }
                if window.refreshing {
                    record_coalesced_refresh();
                    return None;
                }
                if window.should_defer_dirty_frame_request() {
                    record_inactive_dirty_defer();
                    return None;
                }
                Some(window.handle)
            })
            .collect::<Vec<_>>()
        {
            log::debug!(
                "gpui request dirty window frame: window={}",
                window.window_id().as_u64()
            );
            self.update_window(window, |_, window, _| {
                window.request_dirty_frame_if_needed();
            })
            .log_err();
        }
    }

    /// Repeatedly called during `flush_effects` to release any entities whose
    /// reference count has become zero. We invoke any release observers before dropping
    /// each entity.
    fn release_dropped_entities(&mut self) {
        loop {
            let dropped = self.entities.take_dropped();
            if dropped.is_empty() {
                break;
            }

            for (entity_id, mut entity) in dropped {
                self.observers.remove(&entity_id);
                self.event_listeners.remove(&entity_id);
                for release_callback in self.release_listeners.remove(&entity_id) {
                    release_callback(entity.as_mut(), self);
                }
            }
        }
    }

    /// Repeatedly called during `flush_effects` to handle a focused handle being dropped.
    fn release_dropped_focus_handles(&mut self) {
        self.focus_handles
            .clone()
            .write()
            .retain(|handle_id, focus| {
                if focus.ref_count.load(SeqCst) == 0 {
                    for window_handle in self.windows() {
                        window_handle
                            .update(self, |_, window, _| {
                                if window.focus == Some(handle_id) {
                                    window.blur();
                                }
                            })
                            .unwrap();
                    }
                    false
                } else {
                    true
                }
            });
    }

    fn apply_notify_effect(&mut self, emitter: EntityId) {
        self.observers
            .clone()
            .retain(&emitter, |handler| handler(self));
    }

    fn apply_emit_effect(&mut self, emitter: EntityId, event_type: TypeId, event: Box<dyn Any>) {
        self.event_listeners
            .clone()
            .retain(&emitter, |(stored_type, handler)| {
                if *stored_type == event_type {
                    handler(event.as_ref(), self)
                } else {
                    true
                }
            });
    }

    fn apply_refresh_effect(&mut self) {
        self.pending_refresh_windows = false;
        for window in self.windows.values_mut() {
            if let Some(window) = window.as_deref_mut() {
                window.refresh();
            }
        }
    }

    fn apply_notify_global_observers_effect(&mut self, type_id: TypeId) {
        self.pending_global_notifications.remove(&type_id);
        if self.notifying_global_observers.contains(&type_id) {
            self.defer_global_notification(type_id);
            return;
        }

        let count = self
            .global_notification_counts
            .entry(type_id)
            .and_modify(|count| *count += 1)
            .or_insert(1);
        if *count > MAX_GLOBAL_OBSERVER_NOTIFICATIONS_PER_FLUSH {
            log::warn!(
                "deferred global observer notification for {:?} after {} same-flush iterations",
                type_id,
                *count
            );
            self.defer_global_notification(type_id);
            return;
        }

        self.notifying_global_observers.insert(type_id);
        self.global_observers
            .clone()
            .retain(&type_id, |observer| observer(self));
        self.notifying_global_observers.remove(&type_id);
    }

    fn apply_defer_effect(&mut self, callback: Box<dyn FnOnce(&mut Self) + 'static>) {
        callback(self);
    }

    fn apply_entity_created_effect(
        &mut self,
        entity: AnyEntity,
        tid: TypeId,
        window: Option<WindowId>,
    ) {
        self.new_entity_observers.clone().retain(&tid, |observer| {
            if let Some(id) = window {
                self.update_window_id(id, {
                    let entity = entity.clone();
                    |_, window, cx| (observer)(entity, &mut Some(window), cx)
                })
                .expect("All windows should be off the stack when flushing effects");
            } else {
                (observer)(entity.clone(), &mut None, self)
            }
            true
        });
    }

    fn update_window_id<T, F>(&mut self, id: WindowId, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.update(|cx| {
            let mut window = cx.windows.get_mut(id)?.take()?;

            let root_view = window.root.clone().unwrap();

            cx.window_update_stack.push(window.handle.id);
            let result = update(root_view, &mut window, cx);
            cx.window_update_stack.pop();

            if window.removed {
                cx.window_handles.remove(&id);
                cx.windows.remove(id);

                cx.window_closed_observers.clone().retain(&(), |callback| {
                    callback(cx);
                    true
                });
            } else {
                cx.windows.get_mut(id)?.replace(window);
            }

            Some(result)
        })
        .ok_or_else(|| anyhow!("window not found"))
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

    /// Spawns the future returned by the given function on the main thread with
    /// the given priority.
    #[track_caller]
    pub fn spawn_with_priority<AsyncFn, R>(&self, priority: Priority, f: AsyncFn) -> Task<R>
    where
        AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
        R: 'static,
    {
        if self.quitting {
            debug_panic!("Can't spawn on main thread after on_app_quit")
        };

        let mut cx = self.to_async();

        self.foreground_executor
            .spawn_with_priority(priority, async move { f(&mut cx).await })
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

    pub(crate) fn warm_up_text_system_after_startup_frame(&mut self) {
        if self.text_system_warm_up_scheduled {
            return;
        }
        self.text_system_warm_up_scheduled = true;

        let text_system = self.text_system.clone();
        let background_executor = self.background_executor.clone();
        self.background_executor
            .spawn_with_priority(Priority::Low, async move {
                background_executor
                    .timer(STARTUP_TEXT_SYSTEM_WARM_UP_DELAY)
                    .await;
                text_system.warm_up_background();
            })
            .detach();
    }

    /// Updates the application-wide default font family and synchronizes existing windows.
    pub fn set_default_font(&mut self, config: DefaultFontConfig) -> Result<()> {
        let mut fonts = Vec::new();
        let mut font_paths = Vec::new();
        let preload_family = config
            .sources
            .iter()
            .any(|source| !matches!(source, FontSource::SystemFamily(_)));
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
        if preload_family {
            self.text_system
                .preload_font_family(config.family.clone())?;
        }
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

    /// Check whether a global of the given type has been assigned.
    pub fn has_global<G: Global>(&self) -> bool {
        self.globals_by_type.contains_key(&TypeId::of::<G>())
    }

    /// Access the global of the given type. Panics if a global for that type has not been assigned.
    #[track_caller]
    pub fn global<G: Global>(&self) -> &G {
        self.globals_by_type
            .get(&TypeId::of::<G>())
            .map(|any_state| any_state.downcast_ref::<G>().unwrap())
            .with_context(|| format!("no state of type {} exists", type_name::<G>()))
            .unwrap()
    }

    /// Access the global of the given type if a value has been assigned.
    pub fn try_global<G: Global>(&self) -> Option<&G> {
        self.globals_by_type
            .get(&TypeId::of::<G>())
            .map(|any_state| any_state.downcast_ref::<G>().unwrap())
    }

    /// Access the global of the given type mutably. Panics if a global for that type has not been assigned.
    #[track_caller]
    pub fn global_mut<G: Global>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type
            .get_mut(&global_type)
            .and_then(|any_state| any_state.downcast_mut::<G>())
            .with_context(|| format!("no state of type {} exists", type_name::<G>()))
            .unwrap()
    }

    /// Access the global of the given type mutably. A default value is assigned if a global of this type has not
    /// yet been assigned.
    pub fn default_global<G: Global + Default>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type
            .entry(global_type)
            .or_insert_with(|| Box::<G>::default())
            .downcast_mut::<G>()
            .unwrap()
    }

    /// Sets the value of the global of the given type.
    pub fn set_global<G: Global>(&mut self, global: G) {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, Box::new(global));
    }

    /// Clear all stored globals. Does not notify global observers.
    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_globals(&mut self) {
        self.globals_by_type.drain();
    }

    /// Remove the global of the given type from the app context. Does not notify global observers.
    pub fn remove_global<G: Global>(&mut self) -> G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        *self
            .globals_by_type
            .remove(&global_type)
            .unwrap_or_else(|| panic!("no global added for {}", std::any::type_name::<G>()))
            .downcast()
            .unwrap()
    }

    /// Register a callback to be invoked when a global of the given type is updated.
    pub fn observe_global<G: Global>(
        &mut self,
        mut f: impl FnMut(&mut Self) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                f(cx);
                true
            }),
        );
        self.defer(move |_| activate());
        subscription
    }

    /// Move the global of the given type to the stack.
    #[track_caller]
    pub(crate) fn lease_global<G: Global>(&mut self) -> GlobalLease<G> {
        GlobalLease::new(
            self.globals_by_type
                .remove(&TypeId::of::<G>())
                .with_context(|| format!("no global registered of type {}", type_name::<G>()))
                .unwrap(),
        )
    }

    /// Restore the global of the given type after it is moved to the stack.
    pub(crate) fn end_global_lease<G: Global>(&mut self, lease: GlobalLease<G>) {
        let global_type = TypeId::of::<G>();

        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        self.globals_by_type.insert(global_type, lease.global);
    }

    pub(crate) fn new_entity_observer(
        &self,
        key: TypeId,
        value: NewEntityListener,
    ) -> Subscription {
        let (subscription, activate) = self.new_entity_observers.insert(key, value);
        activate();
        subscription
    }

    /// Arrange for the given function to be invoked whenever a view of the specified type is created.
    /// The function will be passed a mutable reference to the view along with an appropriate context.
    pub fn observe_new<T: 'static>(
        &self,
        on_new: impl 'static + Fn(&mut T, Option<&mut Window>, &mut Context<T>),
    ) -> Subscription {
        self.new_entity_observer(
            TypeId::of::<T>(),
            Box::new(
                move |any_entity: AnyEntity, window: &mut Option<&mut Window>, cx: &mut App| {
                    any_entity
                        .downcast::<T>()
                        .unwrap()
                        .update(cx, |entity_state, cx| {
                            on_new(entity_state, window.as_deref_mut(), cx)
                        })
                },
            ),
        )
    }

    /// Observe the release of a entity. The callback is invoked after the entity
    /// has no more strong references but before it has been dropped.
    pub fn observe_release<T>(
        &self,
        handle: &Entity<T>,
        on_release: impl FnOnce(&mut T, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let (subscription, activate) = self.release_listeners.insert(
            handle.entity_id(),
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                on_release(entity, cx)
            }),
        );
        activate();
        subscription
    }

    /// Observe the release of a entity. The callback is invoked after the entity
    /// has no more strong references but before it has been dropped.
    pub fn observe_release_in<T>(
        &self,
        handle: &Entity<T>,
        window: &Window,
        on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let window_handle = window.handle;
        self.observe_release(handle, move |entity, cx| {
            let _ = window_handle.update(cx, |_, window, cx| on_release(entity, window, cx));
        })
    }

    /// Register a callback to be invoked when a keystroke is received by the application
    /// in any window. Note that this fires after all other action and event mechanisms have resolved
    /// and that this API will not be invoked if the event's propagation is stopped.
    pub fn observe_keystrokes(
        &mut self,
        mut f: impl FnMut(&KeystrokeEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        fn inner(
            keystroke_observers: &SubscriberSet<(), KeystrokeObserver>,
            handler: KeystrokeObserver,
        ) -> Subscription {
            let (subscription, activate) = keystroke_observers.insert((), handler);
            activate();
            subscription
        }

        inner(
            &self.keystroke_observers,
            Box::new(move |event, window, cx| {
                f(event, window, cx);
                true
            }),
        )
    }

    /// Register a callback to be invoked when a keystroke is received by the application
    /// in any window. Note that this fires _before_ all other action and event mechanisms have resolved
    /// unlike [`App::observe_keystrokes`] which fires after. This means that `cx.stop_propagation` calls
    /// within interceptors will prevent action dispatch
    pub fn intercept_keystrokes(
        &mut self,
        mut f: impl FnMut(&KeystrokeEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        fn inner(
            keystroke_interceptors: &SubscriberSet<(), KeystrokeObserver>,
            handler: KeystrokeObserver,
        ) -> Subscription {
            let (subscription, activate) = keystroke_interceptors.insert((), handler);
            activate();
            subscription
        }

        inner(
            &self.keystroke_interceptors,
            Box::new(move |event, window, cx| {
                f(event, window, cx);
                true
            }),
        )
    }

    /// Register key bindings.
    pub fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        self.keymap.borrow_mut().add_bindings(bindings);
        self.refresh_windows();
    }

    /// Clear all key bindings in the app.
    pub fn clear_key_bindings(&mut self) {
        self.keymap.borrow_mut().clear();
        self.refresh_windows();
    }

    /// Get all key bindings in the app.
    pub fn key_bindings(&self) -> Rc<RefCell<Keymap>> {
        self.keymap.clone()
    }

    /// Register a global handler for actions invoked via the keyboard. These handlers are run at
    /// the end of the bubble phase for actions, and so will only be invoked if there are no other
    /// handlers or if they called `cx.propagate()`.
    pub fn on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Self) + 'static) {
        self.global_action_listeners
            .entry(TypeId::of::<A>())
            .or_default()
            .push(Rc::new(move |action, phase, cx| {
                if phase == DispatchPhase::Bubble {
                    let action = action.downcast_ref().unwrap();
                    listener(action, cx)
                }
            }));
    }

    /// Event handlers propagate events by default. Call this method to stop dispatching to
    /// event handlers with a lower z-index (mouse) or higher in the tree (keyboard). This is
    /// the opposite of [`Self::propagate`]. It's also possible to cancel a call to [`Self::propagate`] by
    /// calling this method before effects are flushed.
    pub fn stop_propagation(&mut self) {
        self.propagate_event = false;
    }

    /// Action handlers stop propagation by default during the bubble phase of action dispatch
    /// dispatching to action handlers higher in the element tree. This is the opposite of
    /// [`Self::stop_propagation`]. It's also possible to cancel a call to [`Self::stop_propagation`] by calling
    /// this method before effects are flushed.
    pub fn propagate(&mut self) {
        self.propagate_event = true;
    }

    /// Build an action from some arbitrary data, typically a keymap entry.
    pub fn build_action(
        &self,
        name: &str,
        data: Option<serde_json::Value>,
    ) -> std::result::Result<Box<dyn Action>, ActionBuildError> {
        self.actions.build_action(name, data)
    }

    /// Get all action names that have been registered. Note that registration only allows for
    /// actions to be built dynamically, and is unrelated to binding actions in the element tree.
    pub fn all_action_names(&self) -> &[&'static str] {
        self.actions.all_action_names()
    }

    /// Returns key bindings that invoke the given action on the currently focused element, without
    /// checking context. Bindings are returned in the order they were added. For display, the last
    /// binding should take precedence.
    pub fn all_bindings_for_input(&self, input: &[Keystroke]) -> Vec<KeyBinding> {
        RefCell::borrow(&self.keymap).all_bindings_for_input(input)
    }

    /// Get all non-internal actions that have been registered, along with their schemas.
    pub fn action_schemas(
        &self,
        generator: &mut schemars::SchemaGenerator,
    ) -> Vec<(&'static str, Option<schemars::Schema>)> {
        self.actions.action_schemas(generator)
    }

    /// Get a map from a deprecated action name to the canonical name.
    pub fn deprecated_actions_to_preferred_actions(&self) -> &HashMap<&'static str, &'static str> {
        self.actions.deprecated_aliases()
    }

    /// Get a map from an action name to the deprecation messages.
    pub fn action_deprecation_messages(&self) -> &HashMap<&'static str, &'static str> {
        self.actions.deprecation_messages()
    }

    /// Get a map from an action name to the documentation.
    pub fn action_documentation(&self) -> &HashMap<&'static str, &'static str> {
        self.actions.documentation()
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

    pub(crate) fn clear_pending_keystrokes(&mut self) {
        for window in self.windows() {
            window
                .update(self, |_, window, _| {
                    window.clear_pending_keystrokes();
                })
                .ok();
        }
    }

    /// Checks if the given action is bound in the current context, as defined by the app's current focus,
    /// the bindings in the element tree, and any global action listeners.
    pub fn is_action_available(&mut self, action: &dyn Action) -> bool {
        let mut action_available = false;
        if let Some(window) = self.active_window()
            && let Ok(window_action_available) =
                window.update(self, |_, window, cx| window.is_action_available(action, cx))
        {
            action_available = window_action_available;
        }

        action_available
            || self
                .global_action_listeners
                .contains_key(&action.as_any().type_id())
    }

    /// Sets the menu bar for this application. This will replace any existing menu bar.
    pub fn set_menus(&self, menus: Vec<Menu>) {
        self.platform.set_menus(menus, &self.keymap.borrow());
    }

    /// Gets the menu bar for this application.
    pub fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.platform.get_menus()
    }

    /// Sets the right click menu for the app icon in the dock
    pub fn set_dock_menu(&self, menus: Vec<MenuItem>) {
        self.platform.set_dock_menu(menus, &self.keymap.borrow())
    }

    /// Performs the action associated with the given dock menu item, only used on Windows for now.
    pub fn perform_dock_menu_action(&self, action: usize) {
        self.platform.perform_dock_menu_action(action);
    }

    /// Adds given path to the bottom of the list of recent paths for the application.
    /// The list is usually shown on the application icon's context menu in the dock,
    /// and allows to open the recent files via that context menu.
    /// If the path is already in the list, it will be moved to the bottom of the list.
    pub fn add_recent_document(&self, path: &Path) {
        self.platform.add_recent_document(path);
    }

    /// Updates the jump list with the updated list of recent paths for the application, only used on Windows for now.
    /// Note that this also sets the dock menu on Windows.
    pub fn update_jump_list(
        &self,
        menus: Vec<MenuItem>,
        entries: Vec<SmallVec<[PathBuf; 2]>>,
    ) -> Vec<SmallVec<[PathBuf; 2]>> {
        self.platform.update_jump_list(menus, entries)
    }

    /// Dispatch an action to the currently active window or global action handler
    /// See [`crate::Action`] for more information on how actions work
    pub fn dispatch_action(&mut self, action: &dyn Action) {
        if let Some(active_window) = self.active_window() {
            active_window
                .update(self, |_, window, cx| {
                    window.dispatch_action(action.boxed_clone(), cx)
                })
                .log_err();
        } else {
            self.dispatch_global_action(action);
        }
    }

    fn dispatch_global_action(&mut self, action: &dyn Action) {
        self.propagate_event = true;

        if let Some(mut global_listeners) = self
            .global_action_listeners
            .remove(&action.as_any().type_id())
        {
            for listener in &global_listeners {
                listener(action.as_any(), DispatchPhase::Capture, self);
                if !self.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                self.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            self.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }

        if self.propagate_event
            && let Some(mut global_listeners) = self
                .global_action_listeners
                .remove(&action.as_any().type_id())
        {
            for listener in global_listeners.iter().rev() {
                listener(action.as_any(), DispatchPhase::Bubble, self);
                if !self.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                self.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            self.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }
    }

    /// Is there currently something being dragged?
    pub fn has_active_drag(&self) -> bool {
        self.active_drag.is_some()
    }

    /// Gets the cursor style of the currently active drag operation.
    pub fn active_drag_cursor_style(&self) -> Option<CursorStyle> {
        self.active_drag.as_ref().and_then(|drag| drag.cursor_style)
    }

    /// Stops active drag and clears any related effects.
    pub fn stop_active_drag(&mut self, window: &mut Window) -> bool {
        if self.active_drag.is_some() {
            self.active_drag = None;
            window.refresh();
            true
        } else {
            false
        }
    }

    /// Sets the cursor style for the currently active drag operation.
    pub fn set_active_drag_cursor_style(
        &mut self,
        cursor_style: CursorStyle,
        window: &mut Window,
    ) -> bool {
        if let Some(ref mut drag) = self.active_drag {
            drag.cursor_style = Some(cursor_style);
            window.refresh();
            true
        } else {
            false
        }
    }

    /// Set the prompt renderer for GPUI. This will replace the default or platform specific
    /// prompts with this custom implementation.
    pub fn set_prompt_builder(
        &mut self,
        renderer: impl Fn(
            PromptLevel,
            &str,
            Option<&str>,
            &[PromptButton],
            PromptHandle,
            &mut Window,
            &mut App,
        ) -> RenderablePromptHandle
        + 'static,
    ) {
        self.prompt_builder = Some(PromptBuilder::Custom(Box::new(renderer)));
    }

    /// Reset the prompt builder to the default implementation.
    pub fn reset_prompt_builder(&mut self) {
        self.prompt_builder = Some(PromptBuilder::Default);
    }

    /// Remove an asset from GPUI's cache
    pub fn remove_asset<A: Asset>(&mut self, source: &A::Source) {
        self.take_asset::<A>(source);
    }

    /// Remove an asset from GPUI's cache and return its task if it exists.
    pub fn take_asset<A: Asset>(&mut self, source: &A::Source) -> Option<Shared<Task<A::Output>>> {
        let asset_id = (TypeId::of::<A>(), hash(source));
        self.loading_assets
            .remove(&asset_id)
            .map(|boxed_task| *boxed_task.downcast::<Shared<Task<A::Output>>>().unwrap())
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time, and the results of this call will be cached
    pub fn fetch_asset<A: Asset>(&mut self, source: &A::Source) -> (Shared<Task<A::Output>>, bool) {
        let asset_id = (TypeId::of::<A>(), hash(source));
        let mut is_first = false;
        let task = self
            .loading_assets
            .remove(&asset_id)
            .map(|boxed_task| *boxed_task.downcast::<Shared<Task<A::Output>>>().unwrap())
            .unwrap_or_else(|| {
                is_first = true;
                let future = A::load(source.clone(), self);
                self.background_executor()
                    .spawn_with_priority(Priority::Low, future)
                    .shared()
            });

        self.loading_assets.insert(asset_id, Box::new(task.clone()));

        (task, is_first)
    }

    /// Starts loading resource images into GPUI's global image asset cache without requiring a
    /// window. This is intended for idle warmup of images that are likely to be rendered soon.
    pub fn preload_image_resources(
        &mut self,
        sources: impl IntoIterator<Item = Resource>,
    ) -> Vec<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>> {
        sources
            .into_iter()
            .map(|source| self.fetch_asset::<crate::ImgResourceLoader>(&source).0)
            .collect()
    }

    /// Returns retained image asset totals from GPUI's global asset cache.
    pub fn global_image_asset_cache_snapshot(&self) -> GlobalImageAssetCacheSnapshot {
        let mut snapshot = GlobalImageAssetCacheSnapshot::default();
        let resource_type = TypeId::of::<crate::ImgResourceLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<ImageDecoder>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<EncodedImageDecoder>>();
        let compressed_type = TypeId::of::<CompressedImgResourceLoader>();
        let target_type = TypeId::of::<TargetSizeImgResourceLoader>();

        for ((type_id, _), task) in &self.loading_assets {
            if *type_id == resource_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.resource_count = snapshot.resource_count.saturating_add(1);
                    snapshot.resource_decoded_bytes = snapshot
                        .resource_decoded_bytes
                        .saturating_add(image.decoded_byte_len());
                }
            } else if *type_id == inline_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.inline_count = snapshot.inline_count.saturating_add(1);
                    snapshot.inline_decoded_bytes = snapshot
                        .inline_decoded_bytes
                        .saturating_add(image.decoded_byte_len());
                }
            } else if *type_id == inline_bytes_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                    && let Some(Ok(image)) = task.clone().now_or_never()
                {
                    snapshot.inline_count = snapshot.inline_count.saturating_add(1);
                    snapshot.inline_decoded_bytes = snapshot
                        .inline_decoded_bytes
                        .saturating_add(image.decoded_byte_len());
                }
            } else if *type_id == compressed_type {
                if let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<[u8]>, ImageCacheError>>>>()
                    && let Some(Ok(bytes)) = task.clone().now_or_never()
                {
                    snapshot.compressed_count = snapshot.compressed_count.saturating_add(1);
                    snapshot.compressed_bytes =
                        snapshot.compressed_bytes.saturating_add(bytes.len());
                }
            } else if *type_id == target_type
                && let Some(task) =
                    task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                && let Some(Ok(image)) = task.clone().now_or_never()
            {
                snapshot.target_count = snapshot.target_count.saturating_add(1);
                snapshot.target_decoded_bytes = snapshot
                    .target_decoded_bytes
                    .saturating_add(image.decoded_byte_len());
            }
        }

        snapshot
    }

    /// Obtain a new [`FocusHandle`], which allows you to track and manipulate the keyboard focus
    /// for elements rendered within this window.
    #[track_caller]
    pub fn focus_handle(&self) -> FocusHandle {
        FocusHandle::new(&self.focus_handles)
    }

    /// Tell GPUI that an entity has changed and observers of it should be notified.
    pub fn notify(&mut self, entity_id: EntityId) {
        let window_invalidators = mem::take(
            self.window_invalidators_by_entity
                .entry(entity_id)
                .or_default(),
        );

        if window_invalidators.is_empty() {
            if self.pending_notifications.insert(entity_id) {
                if log::log_enabled!(log::Level::Debug) {
                    log::debug!(
                        "gpui notify queued: entity={} type={}",
                        entity_id.as_u64(),
                        self.entity_type_name(entity_id)
                    );
                }
                self.pending_effects
                    .push_back(Effect::Notify { emitter: entity_id });
            }
        } else {
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "gpui notify invalidated windows: entity={} type={} invalidators={}",
                    entity_id.as_u64(),
                    self.entity_type_name(entity_id),
                    window_invalidators.len()
                );
            }
            for invalidator in window_invalidators.values() {
                invalidator.invalidate_view(entity_id, self);
            }
        }

        self.window_invalidators_by_entity
            .insert(entity_id, window_invalidators);
    }

    pub(crate) fn entity_type_name(&self, entity_id: EntityId) -> &'static str {
        self.entities
            .type_name_for_id(entity_id)
            .unwrap_or("(dropped)")
    }

    /// Returns the name for this [`App`].
    #[cfg(any(test, feature = "test-support", debug_assertions))]
    pub fn get_name(&self) -> Option<&'static str> {
        self.name
    }

    /// Returns `true` if the platform file picker supports selecting a mix of files and directories.
    pub fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.platform.can_select_mixed_files_and_dirs()
    }

    /// Removes an image from the sprite atlas on all windows.
    ///
    /// If the current window is being updated, it will be removed from `App.windows`, you can use `current_window` to specify the current window.
    /// This is a no-op if the image is not in the sprite atlas.
    pub fn drop_image(&mut self, image: Arc<RenderImage>, current_window: Option<&mut Window>) {
        // remove the texture from all other windows
        for window in self.windows.values_mut().flatten() {
            _ = window.drop_image(image.clone());
        }

        // remove the texture from the current window
        if let Some(window) = current_window {
            _ = window.drop_image(image);
        }
    }

    /// Returns the image pipeline configuration used by newly rendered image elements.
    pub fn image_pipeline_config(&self) -> ImagePipelineConfig {
        self.image_pipeline_config
    }

    /// Returns the framework-wide GPUI memory policy.
    pub fn gpui_memory_policy(&self) -> GpuiMemoryPolicy {
        self.gpui_memory_policy
    }

    /// Updates framework-wide GPUI memory retention limits.
    pub fn set_gpui_memory_policy(&mut self, policy: GpuiMemoryPolicy) {
        self.gpui_memory_policy = policy;
        for window in self.windows.values_mut().flatten() {
            window.set_gpui_memory_policy(policy);
        }
        self.default_image_cache_config = bounded_cache_config_from_policy(policy);
        if let Some(cache) = self.default_image_cache.clone() {
            cache.update(self, |cache, cx| {
                cache.set_config_without_window(cx.default_image_cache_config, cx);
            });
        }
    }

    /// Creates a framework-owned image usage scope.
    ///
    /// Pass `scope.scope()` to `img(...).usage_scope(...)`. When the final handle clone is dropped,
    /// GPUI releases image assets that are only retained by that scope.
    pub fn gpui_image_usage_scope(
        &self,
        scope: impl Into<GpuiImageUsageScope>,
        release_level: GpuiMemoryTrimLevel,
    ) -> GpuiImageUsageScopeHandle {
        GpuiImageUsageScopeHandle::new(
            self.this.clone(),
            self.foreground_executor.clone(),
            scope.into(),
            release_level,
        )
    }

    /// Records the usage classification for one image asset key.
    pub(crate) fn track_gpui_image_asset_usage(
        &mut self,
        asset_type: TypeId,
        asset_hash: u64,
        usage: ImageUsageKind,
        scope: Option<&GpuiImageUsageScope>,
    ) {
        let key = (asset_type, asset_hash);
        self.image_usage_kinds.insert(key, usage);
        if let Some(scope) = scope {
            self.image_usage_scopes
                .entry(scope.clone())
                .or_default()
                .insert(key);
            self.image_asset_scopes
                .entry(key)
                .or_default()
                .insert(scope.clone());
        }
    }

    /// Records that a target-size decoded image asset was used by the current frame.
    pub(crate) fn touch_gpui_target_image_asset(
        &mut self,
        asset_hash: u64,
        decoded_bytes: usize,
        usage: ImageUsageKind,
        scope: Option<&GpuiImageUsageScope>,
    ) {
        self.image_asset_touch_clock = self.image_asset_touch_clock.saturating_add(1);
        self.target_image_assets.insert(
            asset_hash,
            GpuiImageTargetAssetUsage {
                decoded_bytes,
                usage,
                scope: scope.cloned(),
                last_used: self.image_asset_touch_clock,
            },
        );
    }

    /// Removes target-size image bookkeeping after an asset has been explicitly dropped.
    pub(crate) fn forget_gpui_target_image_asset(&mut self, asset_hash: u64) {
        let asset_key = (TypeId::of::<TargetSizeImgResourceLoader>(), asset_hash);
        self.unlink_gpui_image_asset_key(asset_key);
        self.target_image_assets.remove(&asset_hash);
    }

    /// Ends one application-defined image usage scope and releases GPUI-owned resources in it.
    pub fn end_gpui_image_usage_scope(
        &mut self,
        scope: &GpuiImageUsageScope,
        level: GpuiMemoryTrimLevel,
    ) -> GpuiMemorySnapshot {
        let Some(scope_asset_keys) = self.image_usage_scopes.remove(scope) else {
            return self.gpui_memory_snapshot();
        };

        let mut releasable_asset_keys = Vec::new();
        for asset_key in scope_asset_keys {
            if let Some(scopes) = self.image_asset_scopes.get_mut(&asset_key) {
                scopes.remove(scope);
                if scopes.is_empty() {
                    self.image_asset_scopes.remove(&asset_key);
                    releasable_asset_keys.push(asset_key);
                }
            } else {
                releasable_asset_keys.push(asset_key);
            }
        }

        self.release_gpui_image_asset_keys(releasable_asset_keys);
        if !matches!(level, GpuiMemoryTrimLevel::Light) {
            self.enforce_gpui_target_image_asset_limits();
            for window in self.windows.values_mut().flatten() {
                window.trim_gpui_memory(level);
            }
        }

        self.gpui_memory_snapshot()
    }

    /// Returns the bounded cache used by image elements that do not provide an explicit cache.
    pub(crate) fn default_image_cache(&mut self) -> AnyImageCache {
        if self.default_image_cache.is_none() {
            self.default_image_cache = Some(BoundedImageCache::new(
                self.default_image_cache_config,
                self,
            ));
        }
        let cache = self
            .default_image_cache
            .as_ref()
            .expect("default image cache should be initialized")
            .clone();
        cache.into()
    }

    /// Returns a unified memory snapshot for GPUI-owned image and renderer resources.
    pub fn gpui_memory_snapshot(&self) -> GpuiMemorySnapshot {
        GpuiMemorySnapshot::from_metrics(
            &performance_metrics_snapshot(),
            self.global_image_asset_cache_snapshot(),
        )
    }

    /// Hints GPUI to release framework-owned image and renderer resources.
    pub fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) -> GpuiMemorySnapshot {
        if let Some(cache) = self.default_image_cache.clone() {
            cache.update(self, |cache, cx| match level {
                GpuiMemoryTrimLevel::Light => cache.enforce_current_limits(cx),
                GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => {
                    cache.clear_without_window(cx);
                }
            });
        }

        self.layout_id_buffer.clear();
        if !matches!(level, GpuiMemoryTrimLevel::Light) {
            self.layout_id_buffer.shrink_to(0);
        }
        self.text_system.trim_global_caches(level);

        let _ = trim_element_arena(match level {
            GpuiMemoryTrimLevel::Light => 256 * 1024,
            GpuiMemoryTrimLevel::Moderate => 128 * 1024,
            GpuiMemoryTrimLevel::Aggressive => 64 * 1024,
        });

        if !matches!(level, GpuiMemoryTrimLevel::Light) {
            self.trim_global_image_assets(level);
            self.enforce_gpui_target_image_asset_limits();
        }

        for window in self.windows.values_mut().flatten() {
            window.trim_gpui_memory(level);
        }

        self.gpui_memory_snapshot()
    }

    fn trim_global_image_assets(&mut self, level: GpuiMemoryTrimLevel) {
        let resource_type = TypeId::of::<crate::ImgResourceLoader>();
        let inline_type = TypeId::of::<crate::AssetLogger<ImageDecoder>>();
        let inline_bytes_type = TypeId::of::<crate::AssetLogger<EncodedImageDecoder>>();
        let target_type = TypeId::of::<TargetSizeImgResourceLoader>();
        let compressed_type = TypeId::of::<CompressedImgResourceLoader>();
        let keys = self
            .loading_assets
            .keys()
            .filter_map(|(type_id, asset_hash)| {
                let asset_key = (*type_id, *asset_hash);
                if self.image_asset_has_active_scope(asset_key) {
                    return None;
                }
                let should_drop = *type_id == resource_type
                    || *type_id == inline_type
                    || *type_id == inline_bytes_type
                    || *type_id == target_type
                    || (matches!(level, GpuiMemoryTrimLevel::Aggressive)
                        && *type_id == compressed_type);
                should_drop.then_some(asset_key)
            })
            .collect::<Vec<_>>();

        self.release_gpui_image_asset_keys(keys);
    }

    fn enforce_gpui_target_image_asset_limits(&mut self) {
        let target_type = TypeId::of::<TargetSizeImgResourceLoader>();
        let total_bytes = self
            .target_image_assets
            .values()
            .map(|usage| usage.decoded_bytes)
            .sum::<usize>();
        let preview_bytes = self
            .target_image_assets
            .values()
            .filter(|usage| usage.usage == ImageUsageKind::PreviewImage)
            .map(|usage| usage.decoded_bytes)
            .sum::<usize>();

        let mut release_keys = Vec::new();
        if total_bytes > self.gpui_memory_policy.image_cache_max_bytes {
            let mut reclaim_bytes =
                total_bytes.saturating_sub(self.gpui_memory_policy.image_cache_max_bytes);
            for (asset_hash, usage) in self.gpui_target_image_release_candidates(false) {
                if reclaim_bytes == 0 {
                    break;
                }
                reclaim_bytes = reclaim_bytes.saturating_sub(usage.decoded_bytes);
                release_keys.push((target_type, asset_hash));
            }
        }

        if preview_bytes > self.gpui_memory_policy.preview_cache_max_bytes {
            let mut reclaim_bytes =
                preview_bytes.saturating_sub(self.gpui_memory_policy.preview_cache_max_bytes);
            for (asset_hash, usage) in self.gpui_target_image_release_candidates(true) {
                if reclaim_bytes == 0 {
                    break;
                }
                if release_keys
                    .iter()
                    .any(|(_, release_hash)| *release_hash == asset_hash)
                {
                    continue;
                }
                reclaim_bytes = reclaim_bytes.saturating_sub(usage.decoded_bytes);
                release_keys.push((target_type, asset_hash));
            }
        }

        if !release_keys.is_empty() {
            self.release_gpui_image_asset_keys(release_keys);
        }
    }

    fn gpui_target_image_release_candidates(
        &self,
        preview_only: bool,
    ) -> Vec<(u64, GpuiImageTargetAssetUsage)> {
        let target_type = TypeId::of::<TargetSizeImgResourceLoader>();
        let mut candidates = self
            .target_image_assets
            .iter()
            .filter_map(|(asset_hash, usage)| {
                if preview_only && usage.usage != ImageUsageKind::PreviewImage {
                    return None;
                }
                if self.image_asset_has_active_scope((target_type, *asset_hash)) {
                    return None;
                }
                Some((*asset_hash, usage.clone()))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(_, usage)| usage.last_used);
        candidates
    }

    fn image_asset_has_active_scope(&self, asset_key: (TypeId, u64)) -> bool {
        self.image_asset_scopes
            .get(&asset_key)
            .is_some_and(|scopes| !scopes.is_empty())
    }

    fn release_gpui_image_asset_keys(
        &mut self,
        asset_keys: impl IntoIterator<Item = (TypeId, u64)>,
    ) {
        let mut retained_assets = Vec::new();
        let resource_type = TypeId::of::<crate::ImgResourceLoader>();
        let target_type = TypeId::of::<TargetSizeImgResourceLoader>();

        for asset_key in asset_keys {
            self.unlink_gpui_image_asset_key(asset_key);
            if asset_key.0 == target_type {
                self.target_image_assets.remove(&asset_key.1);
            }

            if asset_key.0 == resource_type
                && let Some(cache) = self.default_image_cache.clone()
            {
                cache.update(self, |cache, cx| cache.remove_hash(asset_key.1, None, cx));
            }

            if asset_key.0 == target_type {
                crate::drop_image_asset_retained(asset_key.1);
            }

            let Some(task) = self.loading_assets.remove(&asset_key) else {
                continue;
            };
            if let Some(task) =
                task.downcast_ref::<Shared<Task<Result<Arc<RenderImage>, ImageCacheError>>>>()
                && let Some(Ok(image)) = task.clone().now_or_never()
            {
                retained_assets.push(image);
            }
        }

        for image in retained_assets {
            self.drop_image(image, None);
        }
    }

    fn unlink_gpui_image_asset_key(&mut self, asset_key: (TypeId, u64)) {
        self.image_usage_kinds.remove(&asset_key);
        if let Some(scopes) = self.image_asset_scopes.remove(&asset_key) {
            for scope in scopes {
                let should_remove_scope =
                    if let Some(asset_keys) = self.image_usage_scopes.get_mut(&scope) {
                        asset_keys.remove(&asset_key);
                        asset_keys.is_empty()
                    } else {
                        false
                    };
                if should_remove_scope {
                    self.image_usage_scopes.remove(&scope);
                }
            }
        }
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

impl AppContext for App {
    type Result<T> = T;

    /// Builds an entity that is owned by the application.
    ///
    /// The given function will be invoked with a [`Context`] and must return an object representing the entity. An
    /// [`Entity`] handle will be returned, which can be used to access the entity in a context.
    fn new<T: 'static>(&mut self, build_entity: impl FnOnce(&mut Context<T>) -> T) -> Entity<T> {
        self.update(|cx| {
            let slot = cx.entities.reserve();
            let handle = slot.clone();
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));

            cx.push_effect(Effect::EntityCreated {
                entity: handle.clone().into_any(),
                tid: TypeId::of::<T>(),
                window: cx.window_update_stack.last().cloned(),
            });

            cx.entities.insert(slot, entity);
            handle
        })
    }

    fn reserve_entity<T: 'static>(&mut self) -> Self::Result<Reservation<T>> {
        Reservation(self.entities.reserve())
    }

    fn insert_entity<T: 'static>(
        &mut self,
        reservation: Reservation<T>,
        build_entity: impl FnOnce(&mut Context<T>) -> T,
    ) -> Self::Result<Entity<T>> {
        self.update(|cx| {
            let slot = reservation.0;
            let entity = build_entity(&mut Context::new_context(cx, slot.downgrade()));
            cx.entities.insert(slot, entity)
        })
    }

    /// Updates the entity referenced by the given handle. The function is passed a mutable reference to the
    /// entity along with a `Context` for the entity.
    fn update_entity<T: 'static, R>(
        &mut self,
        handle: &Entity<T>,
        update: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        self.update(|cx| {
            let mut entity = cx.entities.lease(handle);
            let result = update(
                &mut entity,
                &mut Context::new_context(cx, handle.downgrade()),
            );
            cx.entities.end_lease(entity);
            result
        })
    }

    fn as_mut<'a, T>(&'a mut self, handle: &Entity<T>) -> GpuiBorrow<'a, T>
    where
        T: 'static,
    {
        GpuiBorrow::new(handle.clone(), self)
    }

    fn read_entity<T, R>(
        &self,
        handle: &Entity<T>,
        read: impl FnOnce(&T, &App) -> R,
    ) -> Self::Result<R>
    where
        T: 'static,
    {
        let entity = self.entities.read(handle);
        read(entity, self)
    }

    fn update_window<T, F>(&mut self, handle: AnyWindowHandle, update: F) -> Result<T>
    where
        F: FnOnce(AnyView, &mut Window, &mut App) -> T,
    {
        self.update_window_id(handle.id, update)
    }

    fn read_window<T, R>(
        &self,
        window: &WindowHandle<T>,
        read: impl FnOnce(Entity<T>, &App) -> R,
    ) -> Result<R>
    where
        T: 'static,
    {
        let window = self
            .windows
            .get(window.id)
            .context("window not found")?
            .as_deref()
            .expect("attempted to read a window that is already on the stack");

        let root_view = window.root.clone().unwrap();
        let view = root_view
            .downcast::<T>()
            .map_err(|_| anyhow!("root view's type has changed"))?;

        Ok(read(view, self))
    }

    fn background_spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
    where
        R: Send + 'static,
    {
        self.background_executor.spawn(future)
    }

    fn read_global<G, R>(&self, callback: impl FnOnce(&G, &App) -> R) -> Self::Result<R>
    where
        G: Global,
    {
        let mut g = self.global::<G>();
        callback(g, self)
    }
}

/// These effects are processed at the end of each application update cycle.
pub(crate) enum Effect {
    Notify {
        emitter: EntityId,
    },
    Emit {
        emitter: EntityId,
        event_type: TypeId,
        event: Box<dyn Any>,
    },
    RefreshWindows,
    NotifyGlobalObservers {
        global_type: TypeId,
    },
    Defer {
        callback: Box<dyn FnOnce(&mut App) + 'static>,
    },
    EntityCreated {
        entity: AnyEntity,
        tid: TypeId,
        window: Option<WindowId>,
    },
}

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effect::Notify { emitter } => write!(f, "Notify({})", emitter),
            Effect::Emit { emitter, .. } => write!(f, "Emit({:?})", emitter),
            Effect::RefreshWindows => write!(f, "RefreshWindows"),
            Effect::NotifyGlobalObservers { global_type } => {
                write!(f, "NotifyGlobalObservers({:?})", global_type)
            }
            Effect::Defer { .. } => write!(f, "Defer(..)"),
            Effect::EntityCreated { entity, .. } => write!(f, "EntityCreated({:?})", entity),
        }
    }
}

/// Wraps a global variable value during `update_global` while the value has been moved to the stack.
pub(crate) struct GlobalLease<G: Global> {
    global: Box<dyn Any>,
    global_type: PhantomData<G>,
}

impl<G: Global> GlobalLease<G> {
    fn new(global: Box<dyn Any>) -> Self {
        GlobalLease {
            global,
            global_type: PhantomData,
        }
    }
}

impl<G: Global> Deref for GlobalLease<G> {
    type Target = G;

    fn deref(&self) -> &Self::Target {
        self.global.downcast_ref().unwrap()
    }
}

impl<G: Global> DerefMut for GlobalLease<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.global.downcast_mut().unwrap()
    }
}

/// Contains state associated with an active drag operation, started by dragging an element
/// within the window or by dragging into the app from the underlying platform.
pub struct AnyDrag {
    /// The view used to render this drag
    pub view: AnyView,

    /// The value of the dragged item, to be dropped
    pub value: Arc<dyn Any>,

    /// This is used to render the dragged item in the same place
    /// on the original element that the drag was initiated
    pub cursor_offset: Point<Pixels>,

    /// The cursor style to use while dragging
    pub cursor_style: Option<CursorStyle>,
}

/// Contains state associated with a tooltip. You'll only need this struct if you're implementing
/// tooltip behavior on a custom element. Otherwise, use [Div::tooltip](crate::Interactivity::tooltip).
#[derive(Clone)]
pub struct AnyTooltip {
    /// The view used to display the tooltip
    pub view: AnyView,

    /// The absolute position of the mouse when the tooltip was deployed.
    pub mouse_position: Point<Pixels>,

    /// Given the bounds of the tooltip, checks whether the tooltip should still be visible and
    /// updates its state accordingly. This is needed atop the hovered element's mouse move handler
    /// to handle the case where the element is not painted (e.g. via use of `visible_on_hover`).
    pub check_visible_and_update: Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App) -> bool>,
}

/// A keystroke event, and potentially the associated action
#[derive(Debug)]
pub struct KeystrokeEvent {
    /// The keystroke that occurred
    pub keystroke: Keystroke,

    /// The action that was resolved for the keystroke, if any
    pub action: Option<Box<dyn Action>>,

    /// The context stack at the time
    pub context_stack: Vec<KeyContext>,
}

struct NullHttpClient;

impl HttpClient for NullHttpClient {
    fn send(
        &self,
        _req: http_client::Request<http_client::AsyncBody>,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<http_client::Response<http_client::AsyncBody>>,
    > {
        async move {
            anyhow::bail!("No HttpClient available");
        }
        .boxed()
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }
}

/// A mutable reference to an entity owned by GPUI
pub struct GpuiBorrow<'a, T> {
    inner: Option<Lease<T>>,
    app: &'a mut App,
}

impl<'a, T: 'static> GpuiBorrow<'a, T> {
    fn new(inner: Entity<T>, app: &'a mut App) -> Self {
        app.start_update();
        let lease = app.entities.lease(&inner);
        Self {
            inner: Some(lease),
            app,
        }
    }
}

impl<'a, T: 'static> std::borrow::Borrow<T> for GpuiBorrow<'a, T> {
    fn borrow(&self) -> &T {
        self.inner.as_ref().unwrap().borrow()
    }
}

impl<'a, T: 'static> std::borrow::BorrowMut<T> for GpuiBorrow<'a, T> {
    fn borrow_mut(&mut self) -> &mut T {
        self.inner.as_mut().unwrap().borrow_mut()
    }
}

impl<'a, T: 'static> std::ops::Deref for GpuiBorrow<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}

impl<'a, T: 'static> std::ops::DerefMut for GpuiBorrow<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.as_mut().unwrap()
    }
}

impl<'a, T> Drop for GpuiBorrow<'a, T> {
    fn drop(&mut self) {
        let lease = self.inner.take().unwrap();
        self.app.notify(lease.id);
        self.app.entities.end_lease(lease);
        self.app.finish_update();
    }
}

#[cfg(test)]
mod test {
    use std::{cell::RefCell, rc::Rc};

    use crate::{AppContext, TestAppContext};

    #[test]
    fn test_gpui_borrow() {
        let cx = TestAppContext::single();
        let observation_count = Rc::new(RefCell::new(0));

        let state = cx.update(|cx| {
            let state = cx.new(|_| false);
            cx.observe(&state, {
                let observation_count = observation_count.clone();
                move |_, _| {
                    let mut count = observation_count.borrow_mut();
                    *count += 1;
                }
            })
            .detach();

            state
        });

        cx.update(|cx| {
            // Calling this like this so that we don't clobber the borrow_mut above
            *std::borrow::BorrowMut::borrow_mut(&mut state.as_mut(cx)) = true;
        });

        cx.update(|cx| {
            state.write(cx, false);
        });

        assert_eq!(*observation_count.borrow(), 2);
    }

    #[test]
    fn same_entity_notify_from_observer_is_coalesced_within_flush() {
        let cx = TestAppContext::single();
        let observation_count = Rc::new(RefCell::new(0));

        let state = cx.update(|cx| {
            let state = cx.new(|_| false);
            cx.observe(&state, {
                let observation_count = observation_count.clone();
                move |entity, cx| {
                    *observation_count.borrow_mut() += 1;
                    cx.notify(entity.entity_id());
                }
            })
            .detach();

            state
        });

        cx.update(|cx| {
            cx.notify(state.entity_id());
        });

        assert_eq!(*observation_count.borrow(), 1);

        cx.update(|cx| {
            cx.notify(state.entity_id());
        });

        assert_eq!(*observation_count.borrow(), 2);
    }

    #[test]
    fn foreground_effect_flush_yields_after_batch_limit() {
        let cx = TestAppContext::single();
        let processed_effects = Rc::new(RefCell::new(0usize));
        let before_yields = crate::performance_metrics_snapshot().foreground_effect_yield_count;

        cx.update(|cx| {
            for _ in 0..=super::FOREGROUND_EFFECT_BATCH_LIMIT {
                let processed_effects = processed_effects.clone();
                cx.defer(move |_| {
                    *processed_effects.borrow_mut() += 1;
                });
            }
        });

        let processed_after_first_flush = *processed_effects.borrow();
        assert!(processed_after_first_flush > 0);
        assert!(processed_after_first_flush <= super::FOREGROUND_EFFECT_BATCH_LIMIT);
        assert!(processed_after_first_flush < super::FOREGROUND_EFFECT_BATCH_LIMIT + 1);
        let snapshot = crate::performance_metrics_snapshot();
        assert_eq!(snapshot.foreground_effect_yield_count, before_yields + 1);
        assert!(snapshot.foreground_effect_pending_max >= 1);

        cx.background_executor.run_until_parked();

        assert_eq!(
            *processed_effects.borrow(),
            super::FOREGROUND_EFFECT_BATCH_LIMIT + 1
        );
    }
}

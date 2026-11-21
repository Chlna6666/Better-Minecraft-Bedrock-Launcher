//! BMCBL 插件 API（组件模型 / WIT 契约版，API 1.0.0）。
//!
//! 本 crate 是插件与启动器之间的契约层：
//! - `wit/plugin.wit` 定义 WIT 世界 `bmcbl:plugin/plugin-world@1.0.0`，是唯一的 ABI 契约；
//! - guest 侧绑定由 `wit-bindgen` 在构建期生成（目标 `wasm32-wasip2`，cargo 直接产出
//!   组件），本 crate 不再包含任何手写编解码、内存管理或 host-call 胶水；
//! - 保留人机工程层：`Plugin` trait、`Registrations`/`Page`/`Injection` 构建器、`view!`
//!   宏、toast/log/tr 宏与 `prelude`，它们只是生成绑定之上的便利 API；
//! - `.bmcblx` 打包在 feature `pack` 下（`pack.rs`）。
//!
//! 边界：只负责契约与打包；不依赖 GPUI/应用代码。host 能力由宿主运行时
//! （`bmcbl_plugin_runtime` + 应用侧 host context）做 capability/allowlist 校验。

pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "plugin-world",
        // 让生成的 `export!` 宏为 pub，插件作者才能通过 `export_plugin!` /
        // `#[bmcbl_plugin]` 跨 crate 调用。
        pub_export_macro: true,
    });
}

pub use bmcbl_plugin_macros::{bmcbl_plugin, plugin_metadata};

pub mod app;
pub mod native;

/// Calls native Bedrock DLLs installed once under BMCBL/plugins.
///
/// Requests are resolved by the launcher's game session and require
/// `game-mod.call` capability plus per-version SHA-256 approval.
/// Guest code cannot supply paths, guest PIDs or foreign plugin IDs.
pub mod game_mod {
    use crate::PluginResult;
    use serde::{Serialize, Deserialize};

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Event {
        pub module_id: String,
        pub name: String,
        pub body: Vec<u8>,
    }

    /// Drain the caller's bounded native event queue (max 32 events per game).
    pub fn read_events() -> PluginResult<super::app::Job<Vec<Event>>> {
        super::app::invoke("game-mod.read-events", &())
    }

    use crate::app::{Job, invoke};

    /// Submit a bounded (32 KiB) asynchronous call to a Mod declared in this
    /// plugin's `[[game_mods]]`. Returns a job resolved by `JobReady`.
    /// Calls are unavailable if multiple concurrent games make the target
    /// ambiguous, or if the plugin generation has been revoked.
    pub fn call(module_id: &str, method: &str, payload: &[u8]) -> PluginResult<Job<Vec<u8>>> {
        invoke("game-mod.call", &(module_id, method, payload))
    }
}

/// Reads from the installed plugin package.
pub mod filesystem {
    pub use crate::{read_resource_bytes, read_resource_text};
}

/// Requests through BMCBL's HTTP client and download manager.
pub mod network {
    pub use crate::{
        DownloadRequest, HttpCacheState, HttpTextResponse, download_resource,
        http_get_text as get_text,
    };
}

/// Operations running on BMCBL's async runtime.
pub mod r#async {
    pub use crate::app::Job;
}

/// Integration with BMCBL's task manager.
pub mod tasks {
    pub use crate::{
        TaskCreateRequest, TaskDefinitionRequest, TaskFinishRequest, TaskUpdateRequest, create_task,
        create_typed_task, finish_task, register_task_definition, update_task,
    };
}

/// Native Bedrock world queries.
pub use app::world;

/// Pages rendered by BMCBL's native GPUI runtime.
pub mod ui {
    pub use crate::{
        Page, Tool, View, ViewTree, ViewTreeBuilder, invalidate_page, navigate, open_modal,
        open_window, show_toast, view,
    };
}

#[cfg(feature = "pack")]
pub mod pack;

use bindings::bmcbl::plugin::types;

pub use bindings::exports::bmcbl::plugin::plugin::Guest;

pub use types::{
    ActionEvent, Align, AppInfo, BadgeNode, ButtonNode, CheckboxNode, CompactBehavior,
    ContainerNode, DownloadRequest, EventSubscription, GlobalEvent, HttpCacheState,
    HttpTextResponse, I18nArg, IconNode, ImageFit, ImageNode, InjectionLayout, InjectionRegistration,
    InjectionRequest, InjectionSlot, InputNode, InvalidateTarget, LayoutDirection, LinkNode,
    ListNode, LogLevel, ModalRequest, ToolEntry, PageRegistration, PageRenderRequest,
    PluginContext, PluginError, PluginEvent, PluginEventKind, ProgressNode, Registration,
    RouteChangedEvent, RouteTarget, SelectNode, SelectOption, ServiceCallRequest,
    ServiceRegistration, ShutdownReason,
    SpacerNode, TaskCreateRequest, TaskDefinitionRequest, TaskFinishRequest, TaskUpdateRequest,
    TextNode, TextSizeToken, ThemeColor, ThemeMode, ThemeSnapshot, ThemeToken, ToastKind,
    ToggleNode, ViewNode, ViewStyle, ViewTree, WindowRequest,
};

pub use bindings::bmcbl::plugin::{host_clipboard, host_config, host_downloads, host_events,
    host_log, host_network, host_resources, host_services, host_storage, host_system,
    host_tasks, host_toast, host_ui};

pub const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Version of the package manifest layout.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// 插件实现 trait（静态方法，单实例语义：插件用自身静态/线程局部状态保存数据）。
pub trait Plugin {
    fn init(context: PluginContext) -> PluginResult<Vec<Registration>>;

    fn handle_event(_event: PluginEvent) -> PluginResult<()> {
        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree>;

    fn render_injection(_request: InjectionRequest) -> PluginResult<Option<ViewTree>> {
        Ok(None)
    }

    fn call_service(_request: ServiceCallRequest) -> PluginResult<Vec<u8>> {
        Err(PluginError::new(
            "service-not-found",
            "service call not implemented by this plugin",
        ))
    }

    fn shutdown(_reason: ShutdownReason) -> PluginResult<()> {
        Ok(())
    }
}

/// 把 [`Plugin`] 实现桥接到 wit-bindgen 生成的组件导出。
///
/// 插件作者通过 `#[bmcbl_plugin]` 或 `export_plugin!(MyPlugin)` 导出。
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        #[doc(hidden)]
        struct __BmcblPluginBridge;

        impl $crate::Guest for __BmcblPluginBridge {
            fn init(
                context: $crate::PluginContext,
            ) -> ::core::result::Result<
                ::std::vec::Vec<$crate::Registration>,
                $crate::PluginError,
            > {
                <$plugin as $crate::Plugin>::init(context)
            }

            fn handle_event(
                event: $crate::PluginEvent,
            ) -> ::core::result::Result<(), $crate::PluginError> {
                <$plugin as $crate::Plugin>::handle_event(event)
            }

            fn render_page(
                request: $crate::PageRenderRequest,
            ) -> ::core::result::Result<$crate::ViewTree, $crate::PluginError> {
                <$plugin as $crate::Plugin>::render_page(request)
            }

            fn render_injection(
                request: $crate::InjectionRequest,
            ) -> ::core::result::Result<
                ::core::option::Option<$crate::ViewTree>,
                $crate::PluginError,
            > {
                <$plugin as $crate::Plugin>::render_injection(request)
            }

            fn call_service(
                request: $crate::ServiceCallRequest,
            ) -> ::core::result::Result<::std::vec::Vec<u8>, $crate::PluginError> {
                <$plugin as $crate::Plugin>::call_service(request)
            }

            fn shutdown(
                reason: $crate::ShutdownReason,
            ) -> ::core::result::Result<(), $crate::PluginError> {
                <$plugin as $crate::Plugin>::shutdown(reason)
            }
        }

        $crate::bindings::export!(__BmcblPluginBridge with_types_in $crate::bindings);
    };
}

impl PluginError {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new("denied", message)
    }

    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid-input", message)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not-found", message)
    }

    /// 由启动器侧产生的错误（例如宿主调用失败）。
    #[must_use]
    pub fn app(message: impl Into<String>) -> Self {
        Self::new("app", message)
    }

    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new("timeout", message)
    }
}

pub type PluginResult<T> = Result<T, PluginError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PluginMetadata {
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub authors: &'static [&'static str],
    pub description: &'static str,
    pub website: &'static str,
    pub license: &'static str,
    pub tags: &'static [&'static str],
    pub capabilities: &'static [&'static str],
}

impl PluginMetadata {
    #[must_use]
    pub fn authors_display(self) -> String {
        self.authors.join(", ")
    }

    #[must_use]
    pub fn capabilities_display(self) -> String {
        self.capabilities.join(", ")
    }

    #[must_use]
    pub fn window(self, page_id: impl Into<String>) -> Window {
        Window::new(page_id)
    }

    pub fn open_window(self, page_id: impl Into<String>) -> PluginResult<u64> {
        self.window(page_id).open(self)
    }

    #[must_use]
    pub fn modal(self, page_id: impl Into<String>) -> Modal {
        Modal::new(page_id)
    }

    pub fn open_modal(self, page_id: impl Into<String>) -> PluginResult<()> {
        self.modal(page_id).open(self)
    }

    pub fn navigate_page(self, page_id: impl Into<String>) -> PluginResult<()> {
        navigate_plugin(self.id, page_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginAction {
    String(&'static str),
}

impl PluginAction {
    #[must_use]
    pub const fn new(action_id: &'static str) -> Self {
        Self::String(action_id)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String(action_id) => action_id,
        }
    }
}

impl From<PluginAction> for String {
    fn from(action: PluginAction) -> Self {
        action.as_str().to_string()
    }
}

impl PluginEvent {
    #[must_use]
    pub fn action_id(&self) -> Option<&str> {
        match &self.kind {
            PluginEventKind::Action(action) => Some(action.action_id.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn action_is(&self, action_id: impl AsRef<str>) -> bool {
        self.action_id()
            .is_some_and(|current| current == action_id.as_ref())
    }

    #[must_use]
    pub fn route_path(&self) -> Option<&str> {
        match &self.kind {
            PluginEventKind::RouteChanged(route) => Some(route.path.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn global_event(&self) -> Option<(&str, &str)> {
        match &self.kind {
            PluginEventKind::Global(event) => Some((event.name.as_str(), event.payload.as_str())),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 宿主调用封装：每个函数直接转调 wit-bindgen 生成的导入，错误类型就是 WIT
// `plugin-error` 对应的生成类型，无需任何编解码/重试逻辑。
// ─────────────────────────────────────────────────────────────────────────────

pub fn log(level: LogLevel, message: impl AsRef<str>) {
    host_log::log(level, message.as_ref());
}

pub fn log_debug(message: impl AsRef<str>) {
    log(LogLevel::Debug, message);
}

pub fn log_info(message: impl AsRef<str>) {
    log(LogLevel::Info, message);
}

pub fn log_warn(message: impl AsRef<str>) {
    log(LogLevel::Warn, message);
}

pub fn log_error(message: impl AsRef<str>) {
    log(LogLevel::Error, message);
}

pub fn show_toast(kind: ToastKind, message: impl AsRef<str>) -> PluginResult<()> {
    host_toast::show_toast(kind, message.as_ref())
}

pub fn open_window(request: &WindowRequest) -> PluginResult<u64> {
    host_ui::open_window(request)
}

pub fn open_modal(request: &ModalRequest) -> PluginResult<()> {
    host_ui::open_modal(request)
}

pub fn navigate(target: &RouteTarget) -> PluginResult<()> {
    host_ui::navigate(target)
}

pub fn navigate_plugin(
    plugin_id: impl Into<String>,
    page_id: impl Into<String>,
) -> PluginResult<()> {
    navigate(&RouteTarget {
        plugin_id: Some(plugin_id.into()),
        page_id: Some(page_id.into()),
        path: String::new(),
    })
}

pub fn navigate_page(plugin: PluginMetadata, page_id: impl Into<String>) -> PluginResult<()> {
    navigate_plugin(plugin.id, page_id)
}

pub fn navigate_path(path: impl Into<String>) -> PluginResult<()> {
    navigate(&RouteTarget {
        plugin_id: None,
        page_id: None,
        path: path.into(),
    })
}

pub fn emit_event(name: impl AsRef<str>, payload: impl AsRef<str>) -> PluginResult<()> {
    host_events::emit_event(name.as_ref(), payload.as_ref())
}

pub fn invalidate(target: InvalidateTarget) -> PluginResult<()> {
    host_events::invalidate(&target)
}

pub fn invalidate_all() -> PluginResult<()> {
    invalidate(InvalidateTarget::All)
}

pub fn invalidate_page(page_id: impl Into<String>) -> PluginResult<()> {
    invalidate(InvalidateTarget::Page(page_id.into()))
}

pub fn invalidate_injection(
    slot: InjectionSlot,
    page: Option<impl Into<String>>,
) -> PluginResult<()> {
    invalidate(InvalidateTarget::Injection(InjectionRequest {
        slot,
        page: page.map(Into::into),
    }))
}

#[must_use]
pub fn current_locale() -> String {
    host_ui::current_locale()
}

#[must_use]
pub fn tr(key: impl AsRef<str>) -> String {
    tr_args(key, &[])
}

#[must_use]
pub fn tr_args(key: impl AsRef<str>, args: &[I18nArg]) -> String {
    host_ui::translate(key.as_ref(), args)
}

#[must_use]
pub fn tr_arg(key: impl Into<String>, value: impl Into<String>) -> I18nArg {
    I18nArg {
        key: key.into(),
        value: value.into(),
    }
}

/// Reads plugin configuration through the launcher.
///
/// This performs launcher filesystem I/O and is rejected while rendering plugin UI. Read it during
/// initialization or event handling and retain render-facing values in session/plugin state.
pub fn read_config() -> PluginResult<String> {
    host_config::read()
}

pub fn write_config(text: impl AsRef<str>) -> PluginResult<()> {
    host_config::write(text.as_ref())
}

pub fn http_get_text(
    url: impl AsRef<str>,
    ttl_seconds: u32,
    max_bytes: u32,
) -> PluginResult<HttpTextResponse> {
    host_network::get_text(url.as_ref(), ttl_seconds, max_bytes)
}

pub fn write_clipboard_text(text: impl AsRef<str>) -> PluginResult<()> {
    host_clipboard::write_text(text.as_ref())
}

pub fn read_clipboard_text() -> PluginResult<Option<String>> {
    host_clipboard::read_text()
}

pub fn current_unix_ms() -> PluginResult<u64> {
    Ok(host_system::current_unix_ms())
}

pub fn open_external_url(url: impl AsRef<str>) -> PluginResult<()> {
    host_system::open_external_url(url.as_ref())
}

/// Reads a declared/allowed text resource through the launcher resource cache.
///
/// Exact-file allowlist entries are prewarmed during plugin reload. Other cache misses schedule a
/// background read and return a `resource-loading` error; render callers are invalidated
/// automatically when the resource becomes ready.
pub fn read_resource_text(path: impl AsRef<str>) -> PluginResult<String> {
    host_resources::read_text(path.as_ref())
}

/// Reads a declared/allowed binary resource through the launcher resource cache.
pub fn read_resource_bytes(path: impl AsRef<str>) -> PluginResult<Vec<u8>> {
    host_resources::read_bytes(path.as_ref())
}

pub fn session_get(key: impl AsRef<str>) -> PluginResult<Option<String>> {
    Ok(host_storage::session_get(key.as_ref()))
}

pub fn session_set(key: impl AsRef<str>, value: Option<impl AsRef<str>>) -> PluginResult<()> {
    let value = value.map(|value| value.as_ref().to_string());
    host_storage::session_set(key.as_ref(), value.as_deref());
    Ok(())
}

/// Reads the launcher-prepared persistent plugin KV snapshot.
///
/// Reads are memory-only and safe while rendering plugin UI.
pub fn storage_get(key: impl AsRef<str>) -> PluginResult<Option<String>> {
    host_storage::kv_get(key.as_ref())
}

/// Updates persistent plugin KV storage.
///
/// The launcher validates quota and updates the in-memory snapshot immediately, then queues ordered
/// background persistence. Mutating storage is rejected while rendering plugin UI.
pub fn storage_set(key: impl AsRef<str>, value: impl AsRef<str>) -> PluginResult<()> {
    host_storage::kv_set(key.as_ref(), value.as_ref())
}

/// Deletes a persistent plugin KV value.
pub fn storage_delete(key: impl AsRef<str>) -> PluginResult<()> {
    host_storage::kv_delete(key.as_ref())
}

/// Lists keys from the launcher-prepared persistent plugin KV snapshot.
pub fn storage_list(prefix: Option<impl AsRef<str>>) -> PluginResult<Vec<String>> {
    let prefix = prefix.map(|prefix| prefix.as_ref().to_string());
    host_storage::kv_list(prefix.as_deref())
}

pub fn create_task(request: TaskCreateRequest) -> PluginResult<String> {
    host_tasks::create(&request)
}

pub fn update_task(request: TaskUpdateRequest) -> PluginResult<()> {
    host_tasks::update(&request)
}

pub fn finish_task(request: TaskFinishRequest) -> PluginResult<()> {
    host_tasks::finish(&request)
}

/// Registers a task type owned by this plugin. The type id must start with the
/// plugin's own id (`<plugin_id>.<name>`); the host rejects other namespaces.
pub fn register_task_definition(request: TaskDefinitionRequest) -> PluginResult<()> {
    host_tasks::register_definition(&request)
}

/// Creates a task linked to a registered definition owned by this plugin.
pub fn create_typed_task(
    request: TaskCreateRequest,
    definition: impl Into<String>,
) -> PluginResult<String> {
    let definition = definition.into();
    host_tasks::create_typed(&request, &definition)
}

pub fn app_info() -> PluginResult<AppInfo> {
    Ok(host_system::get_app_info())
}

pub fn register_service(name: impl Into<String>) -> PluginResult<()> {
    host_services::register(&name.into())
}

pub fn call_service(
    service_name: impl Into<String>,
    method: impl Into<String>,
    payload: impl Into<Vec<u8>>,
) -> PluginResult<Vec<u8>> {
    call_plugin_service(None, service_name, method, payload)
}

pub fn call_plugin_service(
    target_plugin: Option<String>,
    service_name: impl Into<String>,
    method: impl Into<String>,
    payload: impl Into<Vec<u8>>,
) -> PluginResult<Vec<u8>> {
    let request = ServiceCallRequest {
        target_plugin,
        service_name: service_name.into(),
        method: method.into(),
        payload: payload.into(),
    };
    host_services::call(&request)
}

pub fn theme_snapshot() -> PluginResult<ThemeSnapshot> {
    Ok(host_ui::get_theme_snapshot())
}

#[must_use]
pub fn is_dark_mode() -> bool {
    theme_snapshot()
        .map(ThemeSnapshot::is_dark)
        .unwrap_or(false)
}

#[must_use]
pub fn theme_accent() -> Option<ThemeColor> {
    host_ui::theme_accent()
}

/// Schedules a download using the launcher's task manager.
pub fn download_resource(request: DownloadRequest) -> PluginResult<String> {
    host_downloads::start(&request)
}

pub fn plugin_error(code: impl Into<String>, message: impl Into<String>) -> PluginError {
    PluginError::new(code, message)
}

impl ThemeSnapshot {
    #[must_use]
    pub const fn light_default() -> Self {
        Self {
            mode: ThemeMode::Light,
            dark_factor: 0.0,
            accent: ThemeColor {
                h: 0.60,
                s: 0.88,
                l: 0.60,
                a: 1.0,
            },
            primary_text: ThemeColor {
                h: 0.61,
                s: 0.30,
                l: 0.20,
                a: 1.0,
            },
            secondary_text: ThemeColor {
                h: 0.60,
                s: 0.16,
                l: 0.45,
                a: 1.0,
            },
            muted_text: ThemeColor {
                h: 0.59,
                s: 0.14,
                l: 0.61,
                a: 1.0,
            },
            surface: ThemeColor {
                h: 0.58,
                s: 0.24,
                l: 0.96,
                a: 0.86,
            },
            surface_hover: ThemeColor {
                h: 0.58,
                s: 0.18,
                l: 0.93,
                a: 0.90,
            },
            border: ThemeColor {
                h: 0.60,
                s: 0.20,
                l: 0.84,
                a: 0.72,
            },
            danger: ThemeColor {
                h: 0.06,
                s: 0.93,
                l: 0.53,
                a: 1.0,
            },
        }
    }

    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self.mode, ThemeMode::Dark)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 注册构建器
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Registrations {
    items: Vec<Registration>,
}

impl Registrations {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn page(mut self, page: Page) -> Self {
        self.items.push(Registration::Page(page.finish()));
        self
    }

    #[must_use]
    pub fn injection(mut self, injection: Injection) -> Self {
        self.items.push(Registration::Injection(injection.finish()));
        self
    }

    #[must_use]
    pub fn subscribe(mut self, event: impl Into<String>) -> Self {
        self.items
            .push(Registration::Subscription(EventSubscription {
                event: event.into(),
            }));
        self
    }

    #[must_use]
    pub fn service(mut self, service_name: impl Into<String>) -> Self {
        self.items.push(Registration::Service(ServiceRegistration {
            service_name: service_name.into(),
        }));
        self
    }

    #[must_use]
    pub fn register(mut self, registration: impl IntoRegistration) -> Self {
        self.items.push(registration.into_registration());
        self
    }

    #[must_use]
    pub fn extend(mut self, registrations: impl IntoIterator<Item = Registration>) -> Self {
        self.items.extend(registrations);
        self
    }

    #[must_use]
    pub fn finish(self) -> Vec<Registration> {
        self.items
    }
}

pub trait IntoRegistration {
    fn into_registration(self) -> Registration;
}

impl IntoRegistration for Registration {
    fn into_registration(self) -> Registration {
        self
    }
}

impl IntoRegistration for Page {
    fn into_registration(self) -> Registration {
        Registration::Page(self.finish())
    }
}

impl IntoRegistration for Injection {
    fn into_registration(self) -> Registration {
        Registration::Injection(self.finish())
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    page_id: String,
    title: String,
    tool: Option<ToolEntry>,
}

impl Page {
    #[must_use]
    pub fn new(page_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: title.into(),
            tool: None,
        }
    }

    #[must_use]
    pub fn tool(mut self, tool: Tool) -> Self {
        self.tool = Some(tool.finish());
        self
    }

    #[must_use]
    pub fn finish(self) -> PageRegistration {
        PageRegistration {
            page_id: self.page_id,
            title: self.title,
            tool: self.tool,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tool {
    label: String,
    icon: Option<String>,
    order: i32,
}

impl Tool {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            order: 0,
        }
    }

    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    #[must_use]
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    #[must_use]
    pub fn finish(self) -> ToolEntry {
        ToolEntry {
            label: self.label,
            icon: self.icon,
            order: self.order,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Injection {
    slot: InjectionSlot,
    page: Option<String>,
    priority: i32,
    layout: Option<InjectionLayout>,
}

impl Injection {
    #[must_use]
    pub fn new(slot: InjectionSlot) -> Self {
        Self {
            slot,
            page: None,
            priority: 0,
            layout: None,
        }
    }

    #[must_use]
    pub fn page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    #[must_use]
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub fn layout(mut self, layout: InjectionLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    #[must_use]
    pub fn finish(self) -> InjectionRegistration {
        InjectionRegistration {
            slot: self.slot,
            page: self.page,
            priority: self.priority,
            layout: self.layout,
        }
    }
}

impl InjectionLayout {
    #[must_use]
    pub const fn sidebar() -> Self {
        Self {
            preferred_width: None,
            min_width: None,
            max_width: None,
            max_height: None,
            priority: 0,
            compact_behavior: CompactBehavior::None,
        }
    }

    #[must_use]
    pub const fn width(mut self, width: u16) -> Self {
        self.preferred_width = Some(width);
        self
    }

    #[must_use]
    pub const fn min_width(mut self, width: u16) -> Self {
        self.min_width = Some(width);
        self
    }

    #[must_use]
    pub const fn max_width(mut self, width: u16) -> Self {
        self.max_width = Some(width);
        self
    }

    #[must_use]
    pub const fn max_height(mut self, height: u16) -> Self {
        self.max_height = Some(height);
        self
    }

    #[must_use]
    pub const fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub const fn compact_behavior(mut self, behavior: CompactBehavior) -> Self {
        self.compact_behavior = behavior;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Window {
    page_id: String,
    title: Option<String>,
    width: u32,
    height: u32,
    resizable: bool,
}

impl Window {
    #[must_use]
    pub fn new(page_id: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: None,
            width: 780,
            height: 520,
            resizable: true,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    pub fn open(self, plugin: PluginMetadata) -> PluginResult<u64> {
        open_window(&WindowRequest {
            title: self.title.unwrap_or_else(|| plugin.name.to_string()),
            plugin_id: plugin.id.to_string(),
            page_id: self.page_id,
            width: self.width,
            height: self.height,
            resizable: self.resizable,
        })
    }
}

pub struct Modal {
    page_id: String,
    title: Option<String>,
    width: u32,
    height: u32,
}

impl Modal {
    #[must_use]
    pub fn new(page_id: impl Into<String>) -> Self {
        Self {
            page_id: page_id.into(),
            title: None,
            width: 760,
            height: 560,
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn open(self, plugin: PluginMetadata) -> PluginResult<()> {
        open_modal(&ModalRequest {
            title: self.title.unwrap_or_else(|| plugin.name.to_string()),
            plugin_id: plugin.id.to_string(),
            page_id: self.page_id,
            width: self.width,
            height: self.height,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// UI DSL 构建层
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct View {
    node: ViewNode,
    children: Vec<View>,
}

impl View {
    #[must_use]
    pub fn node(node: ViewNode) -> Self {
        Self {
            node,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn column() -> Container {
        Container::new(LayoutDirection::Column)
    }

    #[must_use]
    pub fn row() -> Container {
        Container::new(LayoutDirection::Row)
    }

    /// A primary column and a secondary column which stack in a compact window.
    #[must_use]
    pub fn columns() -> Container {
        Self::row().align(Align::Stretch).full_width()
    }

    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::node(crate::text(value))
    }

    #[must_use]
    pub fn title(text: impl Into<String>) -> Self {
        Self::node(title(text))
    }

    #[must_use]
    pub fn badge(label: impl Into<String>) -> Self {
        Self::node(badge(label))
    }

    #[must_use]
    pub fn badge_styled(label: impl Into<String>, style: ViewStyle) -> Self {
        Self::node(badge_styled(label, style))
    }

    #[must_use]
    pub fn button(label: impl Into<String>, action_id: impl Into<String>) -> Self {
        Self::node(button(label, action_id))
    }

    #[must_use]
    pub fn secondary_button(label: impl Into<String>, action_id: impl Into<String>) -> Self {
        let mut node = button(label, action_id);
        if let ViewNode::Button(button) = &mut node {
            button.style.color = Some(ThemeToken::SecondaryText);
        }
        Self::node(node)
    }

    /// Dispatches the current text as the action value when the user edits the input.
    #[must_use]
    pub fn input(
        value: impl Into<String>,
        placeholder: impl Into<String>,
        action_id: impl Into<String>,
    ) -> Self {
        Self::node(ViewNode::Input(InputNode {
            value: value.into(),
            placeholder: placeholder.into(),
            action_id: action_id.into(),
            action_value: None,
            style: ViewStyle { full_width: true, ..default_style() },
        }))
    }

    #[must_use]
    pub fn button_action(
        label: impl Into<String>,
        action_id: impl Into<String>,
        action_value: impl Into<String>,
    ) -> Self {
        Self::node(button_action(label, action_id, action_value))
    }

    #[must_use]
    pub fn checkbox(
        label: impl Into<String>,
        checked: bool,
        action_id: impl Into<String>,
    ) -> Self {
        Self::node(checkbox(label, checked, action_id))
    }

    #[must_use]
    pub fn toggle(
        label: impl Into<String>,
        enabled: bool,
        action_id: impl Into<String>,
    ) -> Self {
        Self::node(toggle(label, enabled, action_id))
    }

    #[must_use]
    pub fn select(
        label: impl Into<String>,
        action_id: impl Into<String>,
        options: impl IntoIterator<Item = SelectOption>,
        selected: Option<impl Into<String>>,
    ) -> Self {
        Self::node(select(label, action_id, options, selected))
    }

    #[must_use]
    pub fn progress(label: impl Into<String>, value: u64, total: Option<u64>) -> Self {
        Self::node(progress(label, value, total))
    }

    #[must_use]
    pub fn link(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self::node(link(label, url))
    }

    #[must_use]
    pub fn image(src: impl Into<String>, alt: impl Into<String>) -> Self {
        Self::node(image(src, alt))
    }

    #[must_use]
    pub fn image_options(
        src: impl Into<String>,
        alt: impl Into<String>,
        options: ImageOptions,
    ) -> Self {
        Self::node(image_options(src, alt, options))
    }

    #[must_use]
    pub fn finish(self) -> ViewTree {
        ViewTreeBuilder::new().finish(self)
    }
}

impl From<ViewNode> for View {
    fn from(root: ViewNode) -> Self {
        Self::node(root)
    }
}

impl From<Container> for View {
    fn from(container: Container) -> Self {
        container.finish_view()
    }
}

#[derive(Clone, Debug)]
pub struct Container {
    style: ViewStyle,
    children: Vec<View>,
}

impl Container {
    #[must_use]
    pub fn new(direction: LayoutDirection) -> Self {
        Self {
            style: ViewStyle {
                direction,
                padding: 24,
                gap: 12,
                ..default_style()
            },
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn padding(mut self, padding: u16) -> Self {
        self.style.padding = padding;
        self
    }

    #[must_use]
    pub fn gap(mut self, gap: u16) -> Self {
        self.style.gap = gap;
        self
    }

    #[must_use]
    pub fn align(mut self, align: Align) -> Self {
        self.style.align = align;
        self
    }

    #[must_use]
    pub fn background(mut self, token: ThemeToken) -> Self {
        self.style.background = Some(token);
        self
    }

    #[must_use]
    pub fn full_width(mut self) -> Self {
        self.style.full_width = true;
        self
    }

    #[must_use]
    pub fn corner_radius(mut self, radius: u16) -> Self {
        self.style.corner_radius = Some(radius);
        self
    }

    #[must_use]
    pub fn child(mut self, child: impl Into<View>) -> Self {
        self.children.push(child.into());
        self
    }

    #[must_use]
    pub fn finish_view(self) -> View {
        View {
            node: ViewNode::Container(ContainerNode {
                style: self.style,
                children: Vec::new(),
            }),
            children: self.children,
        }
    }

    #[must_use]
    pub fn finish(self) -> ViewTree {
        self.finish_view().finish()
    }
}

/// [`ImageNode`] 的可选项构建器（caption/placeholder/fallback/尺寸等）。
#[derive(Clone, Debug)]
pub struct ImageOptions {
    caption: String,
    placeholder: String,
    fallback: String,
    height: Option<u16>,
    min_height: Option<u16>,
    max_height: Option<u16>,
    aspect_ratio_x: Option<u16>,
    aspect_ratio_y: Option<u16>,
    corner_radius: Option<u16>,
    fit: ImageFit,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            caption: String::new(),
            placeholder: String::new(),
            fallback: String::new(),
            height: None,
            min_height: None,
            max_height: None,
            aspect_ratio_x: None,
            aspect_ratio_y: None,
            corner_radius: None,
            fit: ImageFit::Cover,
        }
    }
}

impl ImageOptions {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = caption.into();
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    #[must_use]
    pub fn fallback(mut self, fallback: impl Into<String>) -> Self {
        self.fallback = fallback.into();
        self
    }

    #[must_use]
    pub fn height(mut self, height: u16) -> Self {
        self.height = Some(height);
        self
    }

    #[must_use]
    pub fn min_height(mut self, min_height: u16) -> Self {
        self.min_height = Some(min_height);
        self
    }

    #[must_use]
    pub fn max_height(mut self, max_height: u16) -> Self {
        self.max_height = Some(max_height);
        self
    }

    #[must_use]
    pub fn aspect_ratio(mut self, width: u16, height: u16) -> Self {
        self.aspect_ratio_x = Some(width);
        self.aspect_ratio_y = Some(height);
        self
    }

    #[must_use]
    pub fn corner_radius(mut self, corner_radius: u16) -> Self {
        self.corner_radius = Some(corner_radius);
        self
    }

    #[must_use]
    pub fn fit(mut self, fit: ImageFit) -> Self {
        self.fit = fit;
        self
    }
}

#[must_use]
pub fn card(children: impl IntoIterator<Item = View>) -> View {
    let mut container = View::column()
        .padding(14)
        .gap(10)
        .background(ThemeToken::Surface)
        .corner_radius(8)
        .full_width();
    for child in children {
        container = container.child(child);
    }
    container.finish_view()
}

#[must_use]
pub fn section(title_text: impl Into<String>, children: impl IntoIterator<Item = View>) -> View {
    let mut container = View::column()
        .padding(0)
        .gap(10)
        .full_width()
        .child(View::title(title_text));
    for child in children {
        container = container.child(child);
    }
    container.finish_view()
}

#[derive(Clone, Debug, Default)]
pub struct ViewTreeBuilder {
    nodes: Vec<ViewNode>,
}

impl ViewTreeBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn finish(mut self, root: View) -> ViewTree {
        let root = self.push_recursive(root);
        ViewTree {
            root,
            nodes: self.nodes,
        }
    }

    fn push_recursive(&mut self, view: View) -> u32 {
        let index = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.nodes.push(view.node);
        let child_indexes = view
            .children
            .into_iter()
            .map(|child| self.push_recursive(child))
            .collect::<Vec<_>>();

        if let Ok(node_index) = usize::try_from(index) {
            match &mut self.nodes[node_index] {
                ViewNode::Container(container) => {
                    container.children = child_indexes;
                }
                ViewNode::ItemList(list) => {
                    list.items = child_indexes;
                }
                ViewNode::Text(_)
                | ViewNode::Button(_)
                | ViewNode::Input(_)
                | ViewNode::Checkbox(_)
                | ViewNode::Toggle(_)
                | ViewNode::Select(_)
                | ViewNode::Progress(_)
                | ViewNode::Link(_)
                | ViewNode::Separator
                | ViewNode::Badge(_)
                | ViewNode::Icon(_)
                | ViewNode::Image(_)
                | ViewNode::Spacer(_) => {}
            }
        }

        index
    }
}

#[must_use]
pub fn default_style() -> ViewStyle {
    ViewStyle {
        direction: LayoutDirection::Column,
        gap: 8,
        padding: 0,
        align: Align::Start,
        color: None,
        background: None,
        text_size: TextSizeToken::Body,
        emphasis: false,
        full_width: false,
        corner_radius: None,
    }
}

#[must_use]
pub fn title_style() -> ViewStyle {
    ViewStyle {
        text_size: TextSizeToken::Title,
        emphasis: true,
        ..default_style()
    }
}

#[must_use]
pub fn accent_button_style() -> ViewStyle {
    ViewStyle {
        color: Some(ThemeToken::Accent),
        emphasis: true,
        corner_radius: Some(8),
        ..default_style()
    }
}

#[must_use]
pub fn container(children: Vec<u32>) -> ViewNode {
    ViewNode::Container(ContainerNode {
        style: ViewStyle {
            padding: 24,
            gap: 12,
            ..default_style()
        },
        children,
    })
}

#[must_use]
pub fn text(text: impl Into<String>) -> ViewNode {
    ViewNode::Text(TextNode {
        text: text.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn title(text: impl Into<String>) -> ViewNode {
    ViewNode::Text(TextNode {
        text: text.into(),
        style: title_style(),
    })
}

#[must_use]
pub fn badge(label: impl Into<String>) -> ViewNode {
    ViewNode::Badge(BadgeNode {
        label: label.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn badge_styled(label: impl Into<String>, style: ViewStyle) -> ViewNode {
    ViewNode::Badge(BadgeNode {
        label: label.into(),
        style,
    })
}

#[must_use]
pub fn button(label: impl Into<String>, action_id: impl Into<String>) -> ViewNode {
    ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action_id.into(),
        action_value: None,
        style: accent_button_style(),
    })
}

#[must_use]
pub fn button_action(
    label: impl Into<String>,
    action_id: impl Into<String>,
    action_value: impl Into<String>,
) -> ViewNode {
    ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action_id.into(),
        action_value: Some(action_value.into()),
        style: accent_button_style(),
    })
}

#[must_use]
pub fn checkbox(
    label: impl Into<String>,
    checked: bool,
    action_id: impl Into<String>,
) -> ViewNode {
    ViewNode::Checkbox(CheckboxNode {
        label: label.into(),
        checked,
        action_id: action_id.into(),
        action_value: Some(if checked { "false" } else { "true" }.to_string()),
        style: default_style(),
    })
}

#[must_use]
pub fn toggle(
    label: impl Into<String>,
    enabled: bool,
    action_id: impl Into<String>,
) -> ViewNode {
    ViewNode::Toggle(ToggleNode {
        label: label.into(),
        enabled,
        action_id: action_id.into(),
        action_value: Some(if enabled { "false" } else { "true" }.to_string()),
        style: default_style(),
    })
}

#[must_use]
pub fn select(
    label: impl Into<String>,
    action_id: impl Into<String>,
    options: impl IntoIterator<Item = SelectOption>,
    selected: Option<impl Into<String>>,
) -> ViewNode {
    ViewNode::Select(SelectNode {
        label: label.into(),
        action_id: action_id.into(),
        options: options.into_iter().collect(),
        selected: selected.map(Into::into),
        style: default_style(),
    })
}

#[must_use]
pub fn option(label: impl Into<String>, value: impl Into<String>) -> SelectOption {
    SelectOption {
        label: label.into(),
        value: value.into(),
    }
}

#[must_use]
pub fn progress(label: impl Into<String>, value: u64, total: Option<u64>) -> ViewNode {
    ViewNode::Progress(ProgressNode {
        label: label.into(),
        value,
        total,
        style: default_style(),
    })
}

#[must_use]
pub fn link(label: impl Into<String>, url: impl Into<String>) -> ViewNode {
    ViewNode::Link(LinkNode {
        label: label.into(),
        url: url.into(),
        tooltip: None,
        style: ViewStyle {
            color: Some(ThemeToken::Accent),
            ..default_style()
        },
    })
}

#[must_use]
pub fn link_tooltip(
    label: impl Into<String>,
    url: impl Into<String>,
    tooltip: impl Into<String>,
) -> ViewNode {
    ViewNode::Link(LinkNode {
        label: label.into(),
        url: url.into(),
        tooltip: Some(tooltip.into()),
        style: ViewStyle {
            color: Some(ThemeToken::Accent),
            ..default_style()
        },
    })
}

#[must_use]
pub fn icon(name: impl Into<String>) -> ViewNode {
    ViewNode::Icon(IconNode {
        name: name.into(),
        style: default_style(),
    })
}

#[must_use]
pub fn image(src: impl Into<String>, alt: impl Into<String>) -> ViewNode {
    ViewNode::Image(ImageNode {
        src: src.into(),
        alt: alt.into(),
        caption: String::new(),
        placeholder: String::new(),
        fallback: String::new(),
        style: default_style(),
        height: None,
        min_height: None,
        max_height: None,
        aspect_ratio_x: None,
        aspect_ratio_y: None,
        corner_radius: None,
        fit: ImageFit::Cover,
    })
}

#[must_use]
pub fn image_options(
    src: impl Into<String>,
    alt: impl Into<String>,
    options: ImageOptions,
) -> ViewNode {
    ViewNode::Image(ImageNode {
        src: src.into(),
        alt: alt.into(),
        caption: options.caption,
        placeholder: options.placeholder,
        fallback: options.fallback,
        style: default_style(),
        height: options.height,
        min_height: options.min_height,
        max_height: options.max_height,
        aspect_ratio_x: options.aspect_ratio_x,
        aspect_ratio_y: options.aspect_ratio_y,
        corner_radius: options.corner_radius,
        fit: options.fit,
    })
}

#[must_use]
pub fn spacer(size: u16) -> ViewNode {
    ViewNode::Spacer(SpacerNode { size })
}

// ─────────────────────────────────────────────────────────────────────────────
// 宏
// ─────────────────────────────────────────────────────────────────────────────

#[macro_export]
macro_rules! plugin_actions {
    (
        $(#[$meta:meta])*
        $visibility:vis enum $name:ident {
            $($variant:ident = $action_id:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $visibility enum $name {
            $($variant),*
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $action_id),*
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

#[macro_export]
macro_rules! registrations {
    () => {
        $crate::Registrations::new().finish()
    };
    (page $page_id:expr, title = $title:expr, tool = $tool:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .page($crate::Page::new($page_id, $title).tool($tool))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (page $page_id:expr, title = $title:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .page($crate::Page::new($page_id, $title))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, priority = $priority:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).priority($priority))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, page = $page:expr, priority = $priority:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).page($page).priority($priority))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, page = $page:expr, priority = $priority:expr, layout = $layout:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).page($page).priority($priority).layout($layout))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (injection $slot:expr, priority = $priority:expr, layout = $layout:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .injection($crate::Injection::new($slot).priority($priority).layout($layout))
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (subscribe $event:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .subscribe($event)
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    (service $name:expr; $($rest:tt)*) => {
        $crate::Registrations::new()
            .service($name)
            .extend($crate::registrations!($($rest)*))
            .finish()
    };
    ($($entry:expr),* $(,)?) => {
        $crate::Registrations::new()
            $(.register($entry))*
            .finish()
    };
}

#[macro_export]
macro_rules! view {
    (column($($name:ident = $value:expr),* $(,)?) { $($child:expr);* $(;)? }) => {{
        let view = $crate::View::column()
            $(.$name($value))*
            $(.child($child))*
            .finish();
        view
    }};
    (row($($name:ident = $value:expr),* $(,)?) { $($child:expr);* $(;)? }) => {{
        let view = $crate::View::row()
            $(.$name($value))*
            $(.child($child))*
            .finish();
        view
    }};
    ($node:expr) => {
        $crate::View::from($node).finish()
    };
}

#[macro_export]
macro_rules! toast {
    (info, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Info, $message)
    };
    (success, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Success, $message)
    };
    (error, $message:expr) => {
        $crate::show_toast($crate::ToastKind::Error, $message)
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::log_debug(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log_info(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log_warn(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log_error(format!($($arg)*))
    };
}

#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::tr($key)
    };
    ($key:expr, $( $arg_key:expr => $arg_value:expr ),+ $(,)?) => {{
        let args = vec![$($crate::tr_arg($arg_key, format!("{}", $arg_value))),+];
        $crate::tr_args($key, &args)
    }};
}

#[macro_export]
macro_rules! invalidate {
    (all) => {
        $crate::invalidate_all()
    };
    (page $page_id:expr) => {
        $crate::invalidate_page($page_id)
    };
    (injection $slot:expr) => {
        $crate::invalidate_injection($slot, Option::<String>::None)
    };
    (injection $slot:expr, page = $page:expr) => {
        $crate::invalidate_injection($slot, Some($page))
    };
}

pub mod prelude {
    pub use crate::{
        Align, AppInfo, CompactBehavior, Container, DownloadRequest, EventSubscription,
        HttpCacheState, HttpTextResponse, I18nArg, ImageFit, ImageOptions, Injection,
        InjectionLayout, InjectionRegistration, InjectionRequest, InjectionSlot, LogLevel, Modal,
        ModalRequest, Tool, Page, PageRegistration, PageRenderRequest, Plugin, PluginAction,
        PluginContext, PluginError, PluginEvent, PluginEventKind, PluginMetadata, PluginResult,
        Registration, Registrations, SelectOption, ServiceCallRequest, ServiceRegistration,
        ShutdownReason, TaskCreateRequest, TaskDefinitionRequest, TaskFinishRequest,
        TaskUpdateRequest, TextSizeToken, ThemeColor, ThemeMode, ThemeSnapshot, ThemeToken,
        ToastKind, View, ViewNode, ViewStyle, ViewTree, ViewTreeBuilder, Window,
            app_info, badge, badge_styled, bmcbl_plugin, button, button_action, call_plugin_service,
        call_service, card, checkbox, create_task, create_typed_task, current_locale,
        current_unix_ms, default_style, download_resource, emit_event, finish_task,
        http_get_text, icon, image, image_options, invalidate, invalidate_all,
        invalidate_injection, invalidate_page, is_dark_mode, link, link_tooltip,
        log, log_debug, log_error, log_info, log_warn, navigate, navigate_page,
        navigate_path, open_external_url, open_modal, open_window, option, plugin_actions,
        plugin_error, plugin_metadata, progress, read_clipboard_text, read_config,
        read_resource_bytes, read_resource_text, register_service,
        register_task_definition, registrations, section, select, session_get, session_set,
        show_toast, spacer, storage_delete, storage_get, storage_list, storage_set,
        text, theme_accent, theme_snapshot, title, toast, toggle, tr, tr_arg, tr_args, update_task,
        view, write_clipboard_text, write_config,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrations_builder_outputs_page_injection_and_subscription() {
        let registrations = Registrations::new()
            .page(Page::new("main", "Main").tool(Tool::new("Essentials").icon("plug").order(7)))
            .injection(Injection::new(InjectionSlot::MainRootOverlay).priority(3))
            .subscribe("route-changed")
            .finish();

        assert_eq!(registrations.len(), 3);
    }

    #[test]
    fn view_macro_builds_root_container_with_children() {
        let tree = view! {
            column(padding = 16, gap = 8) {
                badge("Loaded");
                title("BMCBL Essentials");
                button("Open", "open-window");
            }
        };

        assert_eq!(tree.root, 0);
        assert_eq!(tree.nodes.len(), 4);
        let ViewNode::Container(container) = &tree.nodes[0] else {
            panic!("root should be a container");
        };
        assert_eq!(container.children, vec![1, 2, 3]);
    }

    #[test]
    fn ui_control_helpers_build_expected_nodes() {
        let ViewNode::Checkbox(checkbox) = checkbox("Enabled", true, "toggle-enabled") else {
            panic!("checkbox helper should build checkbox node");
        };
        assert_eq!(checkbox.action_value.as_deref(), Some("false"));

        let ViewNode::Select(select) = select(
            "Mode",
            "mode",
            [option("Fast", "fast"), option("Safe", "safe")],
            Some("safe"),
        ) else {
            panic!("select helper should build select node");
        };
        assert_eq!(select.options.len(), 2);
        assert_eq!(select.selected.as_deref(), Some("safe"));

        let ViewNode::Progress(progress) = progress("Sync", 3, Some(10)) else {
            panic!("progress helper should build progress node");
        };
        assert_eq!(progress.value, 3);
        assert_eq!(progress.total, Some(10));
    }

    #[test]
    fn plugin_error_helpers_set_stable_codes() {
        assert_eq!(PluginError::denied("no").code, "denied");
        assert_eq!(PluginError::invalid_input("bad").code, "invalid-input");
        assert_eq!(PluginError::not_found("missing").code, "not-found");
        assert_eq!(PluginError::app("app").code, "app");
        assert_eq!(PluginError::timeout("slow").code, "timeout");
    }

    #[test]
    fn action_helper_matches_action_events() {
        let event = PluginEvent {
            plugin_id: Some("plugin".to_string()),
            page_id: Some("main".to_string()),
            kind: PluginEventKind::Action(ActionEvent {
                action_id: "open-window".to_string(),
                value: None,
            }),
        };

        assert!(event.action_is("open-window"));
    }

    #[test]
    fn image_options_builder_sets_stable_render_fields() {
        let node = image_options(
            "https://example.com/image.webp",
            "Preview",
            ImageOptions::new()
                .caption("Caption")
                .placeholder("Loading")
                .fallback("Unavailable")
                .height(180)
                .min_height(120)
                .max_height(240)
                .aspect_ratio(16, 9)
                .corner_radius(12)
                .fit(ImageFit::Contain),
        );

        let ViewNode::Image(image) = node else {
            panic!("node should be an image");
        };
        assert_eq!(image.caption, "Caption");
        assert_eq!(image.placeholder, "Loading");
        assert_eq!(image.fallback, "Unavailable");
        assert_eq!(image.height, Some(180));
        assert_eq!(image.min_height, Some(120));
        assert_eq!(image.max_height, Some(240));
        assert_eq!(image.aspect_ratio_x, Some(16));
        assert_eq!(image.aspect_ratio_y, Some(9));
        assert_eq!(image.corner_radius, Some(12));
        assert_eq!(image.fit, ImageFit::Contain);
    }

    #[test]
    fn injection_layout_builder_roundtrips() {
        let registration = Injection::new(InjectionSlot::HomeSidebar)
            .page("/")
            .priority(40)
            .layout(
                InjectionLayout::sidebar()
                    .width(320)
                    .min_width(248)
                    .max_width(360)
                    .max_height(360)
                    .priority(2)
                    .compact_behavior(CompactBehavior::Scroll),
            )
            .finish();

        assert_eq!(
            registration.layout.and_then(|layout| layout.preferred_width),
            Some(320)
        );
    }

    #[test]
    fn theme_snapshot_default_is_light() {
        let snapshot = ThemeSnapshot::light_default();
        assert!(!snapshot.is_dark());
        assert_eq!(snapshot.mode, ThemeMode::Light);
    }
}

use crate::plugins::events::{
    CompactBehavior, EventCascade, InjectionLayout, InjectionSlot, PluginEvent, PluginEventKind,
    PluginInjectionRegistration, PluginNavigationEntry, PluginPageRegistration,
    ROUTE_CHANGED_EVENT, sort_injections,
};
use crate::plugins::manifest::{PluginCapability, PluginManifest};
use bmcbl_plugin_runtime::ViewTree;
use crate::tasks::TaskStage;
use crate::tasks::task_manager::TaskFinishStatus;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use anyhow::{Context, Result, anyhow, bail};
use bmcbl_plugin_runtime as abi;
use gpui::{App, BorrowAppContext, ClipboardItem, Global, Hsla, SharedString};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};

use crate::plugins::services::{DependencyGraph, PluginRpcCallStack, PluginServiceRegistry};

pub use bmcbl_plugin_runtime::{
    EVENT_TIMEOUT, RENDER_TIMEOUT, RENDER_WARN_THRESHOLD,
};
const HTTP_DEFAULT_TTL: Duration = Duration::from_secs(30 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const HTTP_MAX_BYTES: usize = 512 * 1024;
const STORAGE_KEY_MAX_BYTES: usize = 128;

static ASYNC_APP_REFRESH_NOTIFICATION: OnceLock<Mutex<Option<crate::plugins::watcher::Sender>>> =
    OnceLock::new();
static PLUGIN_PERSISTENCE_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<PluginPersistenceOp>> =
    OnceLock::new();

const PLUGIN_PERSISTENCE_COALESCE_DELAY: Duration = Duration::from_millis(20);

#[derive(Clone, Debug)]
pub struct PluginPage {
    pub plugin_id: String,
    pub page_id: String,
    pub title: SharedString,
    pub navigation: Option<PluginNavigationEntry>,
    pub icon_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct PluginStatus {
    pub id: String,
    pub name: SharedString,
    pub version: SharedString,
    pub healthy: bool,
    pub error: Option<SharedString>,
    pub generation: u64,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub tags: Vec<String>,
    pub capabilities: Vec<String>,
    pub enabled: bool,
    pub loaded: bool,
    pub permissions: PluginPermissionStatus,
    pub limits: PluginLimitStatus,
    pub has_readme: bool,
    pub has_config: bool,
    pub icon_path: Option<PathBuf>,
    pub root_dir: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct PluginPermissionStatus {
    pub network_allow: Vec<String>,
    pub resource_allow: Vec<String>,
    pub external_allow: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PluginLimitStatus {
    pub max_http_bytes: u64,
    pub max_resource_bytes: u64,
    pub max_storage_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginLoadState {
    Unloaded,
    Loaded { generation: u64 },
    Failed { generation: u64, error: String },
}

#[derive(Clone)]
pub struct PluginInstance {
    pub manifest: PluginManifest,
    pub generation: u64,
    pub pages: Vec<PluginPageRegistration>,
    pub injections: Vec<PluginInjectionRegistration>,
    pub subscriptions: BTreeSet<String>,
    pub services: BTreeSet<String>,
    pub translations: BTreeMap<String, BTreeMap<String, String>>,
    pub state: PluginLoadState,
    pub enabled: bool,
    prepared_wasm: PreparedPluginWasm,
    prepared_resources: PreparedPluginResources,
    runtime: Option<Arc<Mutex<abi::PluginExecution>>>,
}

/// 插件 wasm 线性内存快照（预算评估用）。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PluginWasmMemorySnapshot {
    pub linear_bytes: usize,
    pub page_count: usize,
}

#[derive(Clone)]
enum PreparedPluginWasm {
    /// 原始组件字节 + 内容摘要；编译延迟到实例化时（有磁盘缓存）。
    Ready { wasm: Arc<[u8]>, sha256: String },
    Error(Arc<str>),
}

impl PreparedPluginWasm {
    /// 两次准备结果是否来自同一份 wasm 内容。
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ready { sha256: left, .. }, Self::Ready { sha256: right, .. }) => left == right,
            (Self::Error(left), Self::Error(right)) => left == right,
            _ => false,
        }
    }

}

impl std::fmt::Debug for PreparedPluginWasm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready { sha256, .. } => f.debug_struct("Ready").field("sha256", sha256).finish(),
            Self::Error(error) => f.debug_tuple("Error").field(error).finish(),
        }
    }
}

/// 插件目录内容指纹：只记录相对路径、长度和修改时间，不读取文件内容。
///
/// 用来发现不在清单、翻译和资源清单里的改动（例如 `assets/` 下的文件），从而只失效
/// 该插件的缓存，而不是重建它的运行实例。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PluginSourceStamp {
    entries: Vec<(PathBuf, u64, Option<SystemTime>)>,
}

impl PluginSourceStamp {
    /// 递归记录 `root_dir` 下每个文件的大小与修改时间。
    fn read(root_dir: &Path) -> Self {
        let mut entries = Vec::new();
        collect_source_stamp(root_dir, root_dir, &mut entries);
        entries.sort();
        Self { entries }
    }
}

fn collect_source_stamp(
    root_dir: &Path,
    dir: &Path,
    entries: &mut Vec<(PathBuf, u64, Option<SystemTime>)>,
) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_source_stamp(root_dir, &path, entries);
            continue;
        }
        let relative = path.strip_prefix(root_dir).unwrap_or(&path).to_path_buf();
        entries.push((relative, metadata.len(), metadata.modified().ok()));
    }
}

#[derive(Clone, Debug)]
struct PreparedPluginResources {
    has_readme: bool,
    has_config: bool,
    icon_path: Option<PathBuf>,
    config_text: std::result::Result<String, Arc<str>>,
    storage_values: std::result::Result<BTreeMap<String, String>, Arc<str>>,
    resource_values: BTreeMap<String, Arc<[u8]>>,
    game_mods: Vec<super::game_mods::GameMod>,
    source_stamp: PluginSourceStamp,
}

impl PreparedPluginResources {
    /// 比较除目录指纹外的内容，用于判断是否需要重建插件运行实例。
    fn same_content(&self, other: &Self) -> bool {
        self.has_readme == other.has_readme
            && self.has_config == other.has_config
            && self.icon_path == other.icon_path
            && self.config_text == other.config_text
            && self.storage_values == other.storage_values
            && self.resource_values == other.resource_values
            && self.game_mods == other.game_mods
    }
}

/// 已加载实例与本次扫描结果的对应关系。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedMatch {
    /// 完全一致：复用实例和全部缓存。
    Unchanged,
    /// 插件源码语义一致但目录内容有变化：复用实例，只失效它的缓存。
    Invalidated,
    /// 插件源码语义发生变化：必须重建实例。
    Rebuilt,
}

#[derive(Clone, Debug)]
struct PreparedPluginManifest {
    manifest: PluginManifest,
    enabled: bool,
    translations: BTreeMap<String, BTreeMap<String, String>>,
    wasm: PreparedPluginWasm,
    resources: PreparedPluginResources,
}

#[derive(Clone, Debug, Default)]
struct PreparedPluginReload {
    plugins: Vec<PreparedPluginManifest>,
}

/// 跨线程复用的插件准备缓存。
///
/// 插件目录内容指纹未变时直接复用上一次的准备结果，避免每次重载都重新读取、哈希和解析
/// 全部插件的 wasm、翻译、存储快照与资源文件。缓存按插件目录键控，并随扫描结果收缩。
type PluginPrepareEntries = BTreeMap<PathBuf, (PluginSourceStamp, Arc<PreparedPluginManifest>)>;

#[derive(Clone, Default)]
struct PluginPrepareCache {
    entries: Arc<Mutex<PluginPrepareEntries>>,
}

impl PluginPrepareCache {
    /// 目录指纹与清单都一致时返回上一次的准备结果。
    ///
    /// 同时比较清单是因为调用方可以直接提交内存中的清单（例如安装流程），此时磁盘目录
    /// 可能根本没有变化。
    fn lookup(
        &self,
        root_dir: &Path,
        manifest: &PluginManifest,
        stamp: &PluginSourceStamp,
    ) -> Option<Arc<PreparedPluginManifest>> {
        let entries = self.entries.lock().ok()?;
        let (cached_stamp, prepared) = entries.get(root_dir)?;
        (cached_stamp == stamp && prepared.manifest == *manifest).then(|| Arc::clone(prepared))
    }

    /// 记录本次准备结果与对应的目录指纹。
    fn store(
        &self,
        root_dir: &Path,
        stamp: PluginSourceStamp,
        prepared: Arc<PreparedPluginManifest>,
    ) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.insert(root_dir.to_path_buf(), (stamp, prepared));
    }

    /// 丢弃已经不在扫描结果里的插件目录缓存。
    fn retain_dirs(&self, installed_dirs: &BTreeSet<PathBuf>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.retain(|root_dir, _entry| installed_dirs.contains(root_dir));
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum PluginPersistenceKey {
    Config(String),
    Storage { plugin_id: String, key: String },
}

#[derive(Clone, Debug)]
enum PluginPersistenceOp {
    Config {
        plugin_id: String,
        manifest: PluginManifest,
        content: String,
    },
    StorageSet {
        plugin_id: String,
        storage_dir: PathBuf,
        key: String,
        value: String,
    },
    StorageDelete {
        plugin_id: String,
        storage_dir: PathBuf,
        key: String,
    },
}

impl PluginPersistenceOp {
    fn key(&self) -> PluginPersistenceKey {
        match self {
            Self::Config { plugin_id, .. } => PluginPersistenceKey::Config(plugin_id.clone()),
            Self::StorageSet { plugin_id, key, .. }
            | Self::StorageDelete { plugin_id, key, .. } => PluginPersistenceKey::Storage {
                plugin_id: plugin_id.clone(),
                key: key.clone(),
            },
        }
    }

    fn plugin_id(&self) -> &str {
        match self {
            Self::Config { plugin_id, .. }
            | Self::StorageSet { plugin_id, .. }
            | Self::StorageDelete { plugin_id, .. } => plugin_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PluginEffect {
    Toast {
        kind: abi::ToastKind,
        message: String,
    },
    Navigate {
        target: abi::RouteTarget,
    },
    OpenWindow {
        request: abi::WindowRequest,
    },
    OpenModal {
        request: abi::ModalRequest,
    },
    CloseWindow {
        window_id: u64,
    },
    EmitEvent {
        name: String,
        payload: String,
    },
    Invalidate {
        plugin_id: String,
        target: abi::InvalidateTarget,
    },
    Log {
        plugin_id: String,
        level: abi::LogLevel,
        message: String,
    },
    RegisterService {
        plugin_id: String,
        service_name: String,
    },
}

#[derive(Clone, Debug)]
pub struct RenderedInjection {
    pub plugin_id: String,
    pub tree: Arc<ViewTree>,
    pub layout: Option<InjectionLayout>,
}

#[derive(Clone, Debug)]
pub(crate) enum RenderContext {
    Page {
        page_id: String,
    },
    Injection {
        slot: InjectionSlot,
        page: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum PluginInvalidationTarget {
    Plugin {
        plugin_id: String,
    },
    Page {
        plugin_id: String,
        page_id: String,
    },
    Injection {
        plugin_id: String,
        slot: InjectionSlot,
        page: Option<String>,
    },
}

impl PluginInvalidationTarget {
    fn plugin_id(&self) -> &str {
        match self {
            Self::Plugin { plugin_id }
            | Self::Page { plugin_id, .. }
            | Self::Injection { plugin_id, .. } => plugin_id,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct HttpCacheSnapshot {
    pub state: abi::HttpCacheState,
    pub body: Option<String>,
    pub error: Option<String>,
    pub fetched_at_unix_ms: Option<u64>,
}

#[derive(Debug)]
struct HttpCacheEntry {
    body: Option<String>,
    error: Option<String>,
    fetched_at: Option<Instant>,
    fetched_at_unix_ms: Option<u64>,
    refreshing: bool,
    subscribers: BTreeSet<PluginInvalidationTarget>,
}

#[derive(Debug)]
struct HttpRefreshResult {
    url: String,
    result: std::result::Result<(String, u64), String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PluginHttpFetchCache {
    state: Arc<Mutex<PluginHttpFetchCacheState>>,
    finished_refresh_pending: Arc<AtomicBool>,
}

#[derive(Debug)]
struct PluginHttpFetchCacheState {
    entries: BTreeMap<String, HttpCacheEntry>,
    sender: mpsc::Sender<HttpRefreshResult>,
    receiver: Arc<Mutex<mpsc::Receiver<HttpRefreshResult>>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ResourceCacheKey {
    plugin_id: String,
    path: String,
}

#[derive(Debug)]
struct ResourceCacheEntry {
    bytes: Option<Arc<[u8]>>,
    error: Option<String>,
    loading: bool,
    subscribers: BTreeSet<PluginInvalidationTarget>,
}

#[derive(Debug)]
struct ResourceLoadResult {
    key: ResourceCacheKey,
    result: std::result::Result<Vec<u8>, String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PluginResourceCache {
    state: Arc<Mutex<PluginResourceCacheState>>,
    finished_refresh_pending: Arc<AtomicBool>,
}

#[derive(Debug)]
struct PluginResourceCacheState {
    entries: BTreeMap<ResourceCacheKey, ResourceCacheEntry>,
    sender: mpsc::Sender<ResourceLoadResult>,
    receiver: Arc<Mutex<mpsc::Receiver<ResourceLoadResult>>>,
}

impl Default for PluginResourceCache {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state: Arc::new(Mutex::new(PluginResourceCacheState {
                entries: BTreeMap::new(),
                sender,
                receiver: Arc::new(Mutex::new(receiver)),
            })),
            finished_refresh_pending: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for PluginHttpFetchCache {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            state: Arc::new(Mutex::new(PluginHttpFetchCacheState {
                entries: BTreeMap::new(),
                sender,
                receiver: Arc::new(Mutex::new(receiver)),
            })),
            finished_refresh_pending: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct PageRenderCacheKey {
    plugin_id: String,
    page_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct InjectionRenderCacheKey {
    slot: InjectionSlot,
    page: Option<String>,
}

// A single host-owned working set, with no per-plugin or per-page soft quota.
// Live pages hold Arc leases; recent injection results are protected by recency.
const VIEW_CACHE_SOFT_BYTES: usize = 64 * 1024 * 1024;
const VIEW_CACHE_HOT_AGE: Duration = Duration::from_secs(30);

#[derive(Debug)]
struct CachedView<T> {
    value: T,
    last_used: std::cell::Cell<Instant>,
}

impl<T> CachedView<T> {
    fn new(value: T) -> Self {
        Self { value, last_used: std::cell::Cell::new(Instant::now()) }
    }

    fn get(&self) -> &T {
        self.last_used.set(Instant::now());
        &self.value
    }
}

#[derive(Debug, Default)]
struct RenderCache {
    pages: BTreeMap<PageRenderCacheKey, CachedView<Arc<ViewTree>>>,
    page_errors: BTreeMap<PageRenderCacheKey, Arc<str>>,
    injections: BTreeMap<InjectionRenderCacheKey, CachedView<Vec<RenderedInjection>>>,
    pending_pages: BTreeMap<PageRenderCacheKey, u64>,
    pending_injections: BTreeMap<InjectionRenderCacheKey, u64>,
    ticket: u64,
}

#[derive(Clone)]
pub(crate) struct ServiceDispatcher(
    pub Arc<dyn Fn(&str, abi::ServiceCallRequest) -> Result<Vec<u8>, abi::PluginError> + Send + Sync>,
);

impl std::fmt::Debug for ServiceDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceDispatcher(...)")
    }
}


pub(crate) fn create_service_dispatcher(
    service_registry: Arc<Mutex<PluginServiceRegistry>>,
    service_runtimes: Arc<Mutex<BTreeMap<String, Arc<Mutex<abi::PluginExecution>>>>>,
    rpc_stack: Arc<Mutex<PluginRpcCallStack>>,
) -> ServiceDispatcher {
    ServiceDispatcher(Arc::new(
        move |caller_id: &str, request: abi::ServiceCallRequest| {
            let target_plugin_id = match &request.target_plugin {
                Some(target) => target.clone(),
                None => service_registry
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .find_service(&request.service_name)
                    .map(|descriptor| descriptor.provider_plugin_id.clone())
                    .ok_or_else(|| {
                        abi::PluginError::not_found(format!(
                            "service '{}' is not registered",
                            request.service_name
                        ))
                    })?,
            };
            let _guard = rpc_stack
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .enter(&target_plugin_id)
                .map_err(|error| {
                    abi::PluginError::denied(error.to_string())
                })?;
            let runtime = service_runtimes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&target_plugin_id)
                .cloned()
                .ok_or_else(|| {
                    abi::PluginError::not_found(format!(
                        "plugin '{target_plugin_id}' is not loaded"
                    ))
                })?;
            let mut execution = runtime.try_lock()
                .map_err(|_| abi::PluginError::new("busy", "target plugin is already executing"))?;
            let result = execution.call_service(request);
            let effects = execution.take_effects();
            drop(execution);
            if !effects.is_empty() {
                warn!(
                    caller_id,
                    target_plugin_id,
                    count = effects.len(),
                    "plugin service call emitted plugin effects; effects are discarded"
                );
            }
            result.map_err(|error| {
                abi::PluginError::new("service-call-failed", format!("{error:#}"))
            })
        },
    ))
}



#[derive(Clone)]
struct PluginActivation {
    engine: Arc<std::sync::OnceLock<Result<abi::PluginEngine, String>>>,
    cache_dir: PathBuf,
    package_cache_dir: PathBuf,
    http_cache: PluginHttpFetchCache,
    resource_cache: PluginResourceCache,
    dispatcher: ServiceDispatcher,
}

impl PluginActivation {
    fn instantiate(&self, instance: &PluginInstance) -> Result<abi::PluginExecution> {
        let manifest = &instance.manifest;
        let resources = &instance.prepared_resources;
        let wasm = match &instance.prepared_wasm {
            PreparedPluginWasm::Ready { wasm, .. } => wasm,
            PreparedPluginWasm::Error(error) => return Err(anyhow!("{error}")),
        };
        let engine = self.engine.get_or_init(|| {
            abi::PluginEngine::with_cache(Some(&self.cache_dir)).map_err(|error| error.to_string())
        }).as_ref().map_err(|error| anyhow!("{error}"))?;
        let artifact = engine.load_component(wasm).context("compile plugin component")?;
        self.resource_cache.seed(&manifest.id, &resources.resource_values);
        let context = crate::plugins::host_context::AppPluginHostContext {
            manifest: manifest.clone(),
            http_cache: self.http_cache.clone(),
            resource_cache: self.resource_cache.clone(),
            storage_dir: self.package_cache_dir.join("storage").join(sanitize_storage_segment(&manifest.id)),
            service_dispatcher: Some(self.dispatcher.clone()),
            services: super::app_services::ServiceJobs::new(manifest, instance.generation),
        };
        abi::PluginExecution::instantiate(
            engine,
            &artifact,
            plugin_policy_from_manifest(manifest),
            Arc::new(context),
            abi::PluginInitState {
                locale: current_locale_code(),
                translations: instance.translations.clone(),
                theme: abi::ThemeSnapshot::light_default(),
                clipboard_text: None,
                storage_snapshot: resources.storage_values.clone().map_err(|error| error.to_string()),
                config_text: resources.config_text.clone().map_err(|error| error.to_string()),
            },
        ).context("instantiate plugin component")
    }

    fn activate(&self, mut instance: PluginInstance) -> Result<PluginInstance> {
        let mut execution = self.instantiate(&instance)?;
        let manifest = &instance.manifest;
        let context = abi::PluginContext {
            plugin_id: manifest.id.clone(),
            api_version: manifest.api_version.clone(),
        };
        let registrations: Vec<abi::Registration> =
            execution.init(context).map_err(plugin_error_to_anyhow)?;
        let discarded = execution.discard_non_log_effects();
        if discarded != 0 {
            warn!(
                plugin_id = manifest.id,
                count = discarded,
                "plugin init emitted plugin effects; effects are ignored during init"
            );
        }

        let mut pages = Vec::new();
        let mut injections = Vec::new();
        let mut subscriptions = BTreeSet::new();
        let mut services = BTreeSet::new();

        for registration in registrations {
            match bootstrap_registration_from_abi(&manifest.id, registration)? {
                BootstrapRegistration::Page {
                    page_id,
                    title,
                    navigation,
                } => {
                    manifest.require_capability(PluginCapability::UiPage)?;
                    pages.push(PluginPageRegistration {
                        plugin_id: manifest.id.clone(),
                        page_id,
                        title,
                        navigation,
                    });
                }
                BootstrapRegistration::Injection {
                    slot,
                    page,
                    priority,
                    layout,
                } => {
                    manifest.require_capability(PluginCapability::UiHook)?;
                    injections.push(PluginInjectionRegistration {
                        plugin_id: manifest.id.clone(),
                        slot,
                        page,
                        priority,
                        layout,
                    });
                }
                BootstrapRegistration::Subscription { event } => {
                    manifest.require_capability(PluginCapability::EventGlobal)?;
                    subscriptions.insert(event);
                }
                BootstrapRegistration::Service { service_name } => {
                    services.insert(service_name);
                }
            }
        }

        if pages.is_empty() && manifest.has_capability(&PluginCapability::UiPage) {
            pages.push(PluginPageRegistration {
                plugin_id: manifest.id.clone(),
                page_id: "main".to_string(),
                title: manifest.name.clone(),
                navigation: None,
            });
        }

        instance.pages = pages;
        instance.injections = injections;
        instance.subscriptions = subscriptions;
        instance.services = services;
        instance.runtime = Some(Arc::new(Mutex::new(execution)));
        instance.state = PluginLoadState::Loaded { generation: instance.generation };
        Ok(instance)
    }
}

pub struct PluginRegistry {
    plugins_dir: PathBuf,
    cache_dir: PathBuf,
    package_cache_dir: PathBuf,
    engine: Arc<std::sync::OnceLock<Result<abi::PluginEngine, String>>>,
    activating: BTreeSet<(String, u64)>,
    event_queue: VecDeque<(PluginEvent, Arc<Mutex<EventCascade>>)>,
    event_pending: bool,
    early_job_events: Vec<(String, u64, String)>,
    background_activation: bool,
    plugins: BTreeMap<String, PluginInstance>,
    pages: BTreeMap<(String, String), PluginPage>,
    injections: Vec<PluginInjectionRegistration>,
    /// 事件订阅倒排索引：事件名 -> 订阅该事件的插件 id，避免每次事件都扫描全部插件。
    event_subscribers: BTreeMap<String, BTreeSet<String>>,
    render_cache: RenderCache,
    logs: BTreeMap<String, VecDeque<PluginLogEntry>>,
    log_snapshots: BTreeMap<String, Arc<[PluginLogEntry]>>,
    http_cache: PluginHttpFetchCache,
    resource_cache: PluginResourceCache,
    prepare_cache: PluginPrepareCache,
    generation: u64,
    reload_tx: Option<crate::plugins::watcher::Sender>,
    watcher_task: Option<gpui::Task<()>>,
    last_error: Option<SharedString>,
    loaded_once: bool,
    /// 启动阶段的清单扫描正在后台执行，或已被 first-visible gate 延后。
    initial_reload_pending: bool,
    /// true 表示初始扫描正在等待主窗口第一次可见帧。
    initial_reload_deferred: bool,
    /// 常规 reload 的文件准备阶段正在 blocking pool 执行。
    reload_prepare_pending: bool,
    /// pending 期间又收到 reload 请求；完成当前任务后合并为一次后续 reload。
    reload_prepare_requested: bool,
    active_modal: Option<PluginModalState>,
    pub service_registry: Arc<Mutex<PluginServiceRegistry>>,
    pub rpc_stack: Arc<Mutex<PluginRpcCallStack>>,
    service_runtimes: Arc<Mutex<BTreeMap<String, Arc<Mutex<abi::PluginExecution>>>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginModalState {
    pub plugin_id: String,
    pub page_id: String,
    pub title: SharedString,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct PluginLogEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub level: abi::LogLevel,
    pub message: SharedString,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginMemorySnapshot {
    pub plugin_id: String,
    pub name: SharedString,
    pub enabled: bool,
    pub loaded: bool,
    pub wasm_linear_bytes: usize,
    pub wasm_page_count: usize,
    pub render_cache_entries: usize,
    pub render_cache_bytes: usize,
    pub http_cache_entries: usize,
    pub http_cache_body_bytes: usize,
    pub http_cache_error_bytes: usize,
    pub resource_cache_entries: usize,
    pub resource_cache_bytes: usize,
    pub resource_cache_error_bytes: usize,
    pub log_entries: usize,
    pub log_bytes: usize,
    pub total_estimated_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginMemoryReport {
    pub plugins: Vec<PluginMemorySnapshot>,
    pub module_cache_entries: usize,
    pub module_cache_estimated_bytes: usize,
    pub total_estimated_bytes: usize,
}

impl Global for PluginRegistry {}

impl Default for PluginRegistry {
    fn default() -> Self {
        let plugins_dir = crate::utils::file_ops::bmcbl_subdir("plugins");
        let cache_dir = crate::utils::file_ops::cache_subdir("wasm");
        let package_cache_dir = crate::utils::file_ops::cache_subdir("plugins");
        let mut registry = Self::new(plugins_dir, cache_dir, package_cache_dir);
        registry.background_activation = true;
        registry
    }
}

impl PluginRegistry {
    pub fn new(plugins_dir: PathBuf, cache_dir: PathBuf, package_cache_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            cache_dir,
            package_cache_dir,
            engine: Arc::default(),
            activating: BTreeSet::new(),
            event_queue: VecDeque::new(),
            event_pending: false,
            early_job_events: Vec::new(),
            background_activation: false,
            plugins: BTreeMap::new(),
            pages: BTreeMap::new(),
            injections: Vec::new(),
            event_subscribers: BTreeMap::new(),
            render_cache: RenderCache::default(),
            logs: BTreeMap::new(),
            log_snapshots: BTreeMap::new(),
            http_cache: PluginHttpFetchCache::default(),
            resource_cache: PluginResourceCache::default(),
            prepare_cache: PluginPrepareCache::default(),
            generation: 0,
            reload_tx: None,
            watcher_task: None,
            last_error: None,
            loaded_once: false,
            initial_reload_pending: false,
            initial_reload_deferred: false,
            reload_prepare_pending: false,
            reload_prepare_requested: false,
            active_modal: None,
            service_registry: Arc::new(Mutex::new(PluginServiceRegistry::new())),
            rpc_stack: Arc::new(Mutex::new(PluginRpcCallStack::new())),
            service_runtimes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn publish_active_game_mods(&self) {
        super::game_mods::publish_active(
            self.plugins.values()
                .filter(|instance| instance.enabled && matches!(instance.state, PluginLoadState::Loaded { .. }))
                .flat_map(|instance| instance.prepared_resources.game_mods.iter().cloned()
                    .map(move |module| (module, instance.generation)))
                .collect(),
        );
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn package_cache_dir(&self) -> &Path {
        &self.package_cache_dir
    }

    fn plugin_storage_dir(&self, plugin_id: &str) -> PathBuf {
        self.package_cache_dir
            .join("storage")
            .join(sanitize_storage_segment(plugin_id))
    }

    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.plugins
            .values()
            .map(|instance| {
                let loaded = matches!(instance.state, PluginLoadState::Loaded { .. });
                let failed = matches!(instance.state, PluginLoadState::Failed { .. });
                let error = match &instance.state {
                    PluginLoadState::Failed { error, .. } => {
                        Some(SharedString::from(error.clone()))
                    }
                    PluginLoadState::Unloaded | PluginLoadState::Loaded { .. } => None,
                };
                PluginStatus {
                    id: instance.manifest.id.clone(),
                    name: SharedString::from(instance.manifest.name.clone()),
                    version: SharedString::from(instance.manifest.version.clone()),
                    healthy: !failed,
                    error,
                    generation: instance.generation,
                    authors: instance.manifest.authors.clone(),
                    description: instance.manifest.description.clone(),
                    website: instance.manifest.website.clone(),
                    license: instance.manifest.license.clone(),
                    tags: instance.manifest.tags.clone(),
                    capabilities: instance
                        .manifest
                        .capabilities
                        .iter()
                        .map(|capability| capability.as_str().to_string())
                        .collect(),
                    enabled: instance.enabled,
                    loaded,
                    permissions: PluginPermissionStatus {
                        network_allow: instance.manifest.permissions.network_allow.clone(),
                        resource_allow: instance.manifest.permissions.resource_allow.clone(),
                        external_allow: instance.manifest.permissions.external_allow.clone(),
                    },
                    limits: PluginLimitStatus {
                        max_http_bytes: instance.manifest.limits.max_http_bytes,
                        max_resource_bytes: instance.manifest.limits.max_resource_bytes,
                        max_storage_bytes: instance.manifest.limits.max_storage_bytes,
                    },
                    has_readme: instance.prepared_resources.has_readme,
                    has_config: instance.prepared_resources.has_config,
                    icon_path: instance.prepared_resources.icon_path.clone(),
                    root_dir: instance.manifest.root_dir.clone(),
                }
            })
            .collect()
    }

    pub fn memory_report(&self) -> PluginMemoryReport {
        let mut plugins = self
            .plugins
            .values()
            .map(|instance| {
                let linear_bytes = instance
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.try_lock().ok().map(|execution| execution.observed_memory_bytes()))
                    .unwrap_or_default();
                let wasm = PluginWasmMemorySnapshot {
                    linear_bytes,
                    page_count: linear_bytes / (64 * 1024),
                };
                let render_cache = self
                    .render_cache
                    .plugin_memory(instance.manifest.id.as_str());
                let http_cache = self
                    .http_cache
                    .memory_snapshot_for_plugin(instance.manifest.id.as_str());
                let resource_cache = self
                    .resource_cache
                    .memory_snapshot_for_plugin(instance.manifest.id.as_str());
                let log = plugin_log_memory(
                    self.logs
                        .get(instance.manifest.id.as_str())
                        .map(VecDeque::as_slices),
                );
                let total = wasm
                    .linear_bytes
                    .saturating_add(render_cache.estimated_bytes)
                    .saturating_add(http_cache.body_bytes)
                    .saturating_add(http_cache.error_bytes)
                    .saturating_add(resource_cache.bytes)
                    .saturating_add(resource_cache.error_bytes)
                    .saturating_add(log.bytes);
                PluginMemorySnapshot {
                    plugin_id: instance.manifest.id.clone(),
                    name: SharedString::from(instance.manifest.name.clone()),
                    enabled: instance.enabled,
                    loaded: matches!(instance.state, PluginLoadState::Loaded { .. }),
                    wasm_linear_bytes: wasm.linear_bytes,
                    wasm_page_count: wasm.page_count,
                    render_cache_entries: render_cache.entries,
                    render_cache_bytes: render_cache.estimated_bytes,
                    http_cache_entries: http_cache.entries,
                    http_cache_body_bytes: http_cache.body_bytes,
                    http_cache_error_bytes: http_cache.error_bytes,
                    resource_cache_entries: resource_cache.entries,
                    resource_cache_bytes: resource_cache.bytes,
                    resource_cache_error_bytes: resource_cache.error_bytes,
                    log_entries: log.entries,
                    log_bytes: log.bytes,
                    total_estimated_bytes: total,
                }
            })
            .collect::<Vec<_>>();
        plugins.sort_by(|left, right| {
            right
                .total_estimated_bytes
                .cmp(&left.total_estimated_bytes)
                .then_with(|| left.plugin_id.cmp(&right.plugin_id))
        });
        let total_estimated_bytes = plugins
            .iter()
            .map(|plugin| plugin.total_estimated_bytes)
            .sum::<usize>();
        PluginMemoryReport {
            plugins,
            module_cache_entries: 0,
            module_cache_estimated_bytes: 0,
            total_estimated_bytes,
        }
    }

    pub fn plugin_logs(&self, plugin_id: &str) -> Vec<PluginLogEntry> {
        self.logs
            .get(plugin_id)
            .map(|logs| logs.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn plugin_log_snapshot(&self, plugin_id: &str) -> Arc<[PluginLogEntry]> {
        self.log_snapshots
            .get(plugin_id)
            .cloned()
            .unwrap_or_else(|| Arc::from([]))
    }

    pub fn translate_resource(&self, plugin_id: &str, key: &str) -> Option<String> {
        self.translate_resource_locale(plugin_id, &current_locale_code(), key)
    }

    /// Translates through a specific locale instead of the launcher UI locale.
    pub fn translate_resource_locale(
        &self,
        plugin_id: &str,
        locale: &str,
        key: &str,
    ) -> Option<String> {
        let plugin = self.plugins.get(plugin_id)?;
        let translated = translate_plugin_key(&plugin.translations, locale, key, &[]);
        (translated != key).then_some(translated)
    }

    pub fn set_plugin_enabled(&mut self, plugin_id: &str, enabled: bool) -> Result<()> {
        crate::plugins::state::set_enabled(&self.plugins_dir, plugin_id, enabled)?;
        self.reload_all()
    }

    pub fn uninstall_plugin(&mut self, plugin_id: &str) -> Result<()> {
        let manifest = self
            .plugins
            .get(plugin_id)
            .map(|plugin| plugin.manifest.clone())
            .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
        uninstall_plugin_files(&self.plugins_dir, &manifest, plugin_id)?;
        self.reload_all()
    }

    pub fn reload_plugin(&mut self, plugin_id: &str) -> Result<()> {
        if !self.plugins.contains_key(plugin_id) {
            bail!("unknown plugin {plugin_id}");
        }
        self.reload_all()
    }

    pub fn export_plugin_diagnostics(&self, plugin_id: &str) -> Result<String> {
        let plugin = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
        let status = match &plugin.state {
            PluginLoadState::Unloaded => "unloaded".to_string(),
            PluginLoadState::Loaded { generation } => format!("loaded generation={generation}"),
            PluginLoadState::Failed { generation, error } => {
                format!("failed generation={generation} error={error}")
            }
        };
        let logs = self.plugin_logs(plugin_id);
        let mut output = String::new();
        output.push_str(&format!(
            "Plugin: {} ({})\n",
            plugin.manifest.id, plugin.manifest.name
        ));
        output.push_str(&format!("Version: {}\n", plugin.manifest.version));
        output.push_str(&format!("Enabled: {}\n", plugin.enabled));
        output.push_str(&format!("State: {status}\n"));
        output.push_str(&format!(
            "Capabilities: {}\n",
            plugin
                .manifest
                .capabilities
                .iter()
                .map(PluginCapability::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        output.push_str(&format!(
            "Permissions network={:?} resource={:?} external={:?}\n",
            plugin.manifest.permissions.network_allow,
            plugin.manifest.permissions.resource_allow,
            plugin.manifest.permissions.external_allow
        ));
        output.push_str(&format!(
            "Limits http={} resource={} storage={}\n",
            plugin.manifest.limits.max_http_bytes,
            plugin.manifest.limits.max_resource_bytes,
            plugin.manifest.limits.max_storage_bytes
        ));
        output.push_str(&format!("Root: {}\n", plugin.manifest.root_dir.display()));
        output.push_str("Logs:\n");
        for entry in logs {
            output.push_str(&format!("- {} {:?}: {}\n", entry.timestamp.to_rfc3339(), entry.level, entry.message));
        }
        Ok(output)
    }

    fn push_log(&mut self, plugin_id: String, level: abi::LogLevel, mut message: String) {
        const MAX_PLUGIN_LOGS: usize = 200;
        const MAX_LOG_BYTES: usize = 8192;
        if message.len() > MAX_LOG_BYTES {
            let mut end = MAX_LOG_BYTES - '…'.len_utf8();
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push('…');
        }
        let logs = self.logs.entry(plugin_id.clone()).or_default();
        logs.push_back(PluginLogEntry {
            timestamp: chrono::Utc::now(),
            level,
            message: SharedString::from(message),
        });
        while logs.len() > MAX_PLUGIN_LOGS {
            logs.pop_front();
        }
        self.log_snapshots.insert(
            plugin_id,
            Arc::from(logs.iter().cloned().collect::<Vec<_>>()),
        );
    }

    // Rendering and initialization may emit logs, but must not apply UI,
    // navigation or other active effects through this path.
    fn record_passive_effects(&mut self, effects: Vec<abi::PluginEffect>) {
        for effect in effects {
            if let abi::PluginEffect::Log { plugin_id, level, message } = effect {
                self.push_log(plugin_id, level, message);
            }
        }
    }

    fn record_page_error(&mut self, key: &PageRenderCacheKey, message: &str) {
        warn!(plugin_id = %key.plugin_id, page_id = %key.page_id, error = %message, "plugin page render failed");
        self.push_log(
            key.plugin_id.clone(),
            abi::LogLevel::Error,
            format!("Page {}: {message}", key.page_id),
        );
        self.render_cache.page_errors.insert(key.clone(), Arc::from(message));
    }

    pub fn pages(&self) -> Vec<PluginPage> {
        self.pages.values().cloned().collect()
    }

    pub(crate) fn has_tools(&self) -> bool {
        self.pages.values().any(|page| self.is_enabled_tool(page))
    }

    pub(crate) fn is_tool(&self, plugin_id: &str, page_id: &str) -> bool {
        self.pages
            .get(&(plugin_id.to_string(), page_id.to_string()))
            .is_some_and(|page| self.is_enabled_tool(page))
    }

    fn is_enabled_tool(&self, page: &PluginPage) -> bool {
        page.navigation.is_some()
            && self.plugins.get(&page.plugin_id).is_some_and(|instance| {
                instance.enabled && !matches!(instance.state, PluginLoadState::Failed { .. })
            })
    }

    pub fn tool_pages(&self) -> Vec<PluginPage> {
        let mut pages = self
            .pages
            .values()
            .filter(|page| self.is_enabled_tool(page))
            .cloned()
            .collect::<Vec<_>>();
        pages.sort_by(|left, right| {
            let left_nav = left.navigation.as_ref();
            let right_nav = right.navigation.as_ref();
            left_nav
                .map_or(0, |navigation| navigation.order)
                .cmp(&right_nav.map_or(0, |navigation| navigation.order))
                .then_with(|| left.plugin_id.cmp(&right.plugin_id))
                .then_with(|| left.page_id.cmp(&right.page_id))
        });
        pages
    }

    pub fn page(&self, plugin_id: &str, page_id: &str) -> Option<PluginPage> {
        self.pages
            .get(&(plugin_id.to_string(), page_id.to_string()))
            .cloned()
    }

    fn default_page_registrations(manifest: &PluginManifest) -> Vec<PluginPageRegistration> {
        if !manifest.has_capability(&PluginCapability::UiPage) {
            return Vec::new();
        }

        vec![PluginPageRegistration {
            plugin_id: manifest.id.clone(),
            page_id: "main".to_string(),
            title: manifest.name.clone(),
            navigation: None,
        }]
    }

    fn insert_pages(
        pages_by_key: &mut BTreeMap<(String, String), PluginPage>,
        instance: &PluginInstance,
    ) {
        for page in &instance.pages {
            pages_by_key.insert(
                (page.plugin_id.clone(), page.page_id.clone()),
                PluginPage {
                    plugin_id: page.plugin_id.clone(),
                    page_id: page.page_id.clone(),
                    title: SharedString::from(page.title.clone()),
                    navigation: page.navigation.clone(),
                    icon_path: instance.prepared_resources.icon_path.clone(),
                },
            );
        }
    }

    fn open_modal(&mut self, request: abi::ModalRequest) -> Result<()> {
        let page_exists = self
            .pages
            .contains_key(&(request.plugin_id.clone(), request.page_id.clone()));
        if !page_exists {
            bail!(
                "unknown plugin modal page {}/{}",
                request.plugin_id,
                request.page_id
            );
        }

        self.active_modal = Some(PluginModalState {
            plugin_id: request.plugin_id,
            page_id: request.page_id,
            title: SharedString::from(request.title),
            width: request.width.clamp(320, 980),
            height: request.height.clamp(260, 760),
        });
        Ok(())
    }

    fn close_modal(&mut self) {
        self.active_modal = None;
    }

    fn active_modal(&self) -> Option<PluginModalState> {
        self.active_modal.clone()
    }

    pub fn cached_page(&self, plugin_id: &str, page_id: &str) -> Option<Arc<ViewTree>> {
        self.render_cache
            .pages
            .get(&PageRenderCacheKey {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.to_string(),
            })
            .map(|entry| entry.get().clone())
    }

    pub fn cached_page_error(&self, plugin_id: &str, page_id: &str) -> Option<Arc<str>> {
        self.render_cache
            .page_errors
            .get(&PageRenderCacheKey {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.to_string(),
            })
            .cloned()
    }

    pub fn is_page_pending(&self, plugin_id: &str, page_id: &str) -> bool {
        self.render_cache
            .pending_pages
            .contains_key(&PageRenderCacheKey {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.to_string(),
            })
    }

    pub fn cached_injections(
        &self,
        slot: InjectionSlot,
        page: Option<&str>,
    ) -> Option<Vec<RenderedInjection>> {
        self.render_cache
            .injections
            .iter()
            .find(|(key, _)| key.slot == slot && key.page.as_deref() == page)
            .map(|(_, trees)| trees.get().clone())
    }

    /// 渲染路径专用：该 slot 还没有缓存结果，但确实需要（重新）渲染注入。
    ///
    /// 只读判断，不加载插件、不执行 Wasm。已写入渲染缓存的结果（包括空结果）都视为
    /// 已渲染，因此渲染路径不会每帧重复登记请求；插件失效时缓存条目被移除，请求自然重新登记。
    pub fn injections_need_render(&self, slot: InjectionSlot, page: Option<&str>) -> bool {
        if self
            .render_cache
            .injections
            .keys()
            .any(|key| key.slot == slot && key.page.as_deref() == page)
        {
            return false;
        }
        self.has_injections(slot, page)
    }

    pub fn last_error(&self) -> Option<SharedString> {
        self.last_error.clone()
    }

    pub fn loaded_once(&self) -> bool {
        self.loaded_once
    }

    pub fn reload_all(&mut self) -> Result<()> {
        let prepared = prepare_plugin_reload_from_sources(
            &self.plugins_dir,
            &self.package_cache_dir,
            &self.cache_dir,
            &self.prepare_cache,
        )?;
        self.reload_prepared_manifests(prepared)
    }

    pub fn reload_manifests(&mut self, manifests: Vec<PluginManifest>) -> Result<()> {
        let prepared = prepare_plugin_manifests(
            manifests,
            &self.plugins_dir,
            &self.package_cache_dir,
            &self.prepare_cache,
        );
        self.reload_prepared_manifests(prepared)
    }

    /// 应用一次插件扫描结果。
    ///
    /// 只有源码发生变化的插件会被重建；未变化的插件保留自己的 Wasm `Store`、订阅、页面、
    /// 注入、资源缓存和渲染缓存，因此编辑一个插件不会再让其它插件的运行状态整体作废。
    /// 目录内容变化但源码语义未变的插件只失效缓存，不丢失运行状态。
    fn reload_prepared_manifests(&mut self, prepared: PreparedPluginReload) -> Result<()> {
        let next_generation = self.generation.saturating_add(1);
        let mut seen = BTreeSet::new();
        let mut next_plugins: BTreeMap<String, PluginInstance> = BTreeMap::new();
        let mut rebuilt = Vec::new();

        for prepared_plugin in prepared.plugins {
            let plugin_id = prepared_plugin.manifest.id.clone();
            if !seen.insert(plugin_id.clone()) {
                return Err(anyhow!("duplicate plugin id {plugin_id}"));
            }
            let matched = self
                .plugins
                .get(&plugin_id)
                .map(|instance| instance.compare_prepared(&prepared_plugin));
            let PreparedPluginManifest {
                manifest,
                enabled,
                translations,
                wasm,
                resources,
            } = prepared_plugin;

            match matched {
                Some(PreparedMatch::Unchanged) => {
                    let instance = self
                        .plugins
                        .remove(&plugin_id)
                        .expect("instance checked before reuse");
                    next_plugins.insert(plugin_id, instance);
                    continue;
                }
                Some(PreparedMatch::Invalidated) => {
                    let mut instance = self
                        .plugins
                        .remove(&plugin_id)
                        .expect("instance checked before reuse");
                    instance.prepared_resources.source_stamp = resources.source_stamp;
                    self.invalidate_plugin_caches(&plugin_id);
                    next_plugins.insert(plugin_id, instance);
                    continue;
                }
                Some(PreparedMatch::Rebuilt) | None => {}
            }

            if let Some(previous) = self.plugins.remove(&plugin_id) {
                let shutdown_reason = if enabled {
                    abi::ShutdownReason::Reload
                } else {
                    abi::ShutdownReason::Unload
                };
                self.retire(previous, shutdown_reason);
            }

            let pages = enabled
                .then(|| Self::default_page_registrations(&manifest))
                .unwrap_or_default();
            let instance = PluginInstance {
                manifest,
                generation: next_generation,
                pages,
                injections: Vec::new(),
                subscriptions: BTreeSet::new(),
                services: BTreeSet::new(),
                translations,
                state: PluginLoadState::Unloaded,
                enabled,
                prepared_wasm: wasm,
                prepared_resources: resources,
                runtime: None,
            };
            rebuilt.push(plugin_id.clone());
            next_plugins.insert(plugin_id, instance);
        }

        // 仍然留在注册表里的实例已不在扫描结果中，属于被卸载的插件。
        for (plugin_id, previous) in std::mem::take(&mut self.plugins) {
            self.retire(previous, abi::ShutdownReason::Unload);
            rebuilt.push(plugin_id);
        }
        for plugin_id in &rebuilt {
            self.invalidate_plugin_caches(plugin_id);
        }

        self.generation = next_generation;
        self.plugins = next_plugins;
        self.rebuild_event_subscribers();
        self.rebuild_registrations();
        self.retain_runtime_caches();
        self.loaded_once = true;
        self.publish_active_game_mods();
        Ok(())
    }

    fn retire(&self, instance: PluginInstance, reason: abi::ShutdownReason) {
        if self.background_activation {
            let _ = crate::tasks::runtime::spawn_io(async move {
                let _ = crate::tasks::runtime::blocking::run_io(move || {
                    instance.shutdown_logged(reason, "plugin shutdown failed");
                }).await;
            });
        } else {
            instance.shutdown_logged(reason, "plugin shutdown failed");
        }
    }

    /// 丢弃某个插件的渲染缓存与资源缓存，使它在下一次渲染时重新读取插件目录。
    fn invalidate_plugin_caches(&mut self, plugin_id: &str) {
        self.render_cache.invalidate_plugin(plugin_id);
        self.resource_cache.drop_plugin(plugin_id);
    }

    /// 从当前插件实例重建页面与注入注册表。
    fn rebuild_registrations(&mut self) {
        self.pages.clear();
        self.injections.clear();
        let mut reg = self.service_registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *reg = PluginServiceRegistry::new();
        let mut runtimes = self.service_runtimes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        runtimes.clear();
        for instance in self.plugins.values() {
            Self::insert_pages(&mut self.pages, instance);
            self.injections.extend(instance.injections.iter().cloned());
            for service_name in &instance.services {
                let _ = reg.register_service(instance.manifest.id.clone(), service_name.clone());
            }
            if let Some(runtime) = &instance.runtime {
                runtimes.insert(instance.manifest.id.clone(), runtime.clone());
            }
        }
        sort_injections(&mut self.injections);
    }

    /// 丢弃已卸载插件的资源缓存条目和不再被引用的 wasm 模块缓存。
    fn retain_runtime_caches(&mut self) {
        let installed = self.plugins.keys().cloned().collect::<BTreeSet<_>>();
        self.resource_cache.retain_plugins(&installed);
    }

    fn ensure_plugin_runtime(&mut self, plugin_id: &str) -> Result<()> {
        let Some(existing) = self.plugins.get(plugin_id) else {
            return Err(anyhow!("unknown plugin {plugin_id}"));
        };
        if !existing.enabled {
            return Err(anyhow!("plugin {plugin_id} is disabled"));
        }
        if let PluginLoadState::Failed { error, .. } = &existing.state {
            return Err(anyhow!("{}", error));
        }
        if existing.runtime.is_some() {
            return Ok(());
        }

        if self.background_activation {
            return Err(anyhow!("plugin {plugin_id} is waiting for background activation"));
        }
        let result = self.activation_context().activate(existing.clone());
        self.finish_activation(plugin_id, result)
    }

    fn finish_activation(&mut self, plugin_id: &str, result: Result<PluginInstance>) -> Result<()> {
        match result {
            Ok(instance) => {
                info!(
                    plugin_id = %instance.manifest.id,
                    name = %instance.manifest.name,
                    version = %instance.manifest.version,
                    authors = ?instance.manifest.authors,
                    capabilities = ?instance.manifest.capabilities,
                    pages = instance.pages.len(),
                    injections = instance.injections.len(),
                    subscriptions = instance.subscriptions.len(),
                    generation = instance.generation,
                    "plugin loaded"
                );
                self.push_log(
                    instance.manifest.id.clone(),
                    abi::LogLevel::Info,
                    plugin_loaded_log_message(&instance.manifest),
                );
                if let Some(runtime) = &instance.runtime {
                    let effects = runtime.lock().unwrap_or_else(|error| error.into_inner()).take_effects();
                    self.record_passive_effects(effects);
                }

                self.pages
                    .retain(|(page_plugin_id, _page_id), _page| page_plugin_id != plugin_id);
                self.injections
                    .retain(|registration| registration.plugin_id.as_str() != plugin_id);
                Self::insert_pages(&mut self.pages, &instance);
                self.injections.extend(instance.injections.clone());
                sort_injections(&mut self.injections);
                self.render_cache.invalidate_plugin(plugin_id);
                self.plugins.insert(plugin_id.to_string(), instance);
                self.publish_active_game_mods();
                self.rebuild_event_subscribers();
                self.rebuild_registrations();
                Ok(())
            }
            Err(error) => {
                let error_message = crate::plugins::manifest::format_error_chain(&error);
                if error_message.contains("resource-loading") {
                    debug!(
                        plugin_id,
                        error = %error_message,
                        "plugin lazy load deferred until resource cache is ready"
                    );
                    // Resource cache miss is transient. The async resource loader registered a
                    // plugin-wide subscriber and will request a refresh when bytes arrive. Keep the
                    // instance retryable and never roll back a valid installed package for this.
                    if let Some(instance) = self.plugins.get_mut(plugin_id) {
                        instance.state = PluginLoadState::Unloaded;
                        instance.runtime = None;
                    }
                    self.publish_active_game_mods();
                    return Err(anyhow!("{}", error_message));
                }

                warn!(
                    plugin_id,
                    error = %error_message,
                    "plugin lazy load failed"
                );
                if let Some(instance) = self.plugins.get(plugin_id) {
                    schedule_installed_package_rollback(
                        plugin_id.to_string(),
                        instance.manifest.clone(),
                    );
                }
                self.push_log(
                    plugin_id.to_string(),
                    abi::LogLevel::Error,
                    format!("Failed to load plugin: {error_message}"),
                );
                if let Some(instance) = self.plugins.get_mut(plugin_id) {
                    instance.state = PluginLoadState::Failed {
                        generation: instance.generation,
                        error: error_message.clone(),
                    };
                }
                self.last_error = Some(SharedString::from(error_message.clone()));
                self.publish_active_game_mods();
                Err(anyhow!("{}", error_message))
            }
        }
    }

    fn ensure_plugins_with_capability(&mut self, capability: PluginCapability) {
        let plugin_ids = self
            .plugins
            .values()
            .filter(|instance| instance.enabled)
            .filter(|instance| instance.manifest.has_capability(&capability))
            .filter(|instance| !matches!(instance.state, PluginLoadState::Failed { .. }))
            .filter(|instance| instance.runtime.is_none())
            .map(|instance| instance.manifest.id.clone())
            .collect::<Vec<_>>();

        for plugin_id in plugin_ids {
            if let Err(error) = self.ensure_plugin_runtime(&plugin_id) {
                warn!(plugin_id, error = %error, "plugin lazy load failed");
            }
        }
    }

    /// 安全休眠指定插件：触发 unload 并 drop Store，将 Wasm 线性内存归还操作系统
    pub fn hibernate_plugin(&mut self, plugin_id: &str) -> Result<bool> {
        let Some(instance) = self.plugins.get_mut(plugin_id) else {
            return Ok(false);
        };
        if instance.runtime.is_none() {
            return Ok(false);
        }
        let _ = instance.shutdown(abi::ShutdownReason::Unload);
        instance.runtime = None;
        instance.state = PluginLoadState::Unloaded;
        self.service_runtimes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(plugin_id);
        self.render_cache.invalidate_plugin(plugin_id);
        self.publish_active_game_mods();
        info!(
            plugin_id,
            "plugin hibernated; Store and Wasm linear memory released"
        );
        Ok(true)
    }

    /// 检查指定插件是否处于休眠状态（已启用但尚未物化 Store）
    pub fn is_hibernated(&self, plugin_id: &str) -> bool {
        self.plugins
            .get(plugin_id)
            .map(|inst| inst.enabled && inst.runtime.is_none())
            .unwrap_or(false)
    }

    /// 清理指定插件在宿主侧的渲染缓存与冷数据
    pub fn trim_plugin_caches(&mut self, plugin_id: &str) {
        self.render_cache.invalidate_plugin(plugin_id);
    }

    /// Trim the shared cold view cache under memory pressure. A large active UI
    /// must never cause its plugin Store to be hibernated or its page invalidated.
    pub fn enforce_memory_budgets(&mut self, system_memory_pressure: bool) {
        self.render_cache.trim_cold(if system_memory_pressure { 0 } else { VIEW_CACHE_SOFT_BYTES });
    }

    /// 调用跨插件服务，自动唤醒目标插件并防环/防重入
    pub fn call_plugin_service(
        &mut self,
        _caller_id: &str,
        request: abi::ServiceCallRequest,
    ) -> Result<Vec<u8>> {
        let target_plugin_id = if let Some(target) = &request.target_plugin {
            target.clone()
        } else {
            let reg = self
                .service_registry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let desc = reg.find_service(&request.service_name).ok_or_else(|| {
                anyhow!(
                    "service '{}' not found in service bus",
                    request.service_name
                )
            })?;
            desc.provider_plugin_id.clone()
        };

        self.ensure_plugin_runtime(&target_plugin_id)?;
        let _guard = self
            .rpc_stack
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .enter(&target_plugin_id)?;
        let (_, runtime) = self.plugin_runtime(&target_plugin_id)?;
        let mut exec = runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = exec.call_service(request).map_err(plugin_error_to_anyhow);
        let effects = exec.take_effects();
        if !effects.is_empty() {
            warn!(
                target_plugin_id,
                count = effects.len(),
                "plugin service emitted plugin effects; effects are discarded in launcher direct rpc call"
            );
        }
        result
    }

    fn activation_context(&self) -> PluginActivation {
        PluginActivation {
            engine: self.engine.clone(),
            cache_dir: self.cache_dir.join("plugin-components"),
            package_cache_dir: self.package_cache_dir.clone(),
            http_cache: self.http_cache.clone(),
            resource_cache: self.resource_cache.clone(),
            dispatcher: create_service_dispatcher(
                self.service_registry.clone(), self.service_runtimes.clone(), self.rpc_stack.clone(),
            ),
        }
    }

    pub fn render_page(&mut self, plugin_id: &str, page_id: &str) -> Result<Arc<ViewTree>> {
        self.ensure_plugin_runtime(plugin_id)?;

        let cache_key = PageRenderCacheKey {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
        };
        if let Some(tree) = self.render_cache.pages.get(&cache_key) {
            return Ok(tree.get().clone());
        }
        if let Some(error) = self.render_cache.page_errors.get(&cache_key) {
            return Err(anyhow!("{}", error));
        }

        let (manifest, runtime) = self.plugin_runtime(plugin_id)?;
        manifest.require_capability(PluginCapability::UiPage)?;

        let started = Instant::now();
        let request = abi::PageRenderRequest {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
        };
        let (tree, effects, poisoned) = {
            let mut runtime = runtime
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let tree = runtime.render_page(request)
                .map_err(plugin_error_to_anyhow)
                .and_then(view_tree_from_abi);
            (tree, runtime.take_effects(), runtime.is_poisoned())
        };
        self.record_passive_effects(effects);
        let elapsed = started.elapsed();
        if elapsed > RENDER_TIMEOUT {
            let error = format!("plugin render exceeded {RENDER_TIMEOUT:?}");
            self.record_page_error(&cache_key, &error);
            if poisoned {
                self.mark_unhealthy(plugin_id, error);
            }
            return Err(anyhow!("plugin render exceeded {:?}", RENDER_TIMEOUT));
        }
        if elapsed > RENDER_WARN_THRESHOLD {
            warn!(
                plugin_id,
                page_id,
                elapsed_ms = elapsed.as_millis(),
                "slow plugin render"
            );
        }

        match tree {
            Ok(tree) => {
                let tree = Arc::new(tree);
                self.render_cache.insert_page(cache_key, tree.clone());
                Ok(tree)
            }
            Err(error) => {
                let error_message = format!("{error:#}");
                self.record_page_error(&cache_key, &error_message);
                if poisoned {
                    self.mark_unhealthy(plugin_id, error_message);
                }
                Err(error)
            }
        }
    }

    pub fn render_injections(
        &mut self,
        slot: InjectionSlot,
        page: Option<&str>,
    ) -> Vec<RenderedInjection> {
        self.ensure_plugins_with_capability(PluginCapability::UiHook);

        let cache_key = InjectionRenderCacheKey {
            slot,
            page: page.map(str::to_string),
        };
        if let Some(trees) = self.render_cache.injections.get(&cache_key) {
            return trees.get().clone();
        }

        let registrations = self.injections.clone();
        let trees = registrations
            .into_iter()
            .filter(|registration| registration.slot == slot)
            .filter(|registration| {
                registration
                    .page
                    .as_deref()
                    .is_none_or(|registration_page| Some(registration_page) == page)
            })
            .filter_map(|registration| {
                self.render_injection(&registration.plugin_id, slot, page)
                    .inspect_err(|error| warn!(plugin_id = registration.plugin_id, error = %error))
                    .map(|tree| RenderedInjection {
                        plugin_id: registration.plugin_id,
                        tree: Arc::new(tree),
                        layout: registration.layout,
                    })
                    .ok()
            })
            .collect::<Vec<_>>();
        self.render_cache.insert_injections(cache_key, trees.clone());
        trees
    }

    pub fn has_injections(&self, slot: InjectionSlot, page: Option<&str>) -> bool {
        self.injections
            .iter()
            .filter(|registration| registration.slot == slot)
            .any(|registration| {
                registration
                    .page
                    .as_deref()
                    .is_none_or(|registration_page| Some(registration_page) == page)
            })
    }

    /// 是否存在尚未惰性加载的 UiHook 插件；此时注入注册表还不完整，
    /// 不能仅凭 `has_injections` 判定该 slot 没有注入。
    fn has_pending_ui_hook_loads(&self) -> bool {
        self.plugins.values().any(|instance| {
            instance.enabled
                && instance.manifest.has_capability(&PluginCapability::UiHook)
                && !matches!(instance.state, PluginLoadState::Failed { .. })
                && instance.runtime.is_none()
        })
    }

    fn render_injection(
        &mut self,
        plugin_id: &str,
        slot: InjectionSlot,
        page: Option<&str>,
    ) -> Result<ViewTree> {
        let (manifest, runtime) = self.plugin_runtime(plugin_id)?;
        manifest.require_capability(PluginCapability::UiHook)?;

        let request = abi::InjectionRequest {
            slot: injection_slot_to_abi(slot),
            page: page.map(str::to_string),
        };
        let mut runtime = runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tree = runtime.render_injection(request).map_err(plugin_error_to_anyhow);
        self.record_passive_effects(runtime.take_effects());

        tree?.map(view_tree_from_abi)
            .transpose()?
            .ok_or_else(|| anyhow!("plugin did not render injection for requested slot"))
    }

    /// 派发一次宿主事件，并把事件级联限制在 `cascade` 预算内。
    ///
    /// 插件通过 `emit_event` 产生的后续事件由调用方按队列继续派发，而不是在这里递归，
    /// 避免两个插件互相触发时无限展开主线程调用栈。
    pub(crate) fn handle_event(
        &mut self,
        event: PluginEvent,
        cascade: &mut EventCascade,
    ) -> Vec<abi::PluginEffect> {
        let started = Instant::now();
        match &event.kind {
            PluginEventKind::Action { .. } | PluginEventKind::JobReady { .. } => {
                for plugin_id in self.event_targets(&event) {
                    if let Err(error) = self.ensure_plugin_runtime(&plugin_id) {
                        warn!(plugin_id, error = %error, "plugin action target load failed");
                    }
                }
            }
            PluginEventKind::Global { .. } | PluginEventKind::RouteChanged { .. } => {
                self.ensure_plugins_with_capability(PluginCapability::EventGlobal);
            }
        }
        let targets = self.event_targets(&event);
        self.invalidate_render_cache_for_event(&event, &targets);
        let event_name = event.cascade_name();
        let mut effects = Vec::new();
        let mut budget_warned = false;
        for plugin_id in targets {
            if !cascade.try_deliver(&plugin_id, event_name) {
                if !budget_warned {
                    budget_warned = true;
                    warn!(
                        plugin_id,
                        event = event_name,
                        deliveries = cascade.deliveries(),
                        "plugin event cascade budget exhausted; skipping remaining deliveries"
                    );
                }
                continue;
            }
            debug!(plugin_id, event = ?event.kind, "dispatch plugin event");
            let Some(runtime) = self
                .plugins
                .get(&plugin_id)
                .and_then(|plugin| plugin.runtime.clone())
            else {
                continue;
            };
            let event = plugin_event_to_abi(&event);
            let result = {
                let mut runtime = runtime
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let result: Result<()> =
                    runtime.handle_event(event).map_err(plugin_error_to_anyhow);
                effects.extend(runtime.take_effects());
                result
            };
            if let Err(error) = result {
                let error = error.to_string();
                warn!(plugin_id, error = %error, "plugin event failed");
                self.push_log(plugin_id.clone(), abi::LogLevel::Error, format!("Event: {error}"));
                self.mark_unhealthy(&plugin_id, error);
            }
        }
        let elapsed = started.elapsed();
        if elapsed > EVENT_TIMEOUT {
            warn!(
                elapsed_ms = elapsed.as_millis(),
                "plugin event dispatch exceeded budget"
            );
        }
        effects
    }

    fn invalidate_render_cache_for_event(&mut self, event: &PluginEvent, targets: &[String]) {
        match &event.kind {
            PluginEventKind::Action { .. } | PluginEventKind::JobReady { .. } => {
                if let Some(plugin_id) = event.plugin_id.as_deref() {
                    self.render_cache
                        .invalidate_page(plugin_id, event.page_id.as_deref());
                }
            }
            PluginEventKind::Global { .. } | PluginEventKind::RouteChanged { .. } => {
                for plugin_id in targets {
                    self.render_cache.invalidate_plugin(plugin_id);
                }
                self.render_cache.invalidate_all_injections();
            }
        }
    }

    /// 重建事件订阅倒排索引。
    ///
    /// 只在插件实例被创建或整体重载后调用；事件派发本身只读取索引，不做全量插件扫描。
    fn rebuild_event_subscribers(&mut self) {
        self.event_subscribers.clear();
        for instance in self.plugins.values() {
            for event in &instance.subscriptions {
                self.event_subscribers
                    .entry(event.clone())
                    .or_default()
                    .insert(instance.manifest.id.clone());
            }
        }
    }

    fn subscribers_of(&self, event_name: &str) -> Vec<String> {
        self.event_subscribers
            .get(event_name)
            .map(|subscribers| subscribers.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn event_targets(&self, event: &PluginEvent) -> Vec<String> {
        match &event.kind {
            PluginEventKind::Global { name, .. } => self.subscribers_of(name).into_iter()
                .filter(|id| event.plugin_id.as_ref().is_none_or(|target| target == id)).collect(),
            PluginEventKind::RouteChanged { .. } => self.subscribers_of(ROUTE_CHANGED_EVENT),
            PluginEventKind::Action { .. } | PluginEventKind::JobReady { .. } => event.plugin_id.iter().cloned().collect(),
        }
    }

    fn event_requires_clipboard_snapshot(&self, event: &PluginEvent) -> bool {
        match &event.kind {
            PluginEventKind::JobReady { .. } => false,
            PluginEventKind::Action { .. } => event.plugin_id.as_deref().is_some_and(|plugin_id| {
                self.plugins.get(plugin_id).is_some_and(|plugin| {
                    plugin.enabled
                        && plugin
                            .manifest
                            .has_capability(&PluginCapability::ClipboardRead)
                })
            }),
            PluginEventKind::Global { .. } | PluginEventKind::RouteChanged { .. } => {
                self.event_targets(event).iter().any(|plugin_id| {
                    self.plugins.get(plugin_id).is_some_and(|plugin| {
                        plugin.enabled
                            && plugin
                                .manifest
                                .has_capability(&PluginCapability::ClipboardRead)
                    })
                })
            }
        }
    }

    fn plugin_runtime(
        &self,
        plugin_id: &str,
    ) -> Result<(PluginManifest, Arc<Mutex<abi::PluginExecution>>)> {
        let instance = self
            .plugins
            .get(plugin_id)
            .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
        let runtime = instance
            .runtime
            .clone()
            .ok_or_else(|| anyhow!("plugin {plugin_id} is not loaded"))?;
        Ok((instance.manifest.clone(), runtime))
    }

    fn mark_unhealthy(&mut self, plugin_id: &str, error: String) {
        warn!(plugin_id, error = %error, "plugin runtime marked unhealthy");
        if let Some(instance) = self.plugins.get_mut(plugin_id) {
            instance.state = PluginLoadState::Failed {
                generation: instance.generation,
                error,
            };
        }
        self.publish_active_game_mods();
    }

    fn invalidate_from_plugin(&mut self, plugin_id: &str, target: abi::InvalidateTarget) {
        match target {
            abi::InvalidateTarget::All => {
                self.render_cache.invalidate_plugin(plugin_id);
            }
            abi::InvalidateTarget::Page(page_id) => {
                self.render_cache
                    .invalidate_plugin_page(plugin_id, &page_id);
            }
            abi::InvalidateTarget::Injection(request) => {
                self.render_cache.invalidate_plugin_injection(
                    plugin_id,
                    injection_slot_from_abi(request.slot),
                    request.page.as_deref(),
                );
            }
        }
    }

    fn apply_async_app_refreshes(&mut self) -> bool {
        let mut invalidations = self.http_cache.drain_finished();
        invalidations.extend(self.resource_cache.drain_finished());
        if invalidations.is_empty() {
            return false;
        }
        for invalidation in invalidations {
            match invalidation {
                PluginInvalidationTarget::Plugin { plugin_id } => {
                    self.render_cache.invalidate_plugin(&plugin_id);
                }
                PluginInvalidationTarget::Page { plugin_id, page_id } => {
                    self.render_cache
                        .invalidate_plugin_page(&plugin_id, &page_id);
                }
                PluginInvalidationTarget::Injection {
                    plugin_id,
                    slot,
                    page,
                } => {
                    self.render_cache.invalidate_plugin_injection(
                        &plugin_id,
                        slot,
                        page.as_deref(),
                    );
                }
            }
        }
        true
    }

    fn has_finished_async_app_refreshes(&self) -> bool {
        self.http_cache.has_finished_refreshes() || self.resource_cache.has_finished_refreshes()
    }

    pub fn set_watcher(&mut self, sender: crate::plugins::watcher::Sender, task: gpui::Task<()>) {
        self.reload_tx = Some(sender);
        self.watcher_task = Some(task);
    }
}

impl PluginInstance {
    /// 判断已加载实例与本次扫描结果的关系。
    ///
    /// 失败实例总是重建：插件失败后必须能在下一次重载里重新尝试，而不是永久停留在
    /// 失败状态。
    fn compare_prepared(&self, prepared: &PreparedPluginManifest) -> PreparedMatch {
        let same_plugin = self.enabled == prepared.enabled
            && self.manifest == prepared.manifest
            && self.translations == prepared.translations
            && self.prepared_wasm.matches(&prepared.wasm)
            && self.prepared_resources.same_content(&prepared.resources);
        if !same_plugin || matches!(self.state, PluginLoadState::Failed { .. }) {
            return PreparedMatch::Rebuilt;
        }
        if self.prepared_resources.source_stamp == prepared.resources.source_stamp {
            PreparedMatch::Unchanged
        } else {
            PreparedMatch::Invalidated
        }
    }

    /// 关闭插件运行实例；失败只记录日志，不阻断重载流程。
    fn shutdown_logged(&self, reason: abi::ShutdownReason, message: &'static str) {
        if !matches!(self.state, PluginLoadState::Loaded { .. }) {
            return;
        }
        if let Err(error) = self.shutdown(reason) {
            warn!(
                plugin_id = self.manifest.id,
                error = %crate::plugins::manifest::format_error_chain(&error),
                "{message}"
            );
        }
    }

    fn shutdown(&self, reason: abi::ShutdownReason) -> Result<()> {
        let Some(runtime) = self.runtime.clone() else {
            return Ok(());
        };

        let mut runtime = runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        runtime
            .shutdown(reason)
            .map_err(plugin_error_to_anyhow)?;
        let effects = runtime.take_effects();
        if !effects.is_empty() {
            warn!(
                plugin_id = self.manifest.id,
                count = effects.len(),
                "plugin shutdown emitted plugin effects; effects are ignored during shutdown"
            );
        }
        Ok(())
    }
}





/// 把 manifest 的 capability/allowlist/limits 解析为运行时沙箱策略。
fn plugin_policy_from_manifest(manifest: &PluginManifest) -> abi::PluginPolicy {
    let capabilities = &manifest.capabilities;
    abi::PluginPolicy {
        plugin_id: manifest.id.clone(),
        capabilities: abi::PluginCapabilities {
            ui_page: capabilities.contains(&PluginCapability::UiPage),
            ui_window: capabilities.contains(&PluginCapability::UiWindow),
            ui_hook: capabilities.contains(&PluginCapability::UiHook),
            event_global: capabilities.contains(&PluginCapability::EventGlobal),
            toast: capabilities.contains(&PluginCapability::Toast),
            network_http: capabilities.contains(&PluginCapability::NetworkHttp),
            clipboard_read: capabilities.contains(&PluginCapability::ClipboardRead),
            clipboard_write: capabilities.contains(&PluginCapability::ClipboardWrite),
            external_open: capabilities.contains(&PluginCapability::ExternalOpen),
            resource_read: capabilities.contains(&PluginCapability::ResourceRead),
            storage_kv: capabilities.contains(&PluginCapability::StorageKv),
            task_progress: capabilities.contains(&PluginCapability::TaskProgress),
            config_read: capabilities.contains(&PluginCapability::ConfigRead),
            config_write: capabilities.contains(&PluginCapability::ConfigWrite),
            download_start: capabilities.contains(&PluginCapability::DownloadStart),
        },
        limits: abi::PluginLimits {
            max_http_bytes: manifest.limits.max_http_bytes,
            max_resource_bytes: manifest.limits.max_resource_bytes,
            max_storage_bytes: manifest.limits.max_storage_bytes,
        },
        network_allow: abi::Allowlist::new(manifest.permissions.network_allow.clone()),
        resource_allow: abi::Allowlist::new(manifest.permissions.resource_allow.clone()),
        external_allow: abi::Allowlist::new(manifest.permissions.external_allow.clone()),
    }
}

fn plugin_loaded_log_message(manifest: &PluginManifest) -> String {
    let authors = if manifest.authors.is_empty() {
        "unknown".to_string()
    } else {
        manifest.authors.join(", ")
    };
    let capabilities = manifest
        .capabilities
        .iter()
        .map(PluginCapability::as_str)
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "Loaded {} {} by {} ({})",
        manifest.name, manifest.version, authors, capabilities
    )
}

fn current_theme_snapshot(cx: &App) -> abi::ThemeSnapshot {
    let theme = cx.global::<ThemeState>();
    let dark_factor = theme.factor(Instant::now());
    let colors = lerp_theme_colors(
        &LightColors::theme(),
        &DarkColors::theme(),
        dark_factor,
        theme.accent,
    );
    theme_snapshot_from_colors(theme.target_dark, dark_factor, colors)
}

fn clipboard_text_snapshot(cx: &mut App) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

fn clipboard_text_snapshot_for_event(cx: &mut App, event: &PluginEvent) -> Option<Option<String>> {
    let required = cx
        .global::<PluginRegistry>()
        .event_requires_clipboard_snapshot(event);
    required.then(|| clipboard_text_snapshot(cx))
}

fn theme_snapshot_from_colors(
    target_dark: bool,
    dark_factor: f32,
    colors: ThemeColors,
) -> abi::ThemeSnapshot {
    abi::ThemeSnapshot {
        mode: if target_dark {
            abi::ThemeMode::Dark
        } else {
            abi::ThemeMode::Light
        },
        dark_factor,
        accent: theme_color_from_hsla(colors.accent),
        primary_text: theme_color_from_hsla(colors.text_primary),
        secondary_text: theme_color_from_hsla(colors.text_secondary),
        muted_text: theme_color_from_hsla(colors.text_muted),
        surface: theme_color_from_hsla(colors.settings_card_bg),
        surface_hover: theme_color_from_hsla(colors.surface_hover),
        border: theme_color_from_hsla(colors.border),
        danger: theme_color_from_hsla(colors.danger),
    }
}

fn theme_color_from_hsla(color: Hsla) -> abi::ThemeColor {
    abi::ThemeColor {
        h: color.h,
        s: color.s,
        l: color.l,
        a: color.a,
    }
}





fn schedule_plugin_persistence(
    operation: PluginPersistenceOp,
) -> Result<(), abi::PluginError> {
    let sender = if let Some(sender) = PLUGIN_PERSISTENCE_TX.get() {
        sender
    } else {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        start_plugin_persistence_worker(receiver).map_err(|error| abi::PluginError {
            code: "persistence-schedule-failed".to_string(),
            message: error,
        })?;
        let _ = PLUGIN_PERSISTENCE_TX.set(sender);
        PLUGIN_PERSISTENCE_TX.get().ok_or_else(|| abi::PluginError {
            code: "persistence-schedule-failed".to_string(),
            message: "plugin persistence queue initialization failed".to_string(),
        })?
    };

    sender.send(operation).map_err(|error| abi::PluginError {
        code: "persistence-schedule-failed".to_string(),
        message: format!("plugin persistence queue is unavailable: {error}"),
    })
}

fn start_plugin_persistence_worker(
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<PluginPersistenceOp>,
) -> std::result::Result<(), String> {
    crate::tasks::runtime::spawn_io(async move {
        while let Some(first) = receiver.recv().await {
            let mut pending = BTreeMap::new();
            pending.insert(first.key(), first);

            tokio::time::sleep(PLUGIN_PERSISTENCE_COALESCE_DELAY).await;
            while let Ok(operation) = receiver.try_recv() {
                pending.insert(operation.key(), operation);
            }

            let operations = pending.into_values().collect::<Vec<_>>();
            let persisted = crate::tasks::runtime::blocking::run_io(move || {
                operations
                    .into_iter()
                    .map(|operation| {
                        let plugin_id = operation.plugin_id().to_string();
                        (plugin_id, persist_plugin_operation(operation))
                    })
                    .collect::<Vec<_>>()
            })
            .await;

            match persisted {
                Ok(results) => {
                    for (plugin_id, result) in results {
                        if let Err(error) = result {
                            warn!(
                                plugin_id,
                                error = %crate::plugins::manifest::format_error_chain(&error),
                                "plugin background persistence failed"
                            );
                        }
                    }
                }
                Err(error) => {
                    warn!(error = %error, "plugin persistence worker blocking task failed");
                }
            }
        }
    })
    .map(|_| ())
}

fn persist_plugin_operation(operation: PluginPersistenceOp) -> Result<()> {
    match operation {
        PluginPersistenceOp::Config {
            manifest, content, ..
        } => crate::plugins::manifest::write_user_config(&manifest, &content),
        PluginPersistenceOp::StorageSet {
            storage_dir,
            key,
            value,
            ..
        } => persist_storage_value(&storage_dir, &key, &value)
            .map_err(|error| anyhow!("{}: {}", error.code, error.message)),
        PluginPersistenceOp::StorageDelete {
            storage_dir, key, ..
        } => storage_delete(&storage_dir, &key)
            .map_err(|error| anyhow!("{}: {}", error.code, error.message)),
    }
}

fn storage_snapshot_next_used_bytes(
    values: &BTreeMap<String, String>,
    key: &str,
    value: &str,
) -> u64 {
    let used = values
        .values()
        .map(|value| value.len() as u64)
        .fold(0_u64, u64::saturating_add);
    used.saturating_sub(
        values
            .get(key)
            .map(|current| current.len() as u64)
            .unwrap_or(0),
    )
    .saturating_add(value.len() as u64)
}

fn ensure_blocking_io_allowed(
    render_context: Option<&RenderContext>,
    operation: &'static str,
) -> Result<(), abi::PluginError> {
    if render_context.is_none() {
        return Ok(());
    }

    Err(abi::PluginError {
        code: "blocking-io-render-denied".to_string(),
        message: format!(
            "{operation} is not allowed while rendering plugin UI; load or persist data during init/event handling and keep render state in memory"
        ),
    })
}



fn manifest_has_any_readme(manifest: &PluginManifest) -> bool {
    manifest.readme_path(None).is_some_and(|path| path.exists())
        || manifest
            .readme_locales
            .values()
            .any(|path| manifest.root_dir.join(path).exists())
}


fn load_storage_snapshot(
    storage_dir: &Path,
) -> Result<BTreeMap<String, String>, abi::PluginError> {
    let mut values = BTreeMap::new();
    for key in storage_list(storage_dir, None)? {
        if let Some(value) = storage_get(storage_dir, &key)? {
            values.insert(key, value);
        }
    }
    Ok(values)
}

fn storage_get(
    storage_dir: &Path,
    key: &str,
) -> Result<Option<String>, abi::PluginError> {
    let path = storage_path(storage_dir, key)?;
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| abi::PluginError {
            code: "storage-read-failed".to_string(),
            message: format!("read plugin storage key {key} failed: {error}"),
        })
}

fn storage_set(
    storage_dir: &Path,
    key: &str,
    value: &str,
    quota_bytes: u64,
) -> Result<(), abi::PluginError> {
    let path = storage_path(storage_dir, key)?;
    let existing_len = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let used = storage_used_bytes(storage_dir)?;
    let next_used = used
        .saturating_sub(existing_len)
        .saturating_add(value.len() as u64);
    if next_used > quota_bytes {
        return Err(abi::PluginError {
            code: "storage-quota-exceeded".to_string(),
            message: format!("plugin storage quota exceeded ({next_used}/{quota_bytes} bytes)"),
        });
    }
    persist_storage_value(storage_dir, key, value)
}

fn persist_storage_value(
    storage_dir: &Path,
    key: &str,
    value: &str,
) -> Result<(), abi::PluginError> {
    let path = storage_path(storage_dir, key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| abi::PluginError {
            code: "storage-create-failed".to_string(),
            message: format!("create plugin storage {} failed: {error}", parent.display()),
        })?;
    }
    fs::write(&path, value).map_err(|error| abi::PluginError {
        code: "storage-write-failed".to_string(),
        message: format!("write plugin storage key {key} failed: {error}"),
    })
}

fn storage_delete(storage_dir: &Path, key: &str) -> Result<(), abi::PluginError> {
    let path = storage_path(storage_dir, key)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| abi::PluginError {
            code: "storage-delete-failed".to_string(),
            message: format!("delete plugin storage key {key} failed: {error}"),
        })?;
    }
    Ok(())
}

fn storage_list(
    storage_dir: &Path,
    prefix: Option<&str>,
) -> Result<Vec<String>, abi::PluginError> {
    if let Some(prefix) = prefix {
        validate_storage_key_prefix(prefix)?;
    }
    if !storage_dir.exists() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(storage_dir).map_err(|error| abi::PluginError {
        code: "storage-list-failed".to_string(),
        message: format!(
            "list plugin storage {} failed: {error}",
            storage_dir.display()
        ),
    })? {
        let entry = entry.map_err(|error| abi::PluginError {
            code: "storage-list-failed".to_string(),
            message: format!(
                "list plugin storage {} failed: {error}",
                storage_dir.display()
            ),
        })?;
        if !entry
            .file_type()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(key) = storage_key_from_file_name(&file_name) else {
            continue;
        };
        if prefix.is_none_or(|prefix| key.starts_with(prefix)) {
            keys.push(key);
        }
    }
    keys.sort();
    Ok(keys)
}

fn storage_path(storage_dir: &Path, key: &str) -> Result<PathBuf, abi::PluginError> {
    validate_storage_key(key)?;
    Ok(storage_dir.join(storage_file_name(key)))
}

/// 校验 KV key；委托运行时沙箱规则，但保留旧 ABI 的错误码。
fn validate_storage_key(key: &str) -> Result<(), abi::PluginError> {
    abi::sandbox::validate_storage_key(key)
        .map_err(|error| abi::PluginError::new("invalid-storage-key", error.message))
}

/// 校验 KV key 前缀；委托运行时沙箱规则，但保留旧 ABI 的错误码。
fn validate_storage_key_prefix(prefix: &str) -> Result<(), abi::PluginError> {
    abi::sandbox::validate_storage_key_prefix(prefix)
        .map_err(|error| abi::PluginError::new("invalid-storage-key", error.message))
}

fn storage_file_name(key: &str) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(3 + key.len() * 2);
    output.push_str("kv-");
    for byte in key.as_bytes() {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn storage_key_from_file_name(file_name: &str) -> Option<String> {
    let hex = file_name.strip_prefix("kv-")?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair).ok()?;
        let byte = u8::from_str_radix(pair, 16).ok()?;
        bytes.push(byte);
    }
    let key = String::from_utf8(bytes).ok()?;
    validate_storage_key(&key).ok()?;
    Some(key)
}

fn storage_used_bytes(storage_dir: &Path) -> Result<u64, abi::PluginError> {
    if !storage_dir.exists() {
        return Ok(0);
    }
    let mut used = 0_u64;
    for entry in fs::read_dir(storage_dir).map_err(|error| abi::PluginError {
        code: "storage-quota-check-failed".to_string(),
        message: format!(
            "read plugin storage {} failed: {error}",
            storage_dir.display()
        ),
    })? {
        let entry = entry.map_err(|error| abi::PluginError {
            code: "storage-quota-check-failed".to_string(),
            message: format!(
                "read plugin storage {} failed: {error}",
                storage_dir.display()
            ),
        })?;
        if let Ok(metadata) = entry.metadata()
            && metadata.is_file()
        {
            used = used.saturating_add(metadata.len());
        }
    }
    Ok(used)
}

fn sanitize_storage_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

impl RenderCache {
    fn insert_page(&mut self, key: PageRenderCacheKey, tree: Arc<ViewTree>) {
        self.pages.insert(key, CachedView::new(tree));
        self.trim_cold(VIEW_CACHE_SOFT_BYTES);
    }

    fn insert_injections(&mut self, key: InjectionRenderCacheKey, trees: Vec<RenderedInjection>) {
        self.injections.insert(key, CachedView::new(trees));
        self.trim_cold(VIEW_CACHE_SOFT_BYTES);
    }

    fn trim_cold(&mut self, target: usize) {
        // Only drop host cache ownership. Never revoke an active view or a GPUI
        // asset lease. Active working sets may exceed the soft target.
        enum Key {
            Page(PageRenderCacheKey),
            Injection(InjectionRenderCacheKey),
        }
        let now = Instant::now();
        let mut bytes = 0_usize;
        let mut candidates = Vec::new();
        for (key, entry) in &self.pages {
            let cost = entry.value.estimated_retained_bytes();
            bytes = bytes.saturating_add(cost);
            let last = entry.last_used.get();
            if Arc::strong_count(&entry.value) == 1
                && now.saturating_duration_since(last) >= VIEW_CACHE_HOT_AGE
            {
                candidates.push((last, cost, Key::Page(key.clone())));
            }
        }
        for (key, entry) in &self.injections {
            let cost = entry.value.iter().fold(0_usize, |bytes, tree|
                bytes.saturating_add(tree.tree.estimated_retained_bytes()));
            bytes = bytes.saturating_add(cost);
            let last = entry.last_used.get();
            if now.saturating_duration_since(last) >= VIEW_CACHE_HOT_AGE
                && entry.value.iter().all(|tree| Arc::strong_count(&tree.tree) == 1)
            {
                candidates.push((last, cost, Key::Injection(key.clone())));
            }
        }
        if bytes <= target {
            return;
        }
        candidates.sort_unstable_by_key(|(last, _, _)| *last);
        let mut evicted = 0;
        for (_, cost, key) in candidates {
            if bytes <= target {
                break;
            }
            match key {
                Key::Page(key) => { self.pages.remove(&key); }
                Key::Injection(key) => { self.injections.remove(&key); }
            }
            bytes = bytes.saturating_sub(cost);
            evicted += 1;
        }
        if evicted != 0 {
            debug!(evicted, retained_bytes = bytes, target_bytes = target, "trimmed cold plugin views");
        }
    }

    fn next_ticket(&mut self) -> u64 {
        self.ticket = self.ticket.wrapping_add(1);
        self.ticket
    }

    fn invalidate_plugin(&mut self, plugin_id: &str) {
        self.pending_injections.clear();
        self.pages
            .retain(|key, _tree| key.plugin_id.as_str() != plugin_id);
        self.page_errors
            .retain(|key, _error| key.plugin_id.as_str() != plugin_id);
        self.pending_pages
            .retain(|key, _| key.plugin_id.as_str() != plugin_id);
        for trees in self.injections.values_mut() {
            trees.value.retain(|tree| tree.plugin_id.as_str() != plugin_id);
        }
        self.injections.retain(|_key, trees| !trees.value.is_empty());
    }

    fn invalidate_page(&mut self, plugin_id: &str, page_id: Option<&str>) {
        self.pending_injections.clear();
        self.pages.retain(|key, _tree| {
            if key.plugin_id.as_str() != plugin_id {
                return true;
            }
            page_id.is_some_and(|page_id| key.page_id.as_str() != page_id)
        });
        self.page_errors.retain(|key, _error| {
            if key.plugin_id.as_str() != plugin_id {
                return true;
            }
            page_id.is_some_and(|page_id| key.page_id.as_str() != page_id)
        });
        self.pending_pages.retain(|key, _| {
            if key.plugin_id.as_str() != plugin_id {
                return true;
            }
            page_id.is_some_and(|page_id| key.page_id.as_str() != page_id)
        });
        self.injections.clear();
    }

    fn invalidate_all_injections(&mut self) {
        self.pending_injections.clear();
        self.injections.clear();
    }

    fn invalidate_plugin_page(&mut self, plugin_id: &str, page_id: &str) {
        self.pending_injections.clear();
        self.pages.retain(|key, _tree| {
            key.plugin_id.as_str() != plugin_id || key.page_id.as_str() != page_id
        });
        self.page_errors.retain(|key, _error| {
            key.plugin_id.as_str() != plugin_id || key.page_id.as_str() != page_id
        });
        self.pending_pages
            .retain(|key, _| key.plugin_id.as_str() != plugin_id || key.page_id.as_str() != page_id);
        self.invalidate_plugin_injections(plugin_id);
    }

    fn invalidate_plugin_injections(&mut self, plugin_id: &str) {
        self.pending_injections.clear();
        for trees in self.injections.values_mut() {
            trees.value.retain(|tree| tree.plugin_id.as_str() != plugin_id);
        }
        self.injections.retain(|_key, trees| !trees.value.is_empty());
    }

    fn invalidate_plugin_injection(
        &mut self,
        plugin_id: &str,
        slot: InjectionSlot,
        page: Option<&str>,
    ) {
        self.pending_injections.clear();
        self.injections.retain(|key, trees| {
            if key.slot != slot {
                return true;
            }
            if key.page.as_deref() != page {
                return true;
            }
            trees.value.retain(|tree| tree.plugin_id.as_str() != plugin_id);
            !trees.value.is_empty()
        });
    }

    fn plugin_memory(&self, plugin_id: &str) -> PluginRenderCacheMemory {
        let mut memory = PluginRenderCacheMemory::default();
        for (key, tree) in &self.pages {
            if key.plugin_id == plugin_id {
                memory.entries = memory.entries.saturating_add(1);
                memory.estimated_bytes = memory
                    .estimated_bytes
                    .saturating_add(tree.value.estimated_retained_bytes());
            }
        }
        for (key, error) in &self.page_errors {
            if key.plugin_id == plugin_id {
                memory.entries = memory.entries.saturating_add(1);
                memory.estimated_bytes = memory
                    .estimated_bytes
                    .saturating_add(std::mem::size_of::<Arc<str>>())
                    .saturating_add(error.len());
            }
        }
        for trees in self.injections.values() {
            for tree in &trees.value {
                if tree.plugin_id == plugin_id {
                    memory.entries = memory.entries.saturating_add(1);
                    memory.estimated_bytes = memory
                        .estimated_bytes
                        .saturating_add(tree.tree.estimated_retained_bytes());
                }
            }
        }
        memory
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginRenderCacheMemory {
    entries: usize,
    estimated_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginHttpCacheMemory {
    entries: usize,
    body_bytes: usize,
    error_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginResourceCacheMemory {
    entries: usize,
    bytes: usize,
    error_bytes: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginLogMemory {
    entries: usize,
    bytes: usize,
}

impl PluginHttpFetchCache {
    pub(crate) fn snapshot(
        &self,
        manifest: &PluginManifest,
        plugin_id: &str,
        render_context: Option<&RenderContext>,
        url: &str,
        ttl_seconds: u32,
        max_bytes: u32,
    ) -> std::result::Result<HttpCacheSnapshot, abi::PluginError> {
        manifest
            .require_capability(PluginCapability::NetworkHttp)
            .map_err(|error| abi::PluginError {
                code: "capability-denied".to_string(),
                message: error.to_string(),
            })?;
        if !manifest.allows_network_url(url) {
            return Err(abi::PluginError {
                code: "network-url-denied".to_string(),
                message: format!("plugin {plugin_id} is not allowed to request {url}"),
            });
        }
        if !url.starts_with("https://") {
            return Err(abi::PluginError {
                code: "network-url-invalid".to_string(),
                message: "plugin HTTP requests must use https:// URLs".to_string(),
            });
        }

        let ttl = if ttl_seconds == 0 {
            HTTP_DEFAULT_TTL
        } else {
            Duration::from_secs(u64::from(ttl_seconds))
        };
        let requested_max_bytes = usize::try_from(max_bytes).unwrap_or(HTTP_MAX_BYTES);
        let manifest_max_bytes =
            usize::try_from(manifest.limits.max_http_bytes).unwrap_or(HTTP_MAX_BYTES);
        let max_bytes = requested_max_bytes.clamp(1, manifest_max_bytes.min(HTTP_MAX_BYTES));
        let subscriber = render_context.map(|context| match context {
            RenderContext::Page { page_id } => PluginInvalidationTarget::Page {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.clone(),
            },
            RenderContext::Injection { slot, page } => PluginInvalidationTarget::Injection {
                plugin_id: plugin_id.to_string(),
                slot: *slot,
                page: page.clone(),
            },
        });

        let mut state = self.state.lock().map_err(|_| abi::PluginError {
            code: "network-cache-lock-failed".to_string(),
            message: "plugin HTTP cache lock failed".to_string(),
        })?;
        let sender = state.sender.clone();
        let entry = state
            .entries
            .entry(url.to_string())
            .or_insert_with(|| HttpCacheEntry {
                body: None,
                error: None,
                fetched_at: None,
                fetched_at_unix_ms: None,
                refreshing: false,
                subscribers: BTreeSet::new(),
            });
        if let Some(subscriber) = subscriber {
            entry.subscribers.insert(subscriber);
        }

        let is_fresh = entry
            .fetched_at
            .is_some_and(|fetched_at| fetched_at.elapsed() <= ttl);
        let should_refresh = !is_fresh && !entry.refreshing;
        if should_refresh {
            entry.refreshing = true;
            spawn_http_refresh(
                url.to_string(),
                max_bytes,
                sender,
                self.finished_refresh_pending.clone(),
            );
        }

        let state = if is_fresh {
            abi::HttpCacheState::Fresh
        } else if entry.body.is_some() {
            abi::HttpCacheState::Stale
        } else if entry.error.is_some() {
            abi::HttpCacheState::Error
        } else {
            abi::HttpCacheState::Loading
        };
        Ok(HttpCacheSnapshot {
            state,
            body: entry.body.clone(),
            error: entry.error.clone(),
            fetched_at_unix_ms: entry.fetched_at_unix_ms,
        })
    }

    fn drain_finished(&self) -> Vec<PluginInvalidationTarget> {
        if !self.finished_refresh_pending.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }

        let receiver = {
            let Ok(state) = self.state.lock() else {
                self.finished_refresh_pending.store(true, Ordering::Release);
                return Vec::new();
            };
            state.receiver.clone()
        };

        let mut results = Vec::new();
        let Ok(receiver) = receiver.lock() else {
            self.finished_refresh_pending.store(true, Ordering::Release);
            return Vec::new();
        };
        while let Ok(result) = receiver.try_recv() {
            results.push(result);
        }
        drop(receiver);

        if results.is_empty() {
            return Vec::new();
        }

        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let mut invalidations = BTreeSet::new();
        for result in results {
            let entry = state
                .entries
                .entry(result.url)
                .or_insert_with(|| HttpCacheEntry {
                    body: None,
                    error: None,
                    fetched_at: None,
                    fetched_at_unix_ms: None,
                    refreshing: false,
                    subscribers: BTreeSet::new(),
                });
            entry.refreshing = false;
            match result.result {
                Ok((body, fetched_at_unix_ms)) => {
                    entry.body = Some(body);
                    entry.error = None;
                    entry.fetched_at = Some(Instant::now());
                    entry.fetched_at_unix_ms = Some(fetched_at_unix_ms);
                }
                Err(error) => {
                    entry.error = Some(error);
                    if entry.body.is_none() {
                        entry.fetched_at = Some(Instant::now());
                        entry.fetched_at_unix_ms = Some(current_unix_ms());
                    }
                }
            }
            invalidations.extend(entry.subscribers.iter().cloned());
        }
        invalidations.into_iter().collect()
    }

    fn has_finished_refreshes(&self) -> bool {
        self.finished_refresh_pending.load(Ordering::Acquire)
    }

    fn memory_snapshot_for_plugin(&self, plugin_id: &str) -> PluginHttpCacheMemory {
        let Ok(state) = self.state.lock() else {
            return PluginHttpCacheMemory::default();
        };
        let mut memory = PluginHttpCacheMemory::default();
        for (url, entry) in &state.entries {
            if !entry
                .subscribers
                .iter()
                .any(|target| target.plugin_id() == plugin_id)
            {
                continue;
            }
            memory.entries = memory.entries.saturating_add(1);
            memory.body_bytes = memory
                .body_bytes
                .saturating_add(url.capacity())
                .saturating_add(entry.body.as_ref().map_or(0, String::capacity));
            memory.error_bytes = memory
                .error_bytes
                .saturating_add(entry.error.as_ref().map_or(0, String::capacity));
        }
        memory
    }
}

impl PluginResourceCache {
    fn seed(&self, plugin_id: &str, values: &BTreeMap<String, Arc<[u8]>>) {
        if values.is_empty() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for (path, bytes) in values {
            state.entries.insert(
                ResourceCacheKey {
                    plugin_id: plugin_id.to_string(),
                    path: path.clone(),
                },
                ResourceCacheEntry {
                    bytes: Some(Arc::clone(bytes)),
                    error: None,
                    loading: false,
                    subscribers: BTreeSet::new(),
                },
            );
        }
    }

    /// 丢弃某个插件的全部资源缓存条目。
    ///
    /// 插件目录内容变化后必须重新读取资源，否则插件会一直使用改动前的字节。
    fn drop_plugin(&self, plugin_id: &str) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state
            .entries
            .retain(|key, _entry| key.plugin_id.as_str() != plugin_id);
    }

    /// 只保留仍然安装的插件的资源缓存条目。
    fn retain_plugins(&self, installed: &BTreeSet<String>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state
            .entries
            .retain(|key, _entry| installed.contains(&key.plugin_id));
    }

    pub(crate) fn read(
        &self,
        manifest: &PluginManifest,
        plugin_id: &str,
        render_context: Option<&RenderContext>,
        path: &str,
    ) -> std::result::Result<Arc<[u8]>, abi::PluginError> {
        manifest
            .require_capability(PluginCapability::ResourceRead)
            .map_err(|error| abi::PluginError {
                code: "capability-denied".to_string(),
                message: error.to_string(),
            })?;
        let resource_path = manifest
            .resource_path(path)
            .map_err(|error| abi::PluginError {
                code: "resource-denied".to_string(),
                message: error.to_string(),
            })?;
        let key = ResourceCacheKey {
            plugin_id: plugin_id.to_string(),
            path: path.to_string(),
        };
        let subscriber = match render_context {
            Some(RenderContext::Page { page_id }) => PluginInvalidationTarget::Page {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.clone(),
            },
            Some(RenderContext::Injection { slot, page }) => PluginInvalidationTarget::Injection {
                plugin_id: plugin_id.to_string(),
                slot: *slot,
                page: page.clone(),
            },
            None => PluginInvalidationTarget::Plugin {
                plugin_id: plugin_id.to_string(),
            },
        };

        let mut state = self.state.lock().map_err(|_| abi::PluginError {
            code: "resource-cache-lock-failed".to_string(),
            message: "plugin resource cache lock failed".to_string(),
        })?;
        let sender = state.sender.clone();
        let entry = state
            .entries
            .entry(key.clone())
            .or_insert_with(|| ResourceCacheEntry {
                bytes: None,
                error: None,
                loading: false,
                subscribers: BTreeSet::new(),
            });
        entry.subscribers.insert(subscriber);

        if let Some(bytes) = entry.bytes.as_ref() {
            return Ok(Arc::clone(bytes));
        }
        if let Some(error) = entry.error.as_ref() {
            return Err(abi::PluginError {
                code: "resource-read-failed".to_string(),
                message: error.clone(),
            });
        }
        if !entry.loading {
            entry.loading = true;
            spawn_resource_refresh(
                key,
                manifest.root_dir.clone(),
                resource_path,
                usize::try_from(manifest.limits.max_resource_bytes).unwrap_or(usize::MAX),
                sender,
                Arc::clone(&self.finished_refresh_pending),
            );
        }

        Err(abi::PluginError {
            code: "resource-loading".to_string(),
            message: format!("plugin resource {path} is loading in the background"),
        })
    }

    fn drain_finished(&self) -> Vec<PluginInvalidationTarget> {
        if !self.finished_refresh_pending.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }

        let receiver = {
            let Ok(state) = self.state.lock() else {
                self.finished_refresh_pending.store(true, Ordering::Release);
                return Vec::new();
            };
            Arc::clone(&state.receiver)
        };

        let mut results = Vec::new();
        let Ok(receiver) = receiver.lock() else {
            self.finished_refresh_pending.store(true, Ordering::Release);
            return Vec::new();
        };
        while let Ok(result) = receiver.try_recv() {
            results.push(result);
        }
        drop(receiver);

        if results.is_empty() {
            return Vec::new();
        }

        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        let mut invalidations = BTreeSet::new();
        for result in results {
            let entry = state
                .entries
                .entry(result.key)
                .or_insert_with(|| ResourceCacheEntry {
                    bytes: None,
                    error: None,
                    loading: false,
                    subscribers: BTreeSet::new(),
                });
            entry.loading = false;
            match result.result {
                Ok(bytes) => {
                    entry.bytes = Some(Arc::<[u8]>::from(bytes));
                    entry.error = None;
                }
                Err(error) => {
                    entry.error = Some(error);
                }
            }
            invalidations.extend(entry.subscribers.iter().cloned());
        }
        invalidations.into_iter().collect()
    }

    fn has_finished_refreshes(&self) -> bool {
        self.finished_refresh_pending.load(Ordering::Acquire)
    }

    fn memory_snapshot_for_plugin(&self, plugin_id: &str) -> PluginResourceCacheMemory {
        let Ok(state) = self.state.lock() else {
            return PluginResourceCacheMemory::default();
        };
        let mut memory = PluginResourceCacheMemory::default();
        for (key, entry) in &state.entries {
            if key.plugin_id != plugin_id {
                continue;
            }
            memory.entries = memory.entries.saturating_add(1);
            memory.bytes = memory
                .bytes
                .saturating_add(key.path.capacity())
                .saturating_add(entry.bytes.as_ref().map_or(0, |bytes| bytes.len()));
            memory.error_bytes = memory
                .error_bytes
                .saturating_add(entry.error.as_ref().map_or(0, String::capacity));
        }
        memory
    }
}

fn plugin_log_memory(slices: Option<(&[PluginLogEntry], &[PluginLogEntry])>) -> PluginLogMemory {
    let Some((front, back)) = slices else {
        return PluginLogMemory::default();
    };
    let mut memory = PluginLogMemory {
        entries: front.len().saturating_add(back.len()),
        ..PluginLogMemory::default()
    };
    for entry in front.iter().chain(back.iter()) {
        memory.bytes = memory
            .bytes
            .saturating_add(std::mem::size_of::<PluginLogEntry>())
            .saturating_add(entry.message.len());
    }
    memory
}

fn spawn_resource_refresh(
    key: ResourceCacheKey,
    root_dir: PathBuf,
    resource_path: PathBuf,
    max_bytes: usize,
    sender: mpsc::Sender<ResourceLoadResult>,
    finished_refresh_pending: Arc<AtomicBool>,
) {
    let fallback_key = key.clone();
    let fallback_sender = sender.clone();
    let fallback_finished_refresh_pending = Arc::clone(&finished_refresh_pending);
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        let loaded = crate::tasks::runtime::blocking::run_io(move || {
            read_plugin_resource_path(&root_dir, &resource_path, max_bytes)
        })
        .await;
        let result = match loaded {
            Ok(result) => result,
            Err(error) => Err(error),
        };
        if let Err(error) = sender.send(ResourceLoadResult { key, result }) {
            warn!(error = ?error, "plugin resource refresh result receiver dropped");
            return;
        }
        finished_refresh_pending.store(true, Ordering::Release);
        notify_async_app_refresh_finished();
    }) {
        if let Err(send_error) = fallback_sender.send(ResourceLoadResult {
            key: fallback_key,
            result: Err(format!("schedule resource refresh failed: {error}")),
        }) {
            warn!(error = ?send_error, "plugin resource refresh result receiver dropped");
            return;
        }
        fallback_finished_refresh_pending.store(true, Ordering::Release);
        notify_async_app_refresh_finished();
    }
}

fn read_plugin_resource_path(
    root: &Path,
    path: &Path,
    max_bytes: usize,
) -> std::result::Result<Vec<u8>, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("plugin resource is outside its package".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path).map_err(|error| error.to_string())?
        .take(max_bytes as u64 + 1).read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > max_bytes {
        return Err(format!("plugin resource exceeds {max_bytes} bytes"));
    }
    Ok(bytes)
}

fn spawn_http_refresh(
    url: String,
    max_bytes: usize,
    sender: mpsc::Sender<HttpRefreshResult>,
    finished_refresh_pending: Arc<AtomicBool>,
) {
    let fallback_url = url.clone();
    let fallback_sender = sender.clone();
    let fallback_finished_refresh_pending = Arc::clone(&finished_refresh_pending);
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        let result = fetch_http_text(&url, max_bytes, None).await;
        if let Err(error) = sender.send(HttpRefreshResult { url, result }) {
            warn!(error = ?error, "plugin HTTP refresh result receiver dropped");
            return;
        }
        finished_refresh_pending.store(true, Ordering::Release);
        notify_async_app_refresh_finished();
    }) {
        if let Err(send_error) = fallback_sender.send(HttpRefreshResult {
            url: fallback_url,
            result: Err(format!("schedule HTTP refresh failed: {error}")),
        }) {
            warn!(error = ?send_error, "plugin HTTP refresh result receiver dropped");
            return;
        }
        fallback_finished_refresh_pending.store(true, Ordering::Release);
        notify_async_app_refresh_finished();
    }
}

pub(super) async fn fetch_http_text(
    url: &str,
    max_bytes: usize,
    redirects: Option<&PluginManifest>,
) -> std::result::Result<(String, u64), String> {
    let client = crate::http::proxy::clients::client()
        .map_err(|error| format!("build HTTP client failed: {error}"))?;
    let mut url = reqwest::Url::parse(url).map_err(|error| error.to_string())?;
    let options = crate::http::request::RequestOptions {
        method: "GET",
        headers: None,
        timeout_ms: Some(HTTP_TIMEOUT.as_millis() as u64),
        allow_redirects: Some(false),
    };
    let mut response = crate::http::request::send(&client, &url, &options).await?;
    if let Some(manifest) = redirects {
        for _ in 0..4 {
            if !response.status().is_redirection() {
                break;
            }
            let location = response.headers().get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "HTTP redirect is missing a location".to_owned())?;
            url = url.join(location).map_err(|error| error.to_string())?;
            if !manifest.allows_network_url(url.as_str()) {
                return Err("HTTP redirect is outside the plugin allowlist".into());
            }
            response = crate::http::request::send(&client, &url, &options).await?;
        }
    }
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP status {status}"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            return Err(format!("HTTP response exceeds {max_bytes} bytes"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes)
        .map_err(|error| format!("HTTP body is not utf-8: {error}"))?;
    Ok((text, current_unix_ms()))
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

enum BootstrapRegistration {
    Page {
        page_id: String,
        title: String,
        navigation: Option<PluginNavigationEntry>,
    },
    Injection {
        slot: InjectionSlot,
        page: Option<String>,
        priority: i32,
        layout: Option<InjectionLayout>,
    },
    Subscription {
        event: String,
    },
    Service {
        service_name: String,
    },
}

fn bootstrap_registration_from_abi(
    plugin_id: &str,
    registration: abi::Registration,
) -> Result<BootstrapRegistration> {
    match registration {
        abi::Registration::Page(page) => Ok(BootstrapRegistration::Page {
            page_id: page.page_id,
            title: page.title,
            navigation: page.tool.map(|navigation| PluginNavigationEntry {
                label: navigation.label,
                icon: navigation.icon,
                order: navigation.order,
            }),
        }),
        abi::Registration::Injection(injection) => Ok(BootstrapRegistration::Injection {
            slot: injection_slot_from_abi(injection.slot),
            page: injection.page,
            priority: injection.priority,
            layout: injection.layout.map(injection_layout_from_abi),
        }),
        abi::Registration::Service(service) => {
            if service.service_name.trim().is_empty() {
                bail!("plugin {plugin_id} registered an empty service name");
            }
            Ok(BootstrapRegistration::Service {
                service_name: service.service_name,
            })
        }
        abi::Registration::Subscription(subscription) => {
            if subscription.event.trim().is_empty() {
                bail!("plugin {plugin_id} registered an empty event subscription");
            }
            Ok(BootstrapRegistration::Subscription {
                event: subscription.event,
            })
        }
    }
}

fn plugin_error_to_anyhow(error: abi::PluginError) -> anyhow::Error {
    anyhow!("plugin error {}: {}", error.code, error.message)
}

fn injection_slot_from_abi(slot: abi::InjectionSlot) -> InjectionSlot {
    match slot {
        abi::InjectionSlot::MainRootOverlay => InjectionSlot::MainRootOverlay,
        abi::InjectionSlot::PageHeader => InjectionSlot::PageHeader,
        abi::InjectionSlot::PageBody => InjectionSlot::PageBody,
        abi::InjectionSlot::HomeSidebar => InjectionSlot::HomeSidebar,
    }
}

fn injection_slot_to_abi(slot: InjectionSlot) -> abi::InjectionSlot {
    match slot {
        InjectionSlot::MainRootOverlay => abi::InjectionSlot::MainRootOverlay,
        InjectionSlot::PageHeader => abi::InjectionSlot::PageHeader,
        InjectionSlot::PageBody => abi::InjectionSlot::PageBody,
        InjectionSlot::HomeSidebar => abi::InjectionSlot::HomeSidebar,
    }
}

fn injection_layout_from_abi(layout: abi::InjectionLayout) -> InjectionLayout {
    InjectionLayout {
        preferred_width: layout.preferred_width,
        min_width: layout.min_width,
        max_width: layout.max_width,
        max_height: layout.max_height,
        priority: layout.priority,
        compact_behavior: match layout.compact_behavior {
            abi::CompactBehavior::None => CompactBehavior::None,
            abi::CompactBehavior::Scroll => CompactBehavior::Scroll,
        },
    }
}

fn plugin_event_to_abi(event: &PluginEvent) -> abi::PluginEvent {
    abi::PluginEvent {
        plugin_id: event.plugin_id.clone(),
        page_id: event.page_id.clone(),
        kind: match &event.kind {
            PluginEventKind::JobReady { job_id } => abi::PluginEventKind::JobReady(abi::JobReadyEvent {
            job_id: job_id.clone(),
        }),
        PluginEventKind::RouteChanged { path } => {
                abi::PluginEventKind::RouteChanged(abi::RouteChangedEvent { path: path.clone() })
            }
            PluginEventKind::Action { action_id, value } => {
                abi::PluginEventKind::Action(abi::ActionEvent {
                    action_id: action_id.clone(),
                    value: value.clone(),
                })
            }
            PluginEventKind::Global { name, payload } => {
                abi::PluginEventKind::Global(abi::GlobalEvent {
                    name: name.clone(),
                    payload: payload.clone(),
                })
            }
        },
    }
}

fn view_tree_from_abi(tree: ViewTree) -> Result<ViewTree> {
    tree.validate()?;
    Ok(tree)
}

fn uninstall_plugin_files(
    plugins_dir: &Path,
    manifest: &PluginManifest,
    plugin_id: &str,
) -> Result<()> {
    let root_dir = std::fs::canonicalize(&manifest.root_dir)
        .with_context(|| format!("canonicalize plugin {}", manifest.root_dir.display()))?;
    let plugins_dir_canonical = std::fs::canonicalize(plugins_dir)
        .with_context(|| format!("canonicalize plugins dir {}", plugins_dir.display()))?;
    if !root_dir.starts_with(&plugins_dir_canonical) || root_dir == plugins_dir_canonical {
        bail!("refusing to uninstall plugin outside plugin directory");
    }
    fs::remove_dir_all(&root_dir)
        .with_context(|| format!("remove plugin directory {}", root_dir.display()))?;
    if let Err(error) = super::native::remove_components(plugin_id) {
        tracing::warn!(%plugin_id, %error, "mark native components for cleanup");
    }
    // Task types registered by this plugin must not outlive it.
    crate::tasks::registry::unregister_plugin_namespace(plugin_id);
    crate::plugins::state::remove(plugins_dir, plugin_id)
}

fn schedule_installed_package_rollback(plugin_id: String, manifest: PluginManifest) {
    let plugin_id_for_log = plugin_id.clone();
    let scheduled = crate::tasks::runtime::spawn_io(async move {
        let rollback = crate::tasks::runtime::blocking::run_io(move || {
            crate::plugins::manifest::rollback_installed_package(&manifest)
        })
        .await;

        match rollback {
            Ok(Ok(())) => {
                tracing::debug!(plugin_id, "plugin package rollback finished");
            }
            Ok(Err(error)) => {
                warn!(
                    plugin_id,
                    error = %crate::plugins::manifest::format_error_chain(&error),
                    "failed to roll back installed plugin package"
                );
            }
            Err(error) => {
                warn!(
                    plugin_id,
                    error = %error,
                    "plugin package rollback blocking task failed"
                );
            }
        }
    });
    if let Err(error) = scheduled {
        warn!(
            plugin_id = plugin_id_for_log,
            error = %error,
            "failed to schedule installed plugin package rollback"
        );
    }
}

fn prepare_plugin_reload_from_sources(
    plugins_dir: &Path,
    package_cache_dir: &Path,
    cache_dir: &Path,
    prepare_cache: &PluginPrepareCache,
) -> Result<PreparedPluginReload> {
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("create wasm cache {}", cache_dir.display()))?;
    let manifests =
        crate::plugins::manifest::load_manifests_from_sources(plugins_dir, package_cache_dir)?;
    Ok(prepare_plugin_manifests(
        manifests,
        plugins_dir,
        package_cache_dir,
        prepare_cache,
    ))
}

/// 读取全部插件的清单、翻译、wasm 与资源，并按目录指纹复用未变化插件的准备结果。
fn prepare_plugin_manifests(
    manifests: Vec<PluginManifest>,
    plugins_dir: &Path,
    package_cache_dir: &Path,
    prepare_cache: &PluginPrepareCache,
) -> PreparedPluginReload {
    let disabled_plugins = crate::plugins::state::disabled(plugins_dir);
    let mut plugins = Vec::with_capacity(manifests.len());
    let mut installed_dirs = BTreeSet::new();

    for manifest in manifests {
        let enabled = !disabled_plugins.contains(&manifest.id);
        if enabled && let Err(error) = crate::plugins::manifest::commit_installed_package(&manifest)
        {
            warn!(
                plugin_id = manifest.id,
                error = %crate::plugins::manifest::format_error_chain(&error),
                "failed to finalize installed plugin package"
            );
        }

        // 目录指纹必须在所有写入插件目录的步骤之后采集，否则记录下来的指纹会立刻失效。
        let root_dir = manifest.root_dir.clone();
        let stamp = PluginSourceStamp::read(&root_dir);
        installed_dirs.insert(root_dir.clone());
        if let Some(prepared) = prepare_cache.lookup(&root_dir, &manifest, &stamp) {
            plugins.push(PreparedPluginManifest {
                manifest,
                enabled,
                translations: prepared.translations.clone(),
                wasm: prepared.wasm.clone(),
                resources: prepared.resources.clone(),
            });
            continue;
        }

        let translations = load_plugin_translations(&manifest);
        let wasm = prepare_plugin_wasm(&manifest);
        let resources = prepare_plugin_resources(&manifest, package_cache_dir);
        let prepared = PreparedPluginManifest {
            manifest,
            enabled,
            translations,
            wasm,
            resources,
        };
        prepare_cache.store(&root_dir, stamp, Arc::new(prepared.clone()));
        plugins.push(prepared);
    }
    prepare_cache.retain_dirs(&installed_dirs);

    // 根据 DependencyGraph 拓扑排序插件加载顺序
    let mut graph = crate::plugins::services::DependencyGraph::new();
    for plugin in &plugins {
        let deps: BTreeSet<String> = plugin.manifest.dependencies.keys().cloned().collect();
        graph.add_plugin(plugin.manifest.id.clone(), deps);
    }
    match graph.compute_load_order() {
        Ok(order) => {
            let mut ordered_plugins = Vec::with_capacity(plugins.len());
            let mut plugin_map: BTreeMap<String, PreparedPluginManifest> = plugins
                .into_iter()
                .map(|p| (p.manifest.id.clone(), p))
                .collect();
            for id in order {
                if let Some(p) = plugin_map.remove(&id) {
                    ordered_plugins.push(p);
                }
            }
            for (_, p) in plugin_map {
                ordered_plugins.push(p);
            }
            plugins = ordered_plugins;
        }
        Err(err) => {
            warn!(error = %err, "plugin dependency resolution failed, preserving default order");
        }
    }

    PreparedPluginReload { plugins }
}

fn prepare_plugin_resources(
    manifest: &PluginManifest,
    package_cache_dir: &Path,
) -> PreparedPluginResources {
    let config_text = crate::plugins::manifest::read_user_config(manifest)
        .map_err(|error| Arc::<str>::from(crate::plugins::manifest::format_error_chain(&error)));
    let storage_dir = package_cache_dir
        .join("storage")
        .join(sanitize_storage_segment(&manifest.id));
    let storage_values =
        load_storage_snapshot(&storage_dir).map_err(|error| Arc::<str>::from(error.message));
    let resource_values = prepare_plugin_resource_values(manifest);
    let source_stamp = PluginSourceStamp::read(&manifest.root_dir);

    PreparedPluginResources {
        has_readme: manifest_has_any_readme(manifest),
        has_config: manifest
            .config_schema_path()
            .is_some_and(|path| path.exists()),
        icon_path: manifest.icon_path().filter(|path| path.exists()),
        config_text,
        storage_values,
        resource_values,
        game_mods: super::game_mods::prepare(manifest),
        source_stamp,
    }
}

fn prepare_plugin_resource_values(manifest: &PluginManifest) -> BTreeMap<String, Arc<[u8]>> {
    if !manifest.has_capability(&PluginCapability::ResourceRead) {
        return BTreeMap::new();
    }

    let max_bytes = usize::try_from(manifest.limits.max_resource_bytes).unwrap_or(usize::MAX);
    let mut values = BTreeMap::new();
    for allowed in &manifest.permissions.resource_allow {
        if allowed.ends_with('/') {
            continue;
        }
        let normalized = allowed.trim_end_matches('/');
        let Ok(path) = manifest.resource_path(normalized) else {
            continue;
        };
        if let Ok(bytes) = read_plugin_resource_path(&manifest.root_dir, &path, max_bytes) {
            values.insert(normalized.to_string(), Arc::<[u8]>::from(bytes));
        }
    }
    values
}

fn prepare_plugin_wasm(manifest: &PluginManifest) -> PreparedPluginWasm {
    let path = manifest.wasm_path();
    let bytes = match read_plugin_resource_path(&manifest.root_dir, &path, 32 * 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(error) => {
            return PreparedPluginWasm::Error(Arc::<str>::from(format!(
                "read plugin wasm {} failed: {error}",
                path.display()
            )));
        }
    };

    // 组件校验与编译延迟到实例化时（wasmtime 引擎 + Cranelift 磁盘缓存）；
    // 这里只负责内容读取与摘要，供增量 reload 复用。
    let sha256 = crate::plugins::manifest::sha256_hex(&bytes);
    PreparedPluginWasm::Ready {
        wasm: Arc::from(bytes.as_slice()),
        sha256,
    }
}

pub fn init(cx: &mut App) {
    cx.default_global::<PluginRegistry>();
    spawn_initial_reload(cx);
    start_watcher(cx);
}

/// Initializes the plugin registry without starting filesystem or WASM work.
///
/// Main-window startup uses this path so the first visible frame owns the cold-start CPU and IO
/// budget. Calls that try to render plugin UI before the gate opens see the pending flag and do not
/// start a competing reload.
pub fn init_deferred(cx: &mut App) {
    cx.default_global::<PluginRegistry>();
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        if registry.loaded_once || registry.initial_reload_pending {
            return;
        }
        registry.initial_reload_pending = true;
        registry.initial_reload_deferred = true;
    });
}

/// Starts the initial plugin reload reserved by init_deferred.
pub fn start_deferred_initial_reload(cx: &mut App) {
    let should_start = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        if !registry.initial_reload_deferred {
            return false;
        }
        registry.initial_reload_deferred = false;
        true
    });
    if should_start {
        spawn_initial_reload_task(cx);
    }
}

/// Schedules the initial plugin scan immediately.
fn spawn_initial_reload(cx: &mut App) {
    let should_start = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        if registry.loaded_once || registry.initial_reload_pending {
            return false;
        }
        registry.initial_reload_pending = true;
        registry.initial_reload_deferred = false;
        true
    });
    if !should_start {
        return;
    }
    spawn_initial_reload_task(cx);
}

/// Reads manifests/resources/WASM on the blocking pool and publishes the prepared registry.
fn spawn_initial_reload_task(cx: &mut App) {
    let (plugins_dir, package_cache_dir, cache_dir, prepare_cache) = {
        let registry = cx.global::<PluginRegistry>();
        (
            registry.plugins_dir().to_path_buf(),
            registry.package_cache_dir().to_path_buf(),
            registry.cache_dir().to_path_buf(),
            registry.prepare_cache.clone(),
        )
    };

    cx.spawn(async move |cx| {
        let prepared = crate::tasks::runtime::blocking::run_io(move || {
            prepare_plugin_reload_from_sources(
                &plugins_dir,
                &package_cache_dir,
                &cache_dir,
                &prepare_cache,
            )
        })
        .await;

        cx.update(|cx| {
            let reload_requested = cx.update_global(|registry: &mut PluginRegistry, _cx| {
                registry.initial_reload_pending = false;
                registry.initial_reload_deferred = false;
                if !registry.loaded_once() {
                    let result = match prepared {
                        Ok(Ok(prepared)) => registry.reload_prepared_manifests(prepared),
                        Ok(Err(error)) => Err(error),
                        Err(error) => Err(anyhow!("{error}")),
                    };
                    if let Err(error) = result {
                        let error_message = crate::plugins::manifest::format_error_chain(&error);
                        error!(error = %error_message, "plugin initial load failed");
                        registry.last_error = Some(SharedString::from(error_message));
                    }
                }
                std::mem::take(&mut registry.reload_prepare_requested)
            });
            start_pending_activations(cx);
            cx.refresh_windows();
            if reload_requested {
                reload_all(cx);
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub fn reload_all(cx: &mut App) {
    let should_start = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        if registry.initial_reload_pending || registry.reload_prepare_pending {
            registry.reload_prepare_requested = true;
            return false;
        }
        registry.reload_prepare_pending = true;
        true
    });
    if !should_start {
        return;
    }

    let (plugins_dir, package_cache_dir, cache_dir, prepare_cache) = {
        let registry = cx.global::<PluginRegistry>();
        (
            registry.plugins_dir().to_path_buf(),
            registry.package_cache_dir().to_path_buf(),
            registry.cache_dir().to_path_buf(),
            registry.prepare_cache.clone(),
        )
    };

    cx.spawn(async move |cx| {
        let prepared = crate::tasks::runtime::blocking::run_io(move || {
            prepare_plugin_reload_from_sources(
                &plugins_dir,
                &package_cache_dir,
                &cache_dir,
                &prepare_cache,
            )
        })
        .await;

        cx.update(|cx| {
            let reload_requested = cx.update_global(|registry: &mut PluginRegistry, _cx| {
                registry.reload_prepare_pending = false;
                let result = match prepared {
                    Ok(Ok(prepared)) => registry.reload_prepared_manifests(prepared),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(anyhow!("{error}")),
                };
                if let Err(error) = result {
                    let error_message = crate::plugins::manifest::format_error_chain(&error);
                    error!(error = %error_message, "plugin reload failed");
                    registry.last_error = Some(SharedString::from(error_message));
                } else {
                    }
                std::mem::take(&mut registry.reload_prepare_requested)
            });
            start_pending_activations(cx);
            cx.refresh_windows();
            if reload_requested {
                reload_all(cx);
            }
        })?;

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

/// Compiles and initializes pending components outside GPUI. Results from a superseded
/// package generation cannot re-register its pages or services.
fn start_pending_activations(cx: &mut App) {
    let next = cx.update_global(|registry: &mut PluginRegistry, _| {
        if !registry.activating.is_empty() {
            return None;
        }
        let mut graph = DependencyGraph::new();
        for instance in registry.plugins.values().filter(|instance| instance.enabled) {
            graph.add_plugin(instance.manifest.id.clone(), instance.manifest.dependencies.keys().cloned().collect());
        }
        let order = graph.compute_load_order().unwrap_or_else(|_| registry.plugins.keys().cloned().collect());
        let instance = order.iter().filter_map(|id| registry.plugins.get(id)).find(|instance| {
            instance.enabled && instance.runtime.is_none()
                && matches!(instance.state, PluginLoadState::Unloaded)
        })?.clone();
        registry.activating.insert((instance.manifest.id.clone(), instance.generation));
        Some((registry.activation_context(), instance))
    });
    let Some((activation, instance)) = next else { return };
    let plugin_id = instance.manifest.id.clone();
    let generation = instance.generation;
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::blocking::run_io(move || activation.activate(instance))
            .await.map_err(|error| anyhow!("{error}")).and_then(|result| result);
        let _ = cx.update(|cx| {
            let theme = current_theme_snapshot(cx);
            let retry_later = cx.update_global(|registry: &mut PluginRegistry, _| {
                registry.activating.remove(&(plugin_id.clone(), generation));
                if !registry.plugins.get(&plugin_id).is_some_and(|instance| instance.enabled && instance.generation == generation) {
                    return false;
                }
                let retry_later = result.as_ref().err().is_some_and(|error| format!("{error:#}").contains("resource-loading"));
                if let Ok(instance) = &result
                    && let Some(runtime) = &instance.runtime
                {
                    runtime.lock().unwrap_or_else(|error| error.into_inner()).set_theme(theme);
                }
                let _ = registry.finish_activation(&plugin_id, result);
                retry_later
            });
            if !retry_later {
                start_pending_activations(cx);
            }
            let early = cx.update_global(|registry: &mut PluginRegistry, _| std::mem::take(&mut registry.early_job_events));
            for (id, generation, job_id) in early {
                dispatch_job_ready(cx, id, generation, job_id);
            }
            cx.refresh_windows();
        });
    }).detach();
}

pub fn ensure_manifest_index(cx: &mut App) {
    let needs_sync_reload = {
        let registry = cx.global::<PluginRegistry>();
        !registry.loaded_once() && !registry.initial_reload_pending
    };
    if needs_sync_reload {
        reload_all(cx);
    }
}

pub fn ensure_loaded(cx: &mut App) {
    ensure_manifest_index(cx);
}

pub fn start_watcher(cx: &mut App) {
    if cx.global::<PluginRegistry>().reload_tx.is_some() {
        return;
    }

    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();
    match crate::plugins::watcher::spawn(plugins_dir, cx) {
        Ok((sender, task)) => {
            set_async_app_refresh_sender(sender.clone());
            cx.update_global(|registry: &mut PluginRegistry, _cx| {
                registry.set_watcher(sender, task);
            });
            if drain_async_app_refreshes(cx) {
                cx.refresh_windows();
            }
        }
        Err(error) => {
            let error_message = crate::plugins::manifest::format_error_chain(&error);
            warn!(error = %error_message, "failed to start plugin watcher");
            cx.update_global(|registry: &mut PluginRegistry, _cx| {
                registry.last_error = Some(SharedString::from(error_message));
            });
        }
    }
}

fn set_async_app_refresh_sender(sender: crate::plugins::watcher::Sender) {
    let slot = ASYNC_APP_REFRESH_NOTIFICATION.get_or_init(|| Mutex::new(None));
    if let Ok(mut current) = slot.lock() {
        *current = Some(sender);
    }
}

fn notify_async_app_refresh_finished() {
    let Some(slot) = ASYNC_APP_REFRESH_NOTIFICATION.get() else {
        return;
    };
    let Ok(current) = slot.lock() else {
        return;
    };
    let Some(sender) = current.as_ref() else {
        return;
    };
    if let Err(error) = sender.unbounded_send(crate::plugins::watcher::Message::AsyncAppRefresh) {
        warn!(error = ?error, "plugin HTTP refresh notification receiver dropped");
    }
}

/// Schedules one background render for a cache miss.
pub fn request_page_render(cx: &mut App, plugin_id: &str, page_id: &str) {
    // 已完成的后台刷新在这里落地：正常情况下只做一次原子检查，不会改写注册表。
    drain_async_app_refreshes(cx);

    let hibernated = {
        let registry = cx.global::<PluginRegistry>();
        registry.plugins.get(plugin_id).is_some_and(|plugin| {
            plugin.enabled && plugin.runtime.is_none() && matches!(plugin.state, PluginLoadState::Unloaded)
                && registry.activating.is_empty()
        })
    };
    if hibernated {
        cx.defer(start_pending_activations);
        return;
    }

    let needs_render = {
        let registry = cx.global::<PluginRegistry>();
        registry.plugins.get(plugin_id).is_some_and(|plugin| plugin.enabled && plugin.runtime.is_some())
            && registry.cached_page(plugin_id, page_id).is_none()
            && registry.cached_page_error(plugin_id, page_id).is_none()
            && !registry.is_page_pending(plugin_id, page_id)
    };
    if !needs_render {
        return;
    }

    let plugin_id = plugin_id.to_string();
    let page_id = page_id.to_string();
    cx.defer(move |cx| request_page_render_now(cx, &plugin_id, &page_id));
}

/// 在非渲染上下文执行页面渲染：加载插件、执行 Wasm，并把结果写入渲染缓存。
fn request_page_render_now(cx: &mut App, plugin_id: &str, page_id: &str) {
    ensure_loaded(cx);
    let key = PageRenderCacheKey { plugin_id: plugin_id.to_string(), page_id: page_id.to_string() };
    let prepared = cx.update_global(|registry: &mut PluginRegistry, _| {
        let instance = registry.plugins.get(plugin_id)?;
        let runtime = instance.runtime.clone()?;
        let cache = &mut registry.render_cache;
        if cache.pages.contains_key(&key) || cache.page_errors.contains_key(&key)
            || cache.pending_pages.contains_key(&key) || cache.pending_pages.len() >= 32 {
            return None;
        }
        let ticket = cache.next_ticket();
        cache.pending_pages.insert(key.clone(), ticket);
        Some((runtime, ticket))
    });
    let Some((runtime, ticket)) = prepared else { return };
    let theme = current_theme_snapshot(cx);
    let request = abi::PageRenderRequest { plugin_id: key.plugin_id.clone(), page_id: key.page_id.clone() };
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::blocking::run_io(move || {
            let mut runtime = runtime.lock().unwrap_or_else(|error| error.into_inner());
            runtime.set_theme(theme);
            let rendered = runtime.render_page(request)
                .map_err(plugin_error_to_anyhow)
                .and_then(view_tree_from_abi);
            (rendered, runtime.take_effects(), runtime.is_poisoned())
        }).await;
        let _ = cx.update(|cx| {
            cx.update_global(|registry: &mut PluginRegistry, _| {
                if registry.render_cache.pending_pages.get(&key) != Some(&ticket) {
                    return;
                }
                registry.render_cache.pending_pages.remove(&key);
                let (rendered, effects, poisoned) = match result {
                    Ok(output) => output,
                    Err(error) => (Err(anyhow!("{error}")), Vec::new(), false),
                };
                registry.record_passive_effects(effects);
                match rendered {
                    Ok(tree) => { registry.render_cache.insert_page(key, Arc::new(tree)); }
                    Err(error) => {
                        let message = format!("{error:#}");
                        registry.record_page_error(&key, &message);
                        if poisoned {
                            registry.mark_unhealthy(&key.plugin_id, message);
                        }
                    }
                }
            });
            cx.refresh_windows();
        });
    }).detach();
}

/// 渲染路径读取插件注入：只投影已缓存的树，缓存缺失时登记一次延迟渲染。
///
/// 该函数可以在 `Render` 中调用：命中缓存时只做一次缓存查找，缺失时只在帧结束后执行
/// 插件代码，因此多个插件不会把主线程卡在渲染调用栈里。渲染结果（包括空结果）会写入
/// 渲染缓存，所以同一 slot 不会每帧重复登记。
pub fn render_injections(
    cx: &mut App,
    slot: InjectionSlot,
    page: Option<&str>,
) -> Vec<RenderedInjection> {
    // 已完成的后台刷新在这里落地：正常情况下只做一次原子检查，不会改写注册表。
    drain_async_app_refreshes(cx);

    let should_request = {
        let registry = cx.global::<PluginRegistry>();
        match registry.cached_injections(slot, page) {
            Some(trees) => return trees,
            None => registry.injections_need_render(slot, page) && !registry.render_cache.pending_injections.keys()
                .any(|key| key.slot == slot && key.page.as_deref() == page),
        }
    };
    if !should_request {
        return Vec::new();
    }

    let page_id = page.map(str::to_string);
    cx.defer(move |cx| {
        render_injections_now(cx, slot, page_id.as_deref());
    });
    Vec::new()
}

/// 在非渲染上下文执行注入渲染：加载插件、执行 Wasm，并把结果写入渲染缓存。
fn render_injections_now(cx: &mut App, slot: InjectionSlot, page: Option<&str>) {
    ensure_loaded(cx);
    let key = InjectionRenderCacheKey { slot, page: page.map(str::to_owned) };
    let prepared = cx.update_global(|registry: &mut PluginRegistry, _| {
        let cache = &mut registry.render_cache;
        if cache.injections.contains_key(&key) || cache.pending_injections.contains_key(&key)
            || cache.pending_injections.len() >= 32 {
            return None;
        }
        let work: Vec<_> = registry.injections.iter().filter(|item| {
            item.slot == slot && item.page.as_deref().is_none_or(|id| Some(id) == page)
        }).filter_map(|item| {
            let instance = registry.plugins.get(&item.plugin_id)?;
            Some((item.clone(), instance.runtime.clone()?))
        }).collect();
        if work.is_empty() {
            return None;
        }
        let ticket = cache.next_ticket();
        cache.pending_injections.insert(key.clone(), ticket);
        Some((work, ticket))
    });
    let Some((work, ticket)) = prepared else { return };
    let theme = current_theme_snapshot(cx);
    let request = abi::InjectionRequest { slot: injection_slot_to_abi(slot), page: key.page.clone() };
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::blocking::run_io(move || {
            work.into_iter().map(|(item, runtime)| {
                let mut runtime = runtime.lock().unwrap_or_else(|error| error.into_inner());
                runtime.set_theme(theme);
                let tree = runtime.render_injection(request.clone())
                    .map_err(plugin_error_to_anyhow)
                    .and_then(|tree| tree.map(view_tree_from_abi).transpose());
                (item, tree, runtime.take_effects(), runtime.is_poisoned())
            }).collect::<Vec<_>>()
        }).await;
        let _ = cx.update(|cx| {
            cx.update_global(|registry: &mut PluginRegistry, _| {
                if registry.render_cache.pending_injections.get(&key) != Some(&ticket) {
                    return;
                }
                registry.render_cache.pending_injections.remove(&key);
                match result {
                    Ok(outputs) => {
                        let mut trees = Vec::new();
                        for (item, tree, effects, poisoned) in outputs {
                            registry.record_passive_effects(effects);
                            match tree {
                                Ok(Some(tree)) => trees.push(RenderedInjection {
                                    plugin_id: item.plugin_id,
                                    tree: Arc::new(tree),
                                    layout: item.layout,
                                }),
                                Ok(None) => {}
                                Err(error) => {
                                    let message = format!("{error:#}");
                                    warn!(plugin_id = %item.plugin_id, slot = ?key.slot, error = %message, "plugin injection render failed");
                                    registry.push_log(item.plugin_id.clone(), abi::LogLevel::Error, format!("Injection: {message}"));
                                    if poisoned {
                                        registry.mark_unhealthy(&item.plugin_id, message);
                                    }
                                }
                            }
                        }
                        registry.render_cache.insert_injections(key, trees);
                    }
                    Err(error) => {
                        warn!(slot = ?key.slot, error = %error, "plugin injection worker failed");
                    }
                }
            });
            cx.refresh_windows();
        });
    }).detach();
}

pub fn injection_registrations(
    cx: &App,
    slot: InjectionSlot,
    page: Option<&str>,
) -> Vec<PluginInjectionRegistration> {
    cx.global::<PluginRegistry>()
        .injections
        .iter()
        .filter(|registration| registration.slot == slot)
        .filter(|registration| {
            registration
                .page
                .as_deref()
                .is_none_or(|registration_page| Some(registration_page) == page)
        })
        .cloned()
        .collect()
}

pub(crate) fn drain_async_app_refreshes(cx: &mut App) -> bool {
    if !cx
        .global::<PluginRegistry>()
        .has_finished_async_app_refreshes()
    {
        return false;
    }

    let changed = cx.update_global(|registry: &mut PluginRegistry, _cx| registry.apply_async_app_refreshes());
    if changed {
        start_pending_activations(cx);
    }
    changed
}

pub fn has_injections(cx: &App, slot: InjectionSlot, page: Option<&str>) -> bool {
    cx.global::<PluginRegistry>().has_injections(slot, page)
}

pub(crate) fn game_mods(cx: &App) -> Vec<super::game_mods::GameMod> {
    cx.try_global::<PluginRegistry>().into_iter().flat_map(|registry| registry.plugins.values())
        .filter(|instance| instance.enabled && matches!(instance.state, PluginLoadState::Loaded { .. }))
        .flat_map(|instance| instance.prepared_resources.game_mods.iter().cloned()).collect()
}

pub fn tool_pages(cx: &App) -> Vec<PluginPage> {
    cx.global::<PluginRegistry>().tool_pages()
}

pub(crate) fn plugin_image(cx: &App, id: &str, path: &str) -> Option<gpui::EncodedImageBytes> {
    let registry = cx.global::<PluginRegistry>();
    let plugin = registry.plugins.get(id).filter(|plugin| plugin.enabled)?;
    if path.starts_with("data:") {
        return decode_inline_plugin_image(path);
    }
    let format = match Path::new(path).extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => gpui::ImageFormat::Png,
        "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
        "webp" => gpui::ImageFormat::Webp,
        "gif" => gpui::ImageFormat::Gif,
        _ => return None,
    };
    let bytes = registry.resource_cache.read(&plugin.manifest, id, None, path).ok()?;
    Some(gpui::EncodedImageBytes::new(format, bytes))
}

/// Inline image data is already supplied by the guest. It grants no file or
/// network access. Encoded bytes are already accounted for by the host view;
/// keep only a decoder safety guard against decompression bombs.
fn decode_inline_plugin_image(src: &str) -> Option<gpui::EncodedImageBytes> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let (mime, payload) = src.split_once(',')?;
    let format = match mime {
        "data:image/png;base64" => image::ImageFormat::Png,
        "data:image/jpeg;base64" => image::ImageFormat::Jpeg,
        _ => return None,
    };
    let bytes = STANDARD.decode(payload).ok()?;
    if image::guess_format(&bytes).ok()? != format {
        return None;
    }
    let (width, height) = image::ImageReader::with_format(
        std::io::Cursor::new(bytes.as_slice()), format,
    ).into_dimensions().ok()?;
    // Hard decoder guard, not a thumbnail or UI resource budget.
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 64 * 1024 * 1024 {
        return None;
    }
    let format = gpui::ImageFormat::from_image_format(format)?;
    Some(gpui::EncodedImageBytes::new(format, bytes))
}

#[cfg(test)]
mod inline_plugin_image_tests {
    use super::*;

    #[test]
    fn accepts_bounded_png_and_rejects_non_raster_or_malformed_data() {
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jR1sAAAAASUVORK5CYII=";
        assert!(decode_inline_plugin_image(png).is_some());
        assert!(decode_inline_plugin_image(&png.replace("image/png", "image/jpeg")).is_none());
        assert!(decode_inline_plugin_image("data:image/svg+xml;base64,PHN2Zy8+").is_none());
        assert!(decode_inline_plugin_image("data:image/png;base64,invalid!").is_none());
        assert!(decode_inline_plugin_image(&format!("data:image/png;base64,{}", "A".repeat(128 * 1024))).is_none());
        assert!(decode_inline_plugin_image("file:///C:/world/world_icon.jpeg").is_none());
    }
}

pub fn statuses(cx: &App) -> Vec<PluginStatus> {
    cx.global::<PluginRegistry>().statuses()
}

pub fn plugin_manifest_snapshot(cx: &App, plugin_id: &str) -> Option<PluginManifest> {
    cx.global::<PluginRegistry>()
        .plugins
        .get(plugin_id)
        .map(|plugin| plugin.manifest.clone())
}

pub fn plugin_logs(cx: &App, plugin_id: &str) -> Vec<PluginLogEntry> {
    cx.global::<PluginRegistry>().plugin_logs(plugin_id)
}

pub fn plugin_log_snapshot(cx: &App, plugin_id: &str) -> Arc<[PluginLogEntry]> {
    cx.global::<PluginRegistry>().plugin_log_snapshot(plugin_id)
}

pub fn active_modal(cx: &App) -> Option<PluginModalState> {
    cx.global::<PluginRegistry>().active_modal()
}

pub fn close_modal(cx: &mut App) {
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.close_modal();
    });
    cx.refresh_windows();
}

fn open_modal(cx: &mut App, request: abi::ModalRequest) -> Result<()> {
    let plugin_id = request.plugin_id.clone();
    let page_id = request.page_id.clone();
    cx.update_global(|registry: &mut PluginRegistry, _cx| registry.open_modal(request))?;
    request_page_render(cx, &plugin_id, &page_id);
    cx.refresh_windows();
    Ok(())
}

pub fn translate_resource(cx: &App, plugin_id: &str, key: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .translate_resource(plugin_id, key)
}

pub fn translate_resource_locale(
    cx: &App,
    plugin_id: &str,
    locale: &str,
    key: &str,
) -> Option<String> {
    cx.global::<PluginRegistry>()
        .translate_resource_locale(plugin_id, locale, key)
}

pub fn save_plugin_config<F>(cx: &mut App, plugin_id: String, content: String, on_complete: F)
where
    F: FnOnce(&mut App, Result<()>) + 'static,
{
    let Some(manifest) = plugin_manifest_snapshot(cx, &plugin_id) else {
        on_complete(cx, Err(anyhow!("unknown plugin {plugin_id}")));
        return;
    };

    let content_for_state = content.clone();
    cx.spawn(async move |cx| {
        let mut result = crate::tasks::runtime::blocking::run_io(move || {
            crate::plugins::manifest::write_user_config(&manifest, &content)
        }).await.map_err(|error| anyhow!("{error}")).and_then(|result| result);
        if result.is_ok() {
            let runtime = cx.update(|cx| {
                cx.global::<PluginRegistry>().plugins.get(&plugin_id)
                    .and_then(|instance| instance.runtime.clone())
            })?;
            if let Some(runtime) = runtime {
                result = crate::tasks::runtime::blocking::run_io(move || {
                    runtime.lock().unwrap_or_else(|error| error.into_inner())
                        .set_config_text(Ok(content_for_state));
                }).await.map_err(|error| anyhow!("{error}"));
            }
        }
        cx.update(|cx| {
            if result.is_ok() {
                cx.update_global(|registry: &mut PluginRegistry, _| {
                    registry.render_cache.invalidate_plugin(&plugin_id);
                });
                dispatch_app_event(cx, PluginEvent {
                    plugin_id: Some(plugin_id),
                    page_id: None,
                    kind: PluginEventKind::Global {
                        name: "config-changed".to_string(),
                        payload: String::new(),
                    },
                });
            }
            on_complete(cx, result);
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub fn set_plugin_enabled<F>(cx: &mut App, plugin_id: String, enabled: bool, on_complete: F)
where
    F: FnOnce(&mut App, Result<()>) + 'static,
{
    if !cx
        .global::<PluginRegistry>()
        .plugins
        .contains_key(&plugin_id)
    {
        on_complete(cx, Err(anyhow!("unknown plugin {plugin_id}")));
        return;
    }
    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();

    cx.spawn(async move |cx| {
        let plugin_id_for_io = plugin_id.clone();
        let persisted = crate::tasks::runtime::blocking::run_io(move || {
            crate::plugins::state::set_enabled(&plugins_dir, &plugin_id_for_io, enabled)
        })
        .await;

        cx.update(|cx| {
            let result = match persisted {
                Ok(result) => result,
                Err(error) => Err(anyhow!("{error}")),
            };
            if result.is_ok() {
                reload_all(cx);
            }
            on_complete(cx, result);
        })?;

        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub fn start_uninstall_plugin_task(cx: &mut App, plugin_id: String) -> Result<String> {
    let Some(manifest) = plugin_manifest_snapshot(cx, &plugin_id) else {
        return Err(anyhow!("unknown plugin {plugin_id}"));
    };
    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();

    let task_id = crate::tasks::task_manager::create_task(
        crate::tasks::task_manager::TaskOptions::new(
            t!("TaskTitle.uninstall_plugin").to_string(),
            TaskStage::QUEUED,
        )
        .detail(plugin_id.clone()),
    )
    .map_err(anyhow::Error::msg)?;
    let worker_task_id = task_id.clone();
    let plugin_id_for_worker = plugin_id.clone();
    let workflow = match crate::tasks::runtime::spawn_io(async move {
        crate::tasks::task_manager::reset_progress(
            &worker_task_id,
            None,
            Some(TaskStage::UNINSTALLING_PLUGIN),
        );
        if crate::tasks::task_manager::is_cancelled(&worker_task_id) {
            return;
        }

        let removed = crate::tasks::runtime::blocking::run_io(move || {
            uninstall_plugin_files(&plugins_dir, &manifest, &plugin_id_for_worker)
        })
        .await;

        if crate::tasks::task_manager::is_cancelled(&worker_task_id) {
            return;
        }
        match removed {
            Ok(Ok(())) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Completed,
                Some("插件已卸载".to_string()),
            ),
            Ok(Err(error)) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Error,
                Some(crate::plugins::manifest::format_error_chain(&error)),
            ),
            Err(error) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Error,
                Some(error),
            ),
        }
    }) {
        Ok(workflow) => workflow,
        Err(error) => {
            crate::tasks::task_manager::finish_task(
                &task_id,
                TaskFinishStatus::Error,
                Some(error.clone()),
            );
            return Err(anyhow!(error));
        }
    };

    let monitor_task_id = task_id.clone();
    let _ = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
            && !crate::tasks::task_manager::is_cancelled(&monitor_task_id)
        {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                TaskFinishStatus::Error,
                Some(format!("插件卸载任务异常结束: {error}")),
            );
        }
    });
    Ok(task_id)
}

pub fn reload_plugin(cx: &mut App, plugin_id: String) -> Result<()> {
    if !cx
        .global::<PluginRegistry>()
        .plugins
        .contains_key(&plugin_id)
    {
        return Err(anyhow!("unknown plugin {plugin_id}"));
    }
    reload_all(cx);
    Ok(())
}

pub fn export_plugin_diagnostics(cx: &App, plugin_id: &str) -> Result<String> {
    cx.global::<PluginRegistry>()
        .export_plugin_diagnostics(plugin_id)
}

pub fn reload_plugins(cx: &mut App) {
    reload_all(cx);
    start_watcher(cx);
}

pub fn start_import_plugin_task(cx: &mut App, source_path: PathBuf) -> Result<String> {
    ensure_loaded(cx);

    let extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case(crate::plugins::manifest::PLUGIN_PACKAGE_EXTENSION) {
        return Err(anyhow!(
            "plugin package must use .{} extension",
            crate::plugins::manifest::PLUGIN_PACKAGE_EXTENSION
        ));
    }

    let Some(file_name) = source_path.file_name().map(ToOwned::to_owned) else {
        return Err(anyhow!(
            "plugin package path has no file name: {}",
            source_path.display()
        ));
    };
    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();
    let detail = file_name.to_string_lossy().into_owned();
    let task_id = crate::tasks::task_manager::create_task(
        crate::tasks::task_manager::TaskOptions::new(
            t!("TaskTitle.import_plugin").to_string(),
            TaskStage::QUEUED,
        )
        .detail(detail),
    )
    .map_err(anyhow::Error::msg)?;
    let worker_task_id = task_id.clone();
    let workflow = match crate::tasks::runtime::spawn_io(async move {
        crate::tasks::task_manager::reset_progress(
            &worker_task_id,
            None,
            Some(TaskStage::INSTALLING_PLUGIN),
        );
        let io_task_id = worker_task_id.clone();
        let imported = crate::tasks::runtime::blocking::run_io(move || {
            if crate::tasks::task_manager::is_cancelled(&io_task_id) {
                bail!("插件导入已取消");
            }

            crate::plugins::manifest::install_package(&source_path, &plugins_dir)
                .with_context(|| format!("install plugin package {}", source_path.display()))?;
            Ok(())
        })
        .await;

        if crate::tasks::task_manager::is_cancelled(&worker_task_id) {
            return;
        }
        match imported {
            Ok(Ok(())) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Completed,
                Some("插件包已导入".to_string()),
            ),
            Ok(Err(error)) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Error,
                Some(crate::plugins::manifest::format_error_chain(&error)),
            ),
            Err(error) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                TaskFinishStatus::Error,
                Some(error),
            ),
        }
    }) {
        Ok(workflow) => workflow,
        Err(error) => {
            crate::tasks::task_manager::finish_task(
                &task_id,
                TaskFinishStatus::Error,
                Some(error.clone()),
            );
            return Err(anyhow!(error));
        }
    };

    let monitor_task_id = task_id.clone();
    let _ = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
            && !crate::tasks::task_manager::is_cancelled(&monitor_task_id)
        {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                TaskFinishStatus::Error,
                Some(format!("插件导入任务异常结束: {error}")),
            );
        }
    });
    Ok(task_id)
}

pub(super) fn notify_job_ready(plugin_id: &str, generation: u64, job_id: String) {
    if let Some(sender) = ASYNC_APP_REFRESH_NOTIFICATION.get().and_then(|sender| {
        sender.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }) {
        let _ = sender.unbounded_send(crate::plugins::watcher::Message::JobReady {
            plugin_id: plugin_id.to_string(), generation, job_id,
        });
    }
}

pub(super) async fn confirm_native_download(
    plugin_id: &str,
    generation: u64,
    sha: &str,
) -> Result<(), abi::PluginError> {
    let sender = ASYNC_APP_REFRESH_NOTIFICATION
        .get()
        .and_then(|sender| sender.lock().ok()?.clone())
        .ok_or_else(|| abi::PluginError::app("native approval requires an open BMCBL window"))?;
    let (reply, decision) = tokio::sync::oneshot::channel();
    sender
        .unbounded_send(crate::plugins::watcher::Message::NativeApproval {
            plugin_id: plugin_id.to_owned(),
            generation,
            sha: sha.to_owned(),
            reply,
        })
        .map_err(|_| abi::PluginError::app("native approval window is unavailable"))?;
    if decision.await == Ok(true) {
        Ok(())
    } else {
        Err(abi::PluginError::denied("native component approval was not granted"))
    }
}

pub(super) fn prompt_native_download(
    cx: &mut App,
    plugin_id: String,
    generation: u64,
    sha: String,
    reply: tokio::sync::oneshot::Sender<bool>,
) {
    static PROMPT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let registry = cx.global::<PluginRegistry>();
    if reply.is_closed() || !registry.plugins.get(&plugin_id).is_some_and(|instance| {
        instance.enabled && instance.generation == generation
    }) {
        return;
    }
    let window = cx.active_window().or_else(|| cx.windows().first().copied());
    let (Ok(permit), Some(window)) = (PROMPT.try_acquire(), window) else {
        return;
    };
    let detail = format!("插件 {plugin_id}\nSHA-256  {sha}\n授权后，该原生组件可在 BMCBL 进程中运行。请确认你信任这个插件和组件。");
    let Ok(decision) = window.update(cx, |_, window, cx| {
        window.prompt(
            gpui::PromptLevel::Warning,
            "授权安装原生组件",
            Some(&detail),
            &[gpui::PromptButton::ok("授权安装"), gpui::PromptButton::cancel("取消")],
            cx,
        )
    }) else { return };
    cx.spawn(async move |_| {
        let _permit = permit;
        let _ = reply.send(decision.await == Ok(0));
    }).detach();
}

pub(super) fn dispatch_job_ready(cx: &mut App, plugin_id: String, generation: u64, job_id: String) {
    let registry = cx.global::<PluginRegistry>();
    if !registry.plugins.get(&plugin_id).is_some_and(|instance| instance.enabled && instance.generation == generation) {
        return;
    }
    if registry.activating.contains(&(plugin_id.clone(), generation)) {
        cx.update_global(|registry: &mut PluginRegistry, _| {
            if registry.early_job_events.len() < 32 {
                registry.early_job_events.push((plugin_id, generation, job_id));
            }
        });
        return;
    }
    dispatch_app_event(cx, PluginEvent {
        plugin_id: Some(plugin_id), page_id: None, kind: PluginEventKind::JobReady { job_id },
    });
}

pub fn dispatch_plugin_action(
    cx: &mut App,
    plugin_id: String,
    page_id: Option<String>,
    action_id: String,
    value: Option<String>,
) {
    let event = PluginEvent {
        plugin_id: Some(plugin_id),
        page_id,
        kind: PluginEventKind::Action { action_id, value },
    };
    dispatch_app_event(cx, event);
}

pub fn show_toast(cx: &mut App, plugin_id: &str, message: String) -> Result<()> {
    let manifest = cx
        .global::<PluginRegistry>()
        .plugins
        .get(plugin_id)
        .map(|plugin| plugin.manifest.clone())
        .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
    manifest.require_capability(PluginCapability::Toast)?;
    crate::ui::components::toast::push(cx, SharedString::from(message));
    Ok(())
}

pub fn open_plugin_link(cx: &mut App, plugin_id: &str, url: &str) -> Result<()> {
    let manifest = cx
        .global::<PluginRegistry>()
        .plugins
        .get(plugin_id)
        .map(|plugin| plugin.manifest.clone())
        .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
    manifest.require_capability(PluginCapability::ExternalOpen)?;
    if !manifest.allows_external_url(url) {
        bail!("plugin {plugin_id} is not allowed to open {url}");
    }
    cx.open_url(url);
    Ok(())
}

pub(crate) fn dispatch_global_event(cx: &mut App, name: String, payload: String) {
    let event = PluginEvent {
        plugin_id: None,
        page_id: None,
        kind: PluginEventKind::Global { name, payload },
    };
    dispatch_app_event(cx, event);
}

pub(crate) fn dispatch_route_changed(cx: &mut App, path: String) {
    let event = PluginEvent {
        plugin_id: None,
        page_id: None,
        kind: PluginEventKind::RouteChanged { path },
    };
    dispatch_app_event(cx, event);
}

fn dispatch_app_event(cx: &mut App, event: PluginEvent) {
    enqueue_app_event(cx, event, Arc::new(Mutex::new(EventCascade::new())));
}

fn enqueue_app_event(cx: &mut App, event: PluginEvent, cascade: Arc<Mutex<EventCascade>>) {
    cx.update_global(|registry: &mut PluginRegistry, _| {
        if registry.event_queue.len() < 128 {
            registry.event_queue.push_back((event, cascade));
        }
    });
    run_app_events(cx);
}

fn run_app_events(cx: &mut App) {
    let event = cx.update_global(|registry: &mut PluginRegistry, _| {
        if registry.event_pending {
            return None;
        }
        let entry = registry.event_queue.pop_front()?;
        registry.event_pending = true;
        Some(entry)
    });
    let Some((event, cascade)) = event else { return };
    let theme = current_theme_snapshot(cx);
    let clipboard = clipboard_text_snapshot_for_event(cx, &event);
    let work = {
        let registry = cx.global::<PluginRegistry>();
        let mut budget = cascade.lock().unwrap_or_else(|error| error.into_inner());
        registry.event_targets(&event).into_iter().filter_map(|id| {
            let instance = registry.plugins.get(&id)?;
            if !instance.enabled || matches!(instance.state, PluginLoadState::Failed { .. })
                || !budget.try_deliver(&id, event.cascade_name()) {
                return None;
            }
            Some((id, instance.generation, instance.runtime.clone()?))
        }).collect::<Vec<_>>()
    };
    let input = plugin_event_to_abi(&event);
    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::blocking::run_io(move || {
            work.into_iter().map(|(id, generation, runtime)| {
                let mut runtime = runtime.lock().unwrap_or_else(|error| error.into_inner());
                runtime.set_theme(theme);
                if let Some(text) = &clipboard {
                    runtime.set_clipboard_text(text.clone());
                }
                let result = runtime.handle_event(input.clone());
                (id, generation, result, runtime.take_effects(), runtime.is_poisoned())
            }).collect::<Vec<_>>()
        }).await;
        let _ = cx.update(|cx| {
            let effects = cx.update_global(|registry: &mut PluginRegistry, _| {
                let mut effects = Vec::new();
                let mut targets = Vec::new();
                for (id, generation, result, pending, poisoned) in result.unwrap_or_default() {
                    if !registry.plugins.get(&id).is_some_and(|instance| {
                        instance.enabled && instance.generation == generation
                    }) {
                        continue;
                    }
                    if let Err(error) = result {
                        let text = format!("plugin event failed: {}", error.message);
                        warn!(plugin_id = %id, error = %text, "plugin event failed");
                        registry.push_log(id.clone(), abi::LogLevel::Error, text.clone());
                        if poisoned {
                            registry.mark_unhealthy(&id, text);
                        }
                    }
                    targets.push(id);
                    effects.extend(pending);
                }
                if !targets.is_empty() {
                    registry.invalidate_render_cache_for_event(&event, &targets);
                }
                effects
            });
            apply_plugin_effects(cx, effects, &cascade);
            cx.update_global(|registry: &mut PluginRegistry, _| registry.event_pending = false);
            run_app_events(cx);
            cx.refresh_windows();
        });
    }).detach();
}

/// 应用一次插件调用产生的宿主效果。
///
/// 插件通过 `emit_event` 触发的后续事件放进同一个队列迭代处理，而不是递归调用本函数；
/// 级联总量由 [`EventCascade`] 限制，保证两个插件互相触发时也能收敛。
fn apply_plugin_effects(
    cx: &mut App,
    effects: Vec<abi::PluginEffect>,
    cascade: &Arc<Mutex<EventCascade>>,
) {
    let mut queue: VecDeque<abi::PluginEffect> = effects.into();
    let mut cascade_warned = false;
    while let Some(effect) = queue.pop_front() {
        match effect {
            abi::PluginEffect::Toast { kind, message, .. } => {
                let kind = match kind {
                    abi::ToastKind::Info => crate::ui::components::toast::ToastKind::Info,
                    abi::ToastKind::Success => crate::ui::components::toast::ToastKind::Success,
                    abi::ToastKind::Error => crate::ui::components::toast::ToastKind::Error,
                };
                crate::ui::components::toast::push_kind(cx, kind, SharedString::from(message));
            }
            abi::PluginEffect::Navigate { target, .. } => {
                let target = route_target_from_abi(target);
                crate::ui::navigation::navigate_target(cx, target);
            }
            abi::PluginEffect::OpenWindow { request, .. } => apply_open_window(cx, request),
            abi::PluginEffect::OpenModal { request, .. } => apply_open_modal(cx, request),
            abi::PluginEffect::CloseWindow { window_id, .. } => {
                warn!(
                    window_id,
                    "plugin close-window effect requested; close by id is not wired yet"
                );
            }
            abi::PluginEffect::EmitEvent { name, payload, .. } => {
                let budget = cascade.lock().unwrap_or_else(|error| error.into_inner());
                if budget.is_exhausted() {
                    if !cascade_warned {
                        cascade_warned = true;
                        warn!(
                            event = %name,
                            deliveries = budget.deliveries(),
                            "plugin event cascade budget exhausted; dropping cascaded plugin events"
                        );
                    }
                    continue;
                }
                drop(budget);
                enqueue_app_event(cx, PluginEvent {
                    plugin_id: None, page_id: None, kind: PluginEventKind::Global { name, payload },
                }, cascade.clone());
            }
            abi::PluginEffect::Invalidate { plugin_id, target } => {
                cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry.invalidate_from_plugin(&plugin_id, target);
                });
            }
            abi::PluginEffect::Log {
                plugin_id,
                level,
                message,
            } => {
                cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry.push_log(plugin_id, level, message);
                });
            }
            abi::PluginEffect::RegisterService {
                plugin_id,
                service_name,
            } => {
                let registered = cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry
                        .service_registry
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .register_service(plugin_id.clone(), service_name.clone())
                });
                if let Err(error) = registered {
                    // 重名服务只影响后来者，不影响插件本体，因此记录后继续。
                    warn!(
                        plugin_id,
                        service_name,
                        error = %error,
                        "plugin service registration rejected"
                    );
                }
            }
            abi::PluginEffect::OpenExternalUrl { url, .. } => {
                cx.open_url(&url);
            }
            abi::PluginEffect::WriteClipboard { text, .. } => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            abi::PluginEffect::PersistStorage {
                plugin_id,
                key,
                value,
            } => {
                if let Some((manifest, storage_dir)) = plugin_persistence_target(cx, &plugin_id) {
                    let _ = schedule_plugin_persistence(PluginPersistenceOp::StorageSet {
                        plugin_id,
                        storage_dir,
                        key,
                        value,
                    });
                    let _ = manifest;
                }
            }
            abi::PluginEffect::DeleteStorage { plugin_id, key } => {
                if let Some((_manifest, storage_dir)) = plugin_persistence_target(cx, &plugin_id) {
                    let _ = schedule_plugin_persistence(PluginPersistenceOp::StorageDelete {
                        plugin_id,
                        storage_dir,
                        key,
                    });
                }
            }
            abi::PluginEffect::PersistConfig { plugin_id, text } => {
                if let Some((manifest, _storage_dir)) = plugin_persistence_target(cx, &plugin_id) {
                    let _ = schedule_plugin_persistence(PluginPersistenceOp::Config {
                        plugin_id,
                        manifest,
                        content: text,
                    });
                }
            }
        }
    }
}

/// 查找插件持久化所需的清单与存储目录；未知插件返回 `None`。
fn plugin_persistence_target(cx: &mut App, plugin_id: &str) -> Option<(PluginManifest, PathBuf)> {
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        let manifest = registry.plugins.get(plugin_id)?.manifest.clone();
        Some((manifest, registry.plugin_storage_dir(plugin_id)))
    })
}

/// 应用插件打开窗口效果；失败时回退到插件提示，避免静默失败。
fn apply_open_window(cx: &mut App, request: abi::WindowRequest) {
    let abi::WindowRequest {
        title,
        plugin_id,
        page_id,
        ..
    } = request;
    if let Err(error) = crate::plugins::window::open(cx, plugin_id.clone(), page_id, title) {
        warn!(error = ?error, "plugin open-window effect failed");
        if let Err(error) = show_toast(cx, &plugin_id, "Unable to open plugin window".to_string()) {
            warn!(error = ?error, "plugin open-window fallback toast failed");
        }
    }
}

/// 应用插件打开模态框效果；失败时回退到插件提示，避免静默失败。
fn apply_open_modal(cx: &mut App, request: abi::ModalRequest) {
    let plugin_id = request.plugin_id.clone();
    if let Err(error) = open_modal(cx, request) {
        let error = error.to_string();
        warn!(
            plugin_id = %plugin_id,
            error = %error,
            "plugin open-modal effect failed"
        );
        if let Err(error) = show_toast(cx, &plugin_id, "Unable to open plugin modal".to_string()) {
            warn!(error = %error, "plugin open-modal fallback toast failed");
        }
    }
}

pub(super) fn current_locale_code() -> String {
    crate::config::settings::read()
        .ok()
        .map(|config| config.launcher.language)
        .filter(|language| !language.trim().eq_ignore_ascii_case("auto"))
        .unwrap_or_else(crate::utils::system_info::system_language)
}

fn load_plugin_translations(
    manifest: &PluginManifest,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut translations = BTreeMap::new();
    let Some(lang_dir) = manifest.lang_dir_path() else {
        return translations;
    };
    let Ok(entries) = std::fs::read_dir(lang_dir) else {
        return translations;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("lang") {
            continue;
        }
        let Some(locale) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(table) = read_lang_file(&path) {
            translations.insert(locale.to_string(), table);
        }
    }
    translations
}

fn read_lang_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read plugin locale file {}", path.display()))?;
    let mut values = BTreeMap::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty() {
            values.insert(key.to_string(), value.trim().to_string());
        }
    }
    Ok(values)
}

fn translate_plugin_key(
    translations: &BTreeMap<String, BTreeMap<String, String>>,
    locale: &str,
    key: &str,
    args: &[abi::I18nArg],
) -> String {
    let Some(value) = translation_candidates(locale)
        .into_iter()
        .filter_map(|candidate| translations.get(&candidate))
        .find_map(|table| table.get(key))
        .or_else(|| translations.values().find_map(|table| table.get(key)))
    else {
        return key.to_string();
    };

    let mut output = value.clone();
    for arg in args {
        output = output.replace(&format!("{{{{{}}}}}", arg.key), &arg.value);
    }
    output
}

fn translation_candidates(locale: &str) -> Vec<String> {
    let locale = locale.replace('_', "-");
    let mut candidates = vec![locale.clone()];
    let lower = locale.to_ascii_lowercase();
    if lower.starts_with("zh-tw") || lower.starts_with("zh-hk") {
        push_unique_candidate(&mut candidates, "zh-TW");
    } else if lower.starts_with("zh") {
        push_unique_candidate(&mut candidates, "zh-CN");
    } else if lower.starts_with("en") {
        push_unique_candidate(&mut candidates, "en-US");
    } else if lower.starts_with("ja") {
        push_unique_candidate(&mut candidates, "ja-JP");
    } else if lower.starts_with("ko") {
        push_unique_candidate(&mut candidates, "ko-KR");
    }
    push_unique_candidate(&mut candidates, "en-US");
    candidates
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: &str) {
    if !candidates.iter().any(|value| value == candidate) {
        candidates.push(candidate.to_string());
    }
}

fn route_target_from_abi(target: abi::RouteTarget) -> crate::ui::navigation::RouteTarget {
    if let (Some(plugin_id), Some(page_id)) = (target.plugin_id, target.page_id) {
        return crate::ui::navigation::RouteTarget::Plugin { plugin_id, page_id };
    }

    crate::ui::navigation::RouteTarget::from_pathname(&target.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_view_cache_evicts_cold_views_but_not_live_page_leases() {
        let mut cache = RenderCache::default();
        let key = |id: &str| PageRenderCacheKey {
            plugin_id: id.into(), page_id: "main".into(),
        };
        let tree = Arc::new(ViewTree { root: 0, nodes: vec![abi::ViewNode::Separator] });
        let active = tree.clone();
        cache.insert_page(key("active"), tree);
        cache.insert_page(key("cold"), Arc::new(ViewTree {
            root: 0, nodes: vec![abi::ViewNode::Separator],
        }));
        for entry in cache.pages.values() {
            entry.last_used.set(Instant::now() - VIEW_CACHE_HOT_AGE - Duration::from_secs(1));
        }
        cache.trim_cold(0);
        assert!(cache.pages.contains_key(&key("active")));
        assert!(!cache.pages.contains_key(&key("cold")));
        drop(active);
        cache.trim_cold(0);
        assert!(cache.pages.is_empty());
    }

    #[test]
    fn storage_file_name_roundtrips_through_key_parsing() {
        for key in ["tasks.sync", "a", "with_underscore:colon-dash"] {
            let file_name = storage_file_name(key);
            assert!(file_name.starts_with("kv-"));
            assert_eq!(
                storage_key_from_file_name(&file_name).as_deref(),
                Some(key)
            );
        }
        assert_eq!(storage_key_from_file_name("kv-zz"), None);
        assert_eq!(storage_key_from_file_name("not-a-key"), None);
    }

    #[test]
    fn storage_keys_delegate_to_runtime_sandbox_rules() {
        assert!(validate_storage_key("tasks.sync").is_ok());
        assert!(validate_storage_key("").is_err());
        assert!(validate_storage_key("UPPER").is_err());
        assert!(validate_storage_key_prefix("tasks.").is_ok());
    }

    #[test]
    fn sanitize_storage_segment_replaces_unsafe_characters() {
        assert_eq!(sanitize_storage_segment("abc-123"), "abc-123");
        assert_eq!(sanitize_storage_segment("../evil"), "..-evil");
    }

    fn test_manifest(capabilities: &[&str]) -> PluginManifest {
        let capability_list = capabilities
            .iter()
            .map(|capability| format!(r#""{capability}""#))
            .collect::<Vec<_>>()
            .join(", ");
        let content = format!(
            r#"
schema_version = 1
id = "policy-plugin"
name = "Policy"
version = "0.1.0"
api_version = "1.0.0"
entry = "plugin.wasm"
capabilities = [{capability_list}]
"#
        );
        PluginManifest::parse("policy", &content).expect("manifest should parse")
    }

    #[test]
    fn plugin_loaded_log_message_lists_capabilities() {
        let manifest = test_manifest(&["ui.page", "toast"]);
        let message = plugin_loaded_log_message(&manifest);
        assert!(message.contains("Policy 0.1.0"));
        assert!(message.contains("ui.page, toast"));
    }

    #[test]
    fn policy_conversion_maps_capabilities() {
        let manifest = test_manifest(&["ui.page", "network.http", "download.start"]);
        let policy = plugin_policy_from_manifest(&manifest);
        assert!(policy.capabilities.ui_page);
        assert!(policy.capabilities.network_http);
        assert!(policy.capabilities.download_start);
        assert!(!policy.capabilities.toast);
        assert_eq!(policy.plugin_id, "policy-plugin");
    }

    #[tokio::test]
    #[ignore = "requires access to a public GitHub Release asset"]
    async fn release_text_checks_each_redirect_without_credentials() {
        let url = "https://github.com/BMCBL/PaperConnect/releases/download/v0.1.0/paperconnect-native-abi1-windows-x86_64-msvc.dll.sha256";
        let mut manifest = test_manifest(&["network.http"]);
        manifest.permissions.network_allow.push(url.to_string());
        let error = fetch_http_text(url, 1024, Some(&manifest)).await.unwrap_err();
        assert!(error.contains("outside the plugin allowlist"), "{error}");
        manifest.permissions.network_allow.extend([
            "https://release-assets.githubusercontent.com/".to_owned(),
            "https://objects.githubusercontent.com/".to_owned(),
        ]);
        let (checksum, _) = fetch_http_text(url, 1024, Some(&manifest)).await.unwrap();
        assert!(checksum.contains("paperconnect-native-abi1-windows-x86_64-msvc.dll"));
    }
}

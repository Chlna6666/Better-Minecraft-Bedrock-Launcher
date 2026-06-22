use crate::plugins::events::{
    CompactBehavior, HostEvent, HostEventKind, InjectionLayout, InjectionSlot,
    PluginInjectionRegistration, PluginNavigationEntry, PluginPageRegistration, sort_injections,
};
use crate::plugins::manifest::{PluginCapability, PluginManifest};
use crate::plugins::ui_dsl::{self, ViewTree};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use anyhow::{Context, Result, anyhow, bail};
use bmcbl_plugin_api as abi;
use gpui::{App, BorrowAppContext, ClipboardItem, Global, Hsla, SharedString};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tinywasm::engine::{Config as TinyConfig, FuelPolicy, MemoryBackend, StackConfig};
use tinywasm::types::{MemoryArch, MemoryType, WasmType, WasmValue};
use tinywasm::{
    Engine, FuncContext, Function, HostFunction, Imports, LinearMemory, Module, ModuleInstance,
    PagedMemory, Store,
};
use tracing::{debug, error, info, warn};

pub const INIT_TIMEOUT: Duration = Duration::from_secs(1);
pub const RENDER_WARN_THRESHOLD: Duration = Duration::from_millis(16);
pub const RENDER_TIMEOUT: Duration = Duration::from_millis(100);
pub const EVENT_TIMEOUT: Duration = Duration::from_millis(50);
pub const PLUGIN_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
const HOST_BUFFER_MAX_BYTES: usize = 1024 * 1024;
const ABI_MESSAGE_MAX_BYTES: usize = 1024 * 1024;
const ABI_IMPORT_MODULE: &str = abi::HOST_MODULE;
const ABI_LEGACY_IMPORT_MODULE: &str = "env";
const ABI_IMPORT_NAME: &str = abi::HOST_CALL_NAME;
const ABI_EXPORT_MEMORY: &str = "memory";
const ABI_EXPORT_ALLOC: &str = "bmcbl_alloc";
const ABI_EXPORT_DEALLOC: &str = "bmcbl_dealloc";
const ABI_EXPORT_INIT: &str = "bmcbl_init";
const ABI_EXPORT_HANDLE_EVENT: &str = "bmcbl_handle_event";
const ABI_EXPORT_RENDER_PAGE: &str = "bmcbl_render_page";
const ABI_EXPORT_RENDER_INJECTION: &str = "bmcbl_render_injection";
const ABI_EXPORT_SHUTDOWN: &str = "bmcbl_shutdown";
const INIT_FUEL_BUDGET: u32 = 1_000_000;
const RENDER_FUEL_BUDGET: u32 = 500_000;
const EVENT_FUEL_BUDGET: u32 = 500_000;
const SHUTDOWN_FUEL_BUDGET: u32 = 250_000;
const HTTP_DEFAULT_TTL: Duration = Duration::from_secs(30 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const HTTP_MAX_BYTES: usize = 512 * 1024;
const STORAGE_KEY_MAX_BYTES: usize = 128;

static HTTP_REFRESH_NOTIFICATION: OnceLock<
    Mutex<Option<crate::plugins::watcher::PluginWatcherSender>>,
> = OnceLock::new();

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
    pub memory_mb: u32,
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
    pub translations: BTreeMap<String, BTreeMap<String, String>>,
    pub state: PluginLoadState,
    pub enabled: bool,
    runtime: Option<Rc<RefCell<PluginExecution>>>,
}

#[derive(Clone, Debug)]
pub(crate) enum HostEffect {
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
}

#[derive(Clone, Debug)]
pub struct RenderedInjection {
    pub plugin_id: String,
    pub tree: Arc<ViewTree>,
    pub layout: Option<InjectionLayout>,
}

#[derive(Clone, Debug)]
enum RenderContext {
    Page {
        page_id: String,
    },
    Injection {
        slot: InjectionSlot,
        page: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum HttpInvalidationTarget {
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

impl HttpInvalidationTarget {
    fn plugin_id(&self) -> &str {
        match self {
            Self::Page { plugin_id, .. } | Self::Injection { plugin_id, .. } => plugin_id,
        }
    }
}

#[derive(Clone, Debug)]
struct HttpCacheSnapshot {
    state: abi::HttpCacheState,
    body: Option<String>,
    error: Option<String>,
    fetched_at_unix_ms: Option<u64>,
}

#[derive(Debug)]
struct HttpCacheEntry {
    body: Option<String>,
    error: Option<String>,
    fetched_at: Option<Instant>,
    fetched_at_unix_ms: Option<u64>,
    refreshing: bool,
    subscribers: BTreeSet<HttpInvalidationTarget>,
}

#[derive(Debug)]
struct HttpRefreshResult {
    url: String,
    result: std::result::Result<(String, u64), String>,
}

#[derive(Clone, Debug)]
struct PluginHttpFetchCache {
    state: Arc<Mutex<PluginHttpFetchCacheState>>,
}

#[derive(Debug)]
struct PluginHttpFetchCacheState {
    entries: BTreeMap<String, HttpCacheEntry>,
    sender: mpsc::Sender<HttpRefreshResult>,
    receiver: Arc<Mutex<mpsc::Receiver<HttpRefreshResult>>>,
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

#[derive(Debug, Default)]
struct RenderCache {
    pages: BTreeMap<PageRenderCacheKey, Arc<ViewTree>>,
    page_errors: BTreeMap<PageRenderCacheKey, Arc<str>>,
    injections: BTreeMap<InjectionRenderCacheKey, Vec<RenderedInjection>>,
}

#[derive(Debug)]
struct HostState {
    plugin_id: String,
    capabilities: BTreeSet<PluginCapability>,
    manifest: PluginManifest,
    locale: String,
    translations: BTreeMap<String, BTreeMap<String, String>>,
    session: BTreeMap<String, String>,
    http_cache: PluginHttpFetchCache,
    render_context: Option<RenderContext>,
    theme_snapshot: abi::ThemeSnapshot,
    storage_dir: PathBuf,
    clipboard_text: Option<String>,
    effects: Vec<HostEffect>,
    next_window_id: u64,
}

struct PluginExecution {
    store: Store,
    instance: ModuleInstance,
    memory: tinywasm::Memory,
    alloc: Function,
    dealloc: Function,
    init: Function,
    handle_event: Function,
    render_page: Function,
    render_injection: Function,
    shutdown: Function,
    host_state: Rc<RefCell<HostState>>,
}

pub struct PluginRegistry {
    plugins_dir: PathBuf,
    cache_dir: PathBuf,
    package_cache_dir: PathBuf,
    engine: Option<Engine>,
    plugins: BTreeMap<String, PluginInstance>,
    pages: BTreeMap<(String, String), PluginPage>,
    injections: Vec<PluginInjectionRegistration>,
    render_cache: RenderCache,
    logs: BTreeMap<String, VecDeque<PluginLogEntry>>,
    http_cache: PluginHttpFetchCache,
    module_cache: BTreeMap<String, Module>,
    generation: u64,
    reload_tx: Option<crate::plugins::watcher::PluginWatcherSender>,
    watcher_task: Option<gpui::Task<()>>,
    last_error: Option<SharedString>,
    loaded_once: bool,
    active_modal: Option<PluginModalState>,
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
    pub wasm_limit_bytes: usize,
    pub render_cache_entries: usize,
    pub render_cache_bytes: usize,
    pub http_cache_entries: usize,
    pub http_cache_body_bytes: usize,
    pub http_cache_error_bytes: usize,
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
        Self::new(plugins_dir, cache_dir, package_cache_dir)
    }
}

impl PluginRegistry {
    pub fn new(plugins_dir: PathBuf, cache_dir: PathBuf, package_cache_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            cache_dir,
            package_cache_dir,
            engine: None,
            plugins: BTreeMap::new(),
            pages: BTreeMap::new(),
            injections: Vec::new(),
            render_cache: RenderCache::default(),
            logs: BTreeMap::new(),
            http_cache: PluginHttpFetchCache::default(),
            module_cache: BTreeMap::new(),
            generation: 0,
            reload_tx: None,
            watcher_task: None,
            last_error: None,
            loaded_once: false,
            active_modal: None,
        }
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
                        memory_mb: instance.manifest.limits.memory_mb,
                        max_http_bytes: instance.manifest.limits.max_http_bytes,
                        max_resource_bytes: instance.manifest.limits.max_resource_bytes,
                        max_storage_bytes: instance.manifest.limits.max_storage_bytes,
                    },
                    has_readme: manifest_has_any_readme(&instance.manifest),
                    has_config: instance
                        .manifest
                        .config_schema_path()
                        .is_some_and(|path| path.exists()),
                    icon_path: instance.manifest.icon_path().filter(|path| path.exists()),
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
                let wasm = instance
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.borrow().memory_snapshot().ok())
                    .unwrap_or_default();
                let render_cache = self
                    .render_cache
                    .plugin_memory(instance.manifest.id.as_str());
                let http_cache = self
                    .http_cache
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
                    .saturating_add(log.bytes);
                PluginMemorySnapshot {
                    plugin_id: instance.manifest.id.clone(),
                    name: SharedString::from(instance.manifest.name.clone()),
                    enabled: instance.enabled,
                    loaded: matches!(instance.state, PluginLoadState::Loaded { .. }),
                    wasm_linear_bytes: wasm.linear_bytes,
                    wasm_page_count: wasm.page_count,
                    wasm_limit_bytes: PLUGIN_MEMORY_LIMIT_BYTES,
                    render_cache_entries: render_cache.entries,
                    render_cache_bytes: render_cache.estimated_bytes,
                    http_cache_entries: http_cache.entries,
                    http_cache_body_bytes: http_cache.body_bytes,
                    http_cache_error_bytes: http_cache.error_bytes,
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
        let module_cache_entries = self.module_cache.len();
        let module_cache_estimated_bytes =
            module_cache_entries.saturating_mul(std::mem::size_of::<Module>());
        let total_estimated_bytes = plugins
            .iter()
            .map(|plugin| plugin.total_estimated_bytes)
            .sum::<usize>()
            .saturating_add(module_cache_estimated_bytes);
        PluginMemoryReport {
            plugins,
            module_cache_entries,
            module_cache_estimated_bytes,
            total_estimated_bytes,
        }
    }

    pub fn plugin_readme(&self, plugin_id: &str) -> Result<Option<String>> {
        self.plugin_readme_for_locale(plugin_id, &current_locale_code())
    }

    pub fn plugin_readme_for_locale(
        &self,
        plugin_id: &str,
        locale: &str,
    ) -> Result<Option<String>> {
        let Some(plugin) = self.plugins.get(plugin_id) else {
            return Ok(None);
        };
        let Some(path) = plugin.manifest.readme_path_for_locale(locale) else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        std::fs::read_to_string(&path)
            .map(Some)
            .with_context(|| format!("read plugin readme {}", path.display()))
    }

    pub fn plugin_config_text(&self, plugin_id: &str) -> Result<Option<String>> {
        let Some(plugin) = self.plugins.get(plugin_id) else {
            return Ok(None);
        };
        let text = crate::plugins::manifest::read_user_config(&plugin.manifest)?;
        Ok((!text.is_empty()).then_some(text))
    }

    pub fn plugin_config_schema(&self, plugin_id: &str) -> Result<Option<String>> {
        let Some(plugin) = self.plugins.get(plugin_id) else {
            return Ok(None);
        };
        let Some(path) = plugin.manifest.config_schema_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        std::fs::read_to_string(&path)
            .map(Some)
            .with_context(|| format!("read plugin config schema {}", path.display()))
    }

    pub fn plugin_logs(&self, plugin_id: &str) -> Vec<PluginLogEntry> {
        self.logs
            .get(plugin_id)
            .map(|logs| logs.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn translate_plugin_resource(&self, plugin_id: &str, key: &str) -> Option<String> {
        self.translate_plugin_resource_for_locale(plugin_id, &current_locale_code(), key)
    }

    pub fn translate_plugin_resource_for_locale(
        &self,
        plugin_id: &str,
        locale: &str,
        key: &str,
    ) -> Option<String> {
        let plugin = self.plugins.get(plugin_id)?;
        let translated = translate_plugin_key(&plugin.translations, locale, key, &[]);
        (translated != key).then_some(translated)
    }

    pub fn write_plugin_config(
        &mut self,
        plugin_id: &str,
        content: &str,
    ) -> Result<Vec<HostEffect>> {
        let manifest = self
            .plugins
            .get(plugin_id)
            .map(|plugin| plugin.manifest.clone())
            .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
        crate::plugins::manifest::write_user_config(&manifest, content)?;
        self.render_cache.invalidate_plugin(plugin_id);
        Ok(self.handle_event(HostEvent {
            plugin_id: Some(plugin_id.to_string()),
            page_id: None,
            kind: HostEventKind::Global {
                name: "config-changed".to_string(),
                payload: String::new(),
            },
        }))
    }

    pub fn set_plugin_enabled(&mut self, plugin_id: &str, enabled: bool) -> Result<()> {
        crate::plugins::state::set_plugin_enabled(&self.plugins_dir, plugin_id, enabled)?;
        self.reload_all()
    }

    pub fn uninstall_plugin(&mut self, plugin_id: &str) -> Result<()> {
        let manifest = self
            .plugins
            .get(plugin_id)
            .map(|plugin| plugin.manifest.clone())
            .ok_or_else(|| anyhow!("unknown plugin {plugin_id}"))?;
        let root_dir = std::fs::canonicalize(&manifest.root_dir)
            .with_context(|| format!("canonicalize plugin {}", manifest.root_dir.display()))?;
        let plugins_dir = std::fs::canonicalize(&self.plugins_dir)
            .with_context(|| format!("canonicalize plugins dir {}", self.plugins_dir.display()))?;
        if !root_dir.starts_with(&plugins_dir) || root_dir == plugins_dir {
            bail!("refusing to uninstall plugin outside plugin directory");
        }
        fs::remove_dir_all(&root_dir)
            .with_context(|| format!("remove plugin directory {}", root_dir.display()))?;
        crate::plugins::state::remove_plugin_state(&self.plugins_dir, plugin_id)?;
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
            "Limits memory={}MB http={} resource={} storage={}\n",
            plugin.manifest.limits.memory_mb,
            plugin.manifest.limits.max_http_bytes,
            plugin.manifest.limits.max_resource_bytes,
            plugin.manifest.limits.max_storage_bytes
        ));
        output.push_str(&format!("Root: {}\n", plugin.manifest.root_dir.display()));
        output.push_str("Logs:\n");
        for entry in logs {
            output.push_str(&format!("- {:?}: {}\n", entry.level, entry.message));
        }
        Ok(output)
    }

    fn push_log(&mut self, plugin_id: String, level: abi::LogLevel, message: String) {
        const MAX_PLUGIN_LOGS: usize = 200;
        let logs = self.logs.entry(plugin_id).or_default();
        logs.push_back(PluginLogEntry {
            level,
            message: SharedString::from(message),
        });
        while logs.len() > MAX_PLUGIN_LOGS {
            logs.pop_front();
        }
    }

    pub fn pages(&self) -> Vec<PluginPage> {
        self.pages.values().cloned().collect()
    }

    pub fn navigation_pages(&self) -> Vec<PluginPage> {
        let mut pages = self
            .pages
            .values()
            .filter(|page| page.navigation.is_some())
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
            navigation: Some(PluginNavigationEntry {
                label: manifest.name.clone(),
                icon: Some("plug".to_string()),
                order: 1000,
            }),
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
                    icon_path: instance.manifest.icon_path().filter(|path| path.exists()),
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

    fn cached_page(&self, plugin_id: &str, page_id: &str) -> Option<Arc<ViewTree>> {
        self.render_cache
            .pages
            .get(&PageRenderCacheKey {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.to_string(),
            })
            .cloned()
    }

    fn cached_page_error(&self, plugin_id: &str, page_id: &str) -> Option<Arc<str>> {
        self.render_cache
            .page_errors
            .get(&PageRenderCacheKey {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.to_string(),
            })
            .cloned()
    }

    fn cached_injections(
        &self,
        slot: InjectionSlot,
        page: Option<&str>,
    ) -> Option<Vec<RenderedInjection>> {
        self.render_cache
            .injections
            .get(&InjectionRenderCacheKey {
                slot,
                page: page.map(str::to_string),
            })
            .cloned()
    }

    pub fn last_error(&self) -> Option<SharedString> {
        self.last_error.clone()
    }

    pub fn loaded_once(&self) -> bool {
        self.loaded_once
    }

    pub fn init_engine(&mut self) -> Result<()> {
        if self.engine.is_some() {
            return Ok(());
        }

        std::fs::create_dir_all(&self.cache_dir)
            .with_context(|| format!("create wasm cache {}", self.cache_dir.display()))?;
        let config = TinyConfig::new()
            .with_fuel_policy(FuelPolicy::Weighted)
            .with_memory_backend(plugin_memory_backend())
            .with_call_stack(StackConfig::fixed(256))
            .with_value_stack(StackConfig::dynamic(1024, 16 * 1024))
            .with_trap_on_oom(true);
        self.engine = Some(Engine::new(config));
        Ok(())
    }

    pub fn reload_all(&mut self) -> Result<()> {
        let manifests = crate::plugins::manifest::load_manifests_from_sources(
            &self.plugins_dir,
            &self.package_cache_dir,
        )?;
        self.reload_manifests(manifests)
    }

    pub fn reload_manifests(&mut self, manifests: Vec<PluginManifest>) -> Result<()> {
        let mut next_plugins = BTreeMap::new();
        let mut next_pages = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let disabled_plugins = crate::plugins::state::disabled_plugins(&self.plugins_dir);
        let next_generation = self.generation.saturating_add(1);

        for manifest in manifests {
            if !seen.insert(manifest.id.clone()) {
                return Err(anyhow!("duplicate plugin id {}", manifest.id));
            }

            let previous = self.plugins.get(&manifest.id).cloned();
            let enabled = !disabled_plugins.contains(&manifest.id);
            let shutdown_reason = if enabled {
                abi::ShutdownReason::Reload
            } else {
                abi::ShutdownReason::Unload
            };
            if let Some(previous) = previous
                .as_ref()
                .filter(|instance| matches!(instance.state, PluginLoadState::Loaded { .. }))
                && let Err(error) = previous.shutdown(shutdown_reason)
            {
                warn!(
                    plugin_id = previous.manifest.id,
                    error = %crate::plugins::manifest::format_error_chain(&error),
                    "plugin shutdown before manifest reload failed"
                );
            }

            if enabled
                && let Err(error) = crate::plugins::manifest::commit_installed_package(&manifest)
            {
                warn!(
                    plugin_id = manifest.id,
                    error = %crate::plugins::manifest::format_error_chain(&error),
                    "failed to finalize installed plugin package"
                );
            }

            let pages = enabled
                .then(|| Self::default_page_registrations(&manifest))
                .unwrap_or_default();
            let instance = PluginInstance {
                manifest: manifest.clone(),
                generation: next_generation,
                pages,
                injections: Vec::new(),
                subscriptions: BTreeSet::new(),
                translations: load_plugin_translations(&manifest),
                state: PluginLoadState::Unloaded,
                enabled,
                runtime: None,
            };
            Self::insert_pages(&mut next_pages, &instance);
            next_plugins.insert(manifest.id.clone(), instance);
        }

        for (plugin_id, previous) in &self.plugins {
            if seen.contains(plugin_id) {
                continue;
            }
            if !matches!(previous.state, PluginLoadState::Loaded { .. }) {
                continue;
            }
            if let Err(error) = previous.shutdown(abi::ShutdownReason::Unload) {
                warn!(
                    plugin_id,
                    error = %crate::plugins::manifest::format_error_chain(&error),
                    "plugin shutdown before unload failed"
                );
            }
        }

        self.generation = next_generation;
        self.plugins = next_plugins;
        self.pages = next_pages;
        self.injections.clear();
        self.render_cache.clear();
        self.module_cache.clear();
        self.loaded_once = true;
        Ok(())
    }

    fn set_theme_snapshot(&mut self, snapshot: abi::ThemeSnapshot) {
        for instance in self.plugins.values_mut() {
            if let Some(runtime) = instance.runtime.as_ref() {
                runtime.borrow_mut().host_state.borrow_mut().theme_snapshot = snapshot;
            }
        }
    }

    fn set_clipboard_snapshot(&mut self, text: Option<String>) {
        for instance in self.plugins.values_mut() {
            if let Some(runtime) = instance.runtime.as_ref() {
                runtime.borrow_mut().set_clipboard_snapshot(text.clone());
            }
        }
    }

    fn load_manifest(&mut self, manifest: PluginManifest) -> Result<PluginInstance> {
        let mut execution = self.instantiate_plugin(&manifest)?;

        let started = Instant::now();
        let context = abi::PluginContext {
            plugin_id: manifest.id.clone(),
            api_version: manifest.api_version.clone(),
        };
        let registrations: Vec<abi::Registration> =
            execution.call_entry(EntryCall::Init(context), INIT_FUEL_BUDGET, INIT_TIMEOUT)?;
        let elapsed = started.elapsed();
        if elapsed > INIT_TIMEOUT {
            return Err(anyhow!("plugin init exceeded {:?}", INIT_TIMEOUT));
        }
        let effects = execution.drain_effects();
        if !effects.is_empty() {
            warn!(
                plugin_id = manifest.id,
                count = effects.len(),
                "plugin init emitted host effects; effects are ignored during init"
            );
        }

        let mut pages = Vec::new();
        let mut injections = Vec::new();
        let mut subscriptions = BTreeSet::new();

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
            }
        }

        if pages.is_empty() && manifest.has_capability(&PluginCapability::UiPage) {
            pages.push(PluginPageRegistration {
                plugin_id: manifest.id.clone(),
                page_id: "main".to_string(),
                title: manifest.name.clone(),
                navigation: Some(PluginNavigationEntry {
                    label: manifest.name.clone(),
                    icon: Some("plug".to_string()),
                    order: 1000,
                }),
            });
        }

        let translations = execution.host_state.borrow().translations.clone();
        Ok(PluginInstance {
            manifest,
            generation: self.generation.saturating_add(1),
            pages,
            injections,
            subscriptions,
            translations,
            state: PluginLoadState::Loaded {
                generation: self.generation.saturating_add(1),
            },
            enabled: true,
            runtime: Some(Rc::new(RefCell::new(execution))),
        })
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

        let manifest = existing.manifest.clone();
        match self.load_manifest(manifest) {
            Ok(mut instance) => {
                if let Some(previous) = self
                    .plugins
                    .get(plugin_id)
                    .filter(|instance| matches!(instance.state, PluginLoadState::Loaded { .. }))
                    && let Err(error) = previous.shutdown(abi::ShutdownReason::Reload)
                {
                    warn!(
                        plugin_id = previous.manifest.id,
                        error = %crate::plugins::manifest::format_error_chain(&error),
                        "plugin shutdown before lazy reload failed"
                    );
                }

                info!(
                    plugin_id = %instance.manifest.id,
                    name = %instance.manifest.name,
                    version = %instance.manifest.version,
                    authors = ?instance.manifest.authors,
                    capabilities = ?instance.manifest.capabilities,
                    pages = instance.pages.len(),
                    injections = instance.injections.len(),
                    subscriptions = instance.subscriptions.len(),
                    "plugin loaded"
                );
                self.push_log(
                    instance.manifest.id.clone(),
                    abi::LogLevel::Info,
                    plugin_loaded_log_message(&instance.manifest),
                );

                self.pages
                    .retain(|(page_plugin_id, _page_id), _page| page_plugin_id != plugin_id);
                self.injections
                    .retain(|registration| registration.plugin_id.as_str() != plugin_id);
                Self::insert_pages(&mut self.pages, &instance);
                self.injections.extend(instance.injections.clone());
                sort_injections(&mut self.injections);
                instance.state = PluginLoadState::Loaded {
                    generation: instance.generation,
                };
                self.render_cache.invalidate_plugin(plugin_id);
                self.plugins.insert(plugin_id.to_string(), instance);
                Ok(())
            }
            Err(error) => {
                let error_message = crate::plugins::manifest::format_error_chain(&error);
                warn!(
                    plugin_id,
                    error = %error_message,
                    "plugin lazy load failed"
                );
                if let Some(instance) = self.plugins.get(plugin_id)
                    && let Err(rollback_error) =
                        crate::plugins::manifest::rollback_installed_package(&instance.manifest)
                {
                    warn!(
                        plugin_id,
                        error = %crate::plugins::manifest::format_error_chain(&rollback_error),
                        "failed to roll back installed plugin package"
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

    fn instantiate_plugin(&mut self, manifest: &PluginManifest) -> Result<PluginExecution> {
        self.init_engine()?;
        let wasm_path = manifest.wasm_path();
        if !wasm_path.exists() {
            return Err(anyhow!(
                "plugin wasm entry missing: {}",
                wasm_path.display()
            ));
        }

        let Some(engine) = &self.engine else {
            return Err(anyhow!("plugin engine is not initialized"));
        };

        let wasm = std::fs::read(&wasm_path)
            .with_context(|| format!("read plugin wasm {}", wasm_path.display()))?;
        let wasm_hash = crate::plugins::manifest::sha256_hex(&wasm);
        let module = if let Some(module) = self.module_cache.get(&wasm_hash) {
            module.clone()
        } else {
            let module = tinywasm::parse_bytes(&wasm).map_err(|error| {
                anyhow!("parse plugin wasm {} failed: {error}", wasm_path.display())
            })?;
            validate_module_abi(&module, manifest.limits.memory_mb)?;
            self.module_cache.insert(wasm_hash, module.clone());
            module
        };

        let locale = current_locale_code();
        let translations = load_plugin_translations(manifest);
        let storage_dir = self.plugin_storage_dir(&manifest.id);
        let host_state = Rc::new(RefCell::new(HostState::new(
            manifest,
            locale,
            translations,
            self.http_cache.clone(),
            storage_dir,
        )));
        let mut store = Store::new(engine.clone());
        let imports = host_imports(&mut store, host_state.clone());
        let instance = ModuleInstance::instantiate(&mut store, &module, Some(imports))
            .map_err(|error| anyhow!("instantiate plugin module failed: {error}"))?;
        let memory = instance
            .memory(ABI_EXPORT_MEMORY)
            .map_err(|error| anyhow!("plugin missing exported memory: {error}"))?;
        let alloc = alloc_function_export(&instance, &store)?;
        let dealloc = dealloc_function_export(&instance, &store)?;
        let init = entry_function_export(&instance, &store, ABI_EXPORT_INIT)?;
        let handle_event = entry_function_export(&instance, &store, ABI_EXPORT_HANDLE_EVENT)?;
        let render_page = entry_function_export(&instance, &store, ABI_EXPORT_RENDER_PAGE)?;
        let render_injection =
            entry_function_export(&instance, &store, ABI_EXPORT_RENDER_INJECTION)?;
        let shutdown = entry_function_export(&instance, &store, ABI_EXPORT_SHUTDOWN)?;

        Ok(PluginExecution {
            store,
            instance,
            memory,
            alloc,
            dealloc,
            init,
            handle_event,
            render_page,
            render_injection,
            shutdown,
            host_state,
        })
    }

    pub fn render_page(&mut self, plugin_id: &str, page_id: &str) -> Result<Arc<ViewTree>> {
        self.ensure_plugin_runtime(plugin_id)?;

        let cache_key = PageRenderCacheKey {
            plugin_id: plugin_id.to_string(),
            page_id: page_id.to_string(),
        };
        if let Some(tree) = self.render_cache.pages.get(&cache_key) {
            return Ok(tree.clone());
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
        let tree = {
            let mut runtime = runtime.borrow_mut();
            runtime.set_render_context(Some(RenderContext::Page {
                page_id: page_id.to_string(),
            }));
            let tree = runtime.call_entry(
                EntryCall::RenderPage(request),
                RENDER_FUEL_BUDGET,
                RENDER_TIMEOUT,
            );
            runtime.set_render_context(None);
            let tree = tree?;
            let effects = runtime.drain_effects();
            if !effects.is_empty() {
                warn!(
                    plugin_id,
                    page_id,
                    count = effects.len(),
                    "plugin render emitted host effects; effects are ignored during render"
                );
            }
            view_tree_from_abi(tree)
        };
        let elapsed = started.elapsed();
        if elapsed > RENDER_TIMEOUT {
            let error = format!("plugin render exceeded {RENDER_TIMEOUT:?}");
            self.render_cache
                .page_errors
                .insert(cache_key, Arc::<str>::from(error.clone()));
            self.mark_unhealthy(plugin_id, error);
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
                self.render_cache.pages.insert(cache_key, tree.clone());
                Ok(tree)
            }
            Err(error) => {
                let error_message = error.to_string();
                self.render_cache
                    .page_errors
                    .insert(cache_key, Arc::<str>::from(error_message.clone()));
                self.mark_unhealthy(plugin_id, error_message);
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
            return trees.clone();
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
        self.render_cache
            .injections
            .insert(cache_key, trees.clone());
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
        let mut runtime = runtime.borrow_mut();
        runtime.set_render_context(Some(RenderContext::Injection {
            slot,
            page: page.map(str::to_string),
        }));
        let tree = runtime.call_entry(
            EntryCall::RenderInjection(request),
            RENDER_FUEL_BUDGET,
            RENDER_TIMEOUT,
        );
        runtime.set_render_context(None);
        let tree: Option<abi::ViewTree> = tree?;
        let effects = runtime.drain_effects();
        if !effects.is_empty() {
            warn!(
                plugin_id,
                count = effects.len(),
                "plugin injection render emitted host effects; effects are ignored during render"
            );
        }

        tree.map(view_tree_from_abi)
            .transpose()?
            .ok_or_else(|| anyhow!("plugin did not render injection for requested slot"))
    }

    pub(crate) fn handle_event(&mut self, event: HostEvent) -> Vec<HostEffect> {
        let started = Instant::now();
        let target_hint = self.event_targets(&event);
        match &event.kind {
            HostEventKind::Action { .. } => {
                for plugin_id in target_hint {
                    if let Err(error) = self.ensure_plugin_runtime(&plugin_id) {
                        warn!(plugin_id, error = %error, "plugin action target load failed");
                    }
                }
            }
            HostEventKind::Global { .. } | HostEventKind::RouteChanged { .. } => {
                self.ensure_plugins_with_capability(PluginCapability::EventGlobal);
            }
        }
        let targets = self.event_targets(&event);
        self.invalidate_render_cache_for_event(&event, &targets);
        let mut effects = Vec::new();
        for plugin_id in targets {
            debug!(plugin_id, event = ?event.kind, "dispatch plugin event");
            let Some(runtime) = self
                .plugins
                .get(&plugin_id)
                .and_then(|plugin| plugin.runtime.clone())
            else {
                continue;
            };
            let event = host_event_to_abi(&event);
            let result = {
                let mut runtime = runtime.borrow_mut();
                let result: Result<()> = runtime.call_entry(
                    EntryCall::HandleEvent(event),
                    EVENT_FUEL_BUDGET,
                    EVENT_TIMEOUT,
                );
                effects.extend(runtime.drain_effects());
                result
            };
            if let Err(error) = result {
                let error = error.to_string();
                warn!(plugin_id, error = %error, "plugin event failed");
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

    fn invalidate_render_cache_for_event(&mut self, event: &HostEvent, targets: &[String]) {
        match &event.kind {
            HostEventKind::Action { .. } => {
                if let Some(plugin_id) = event.plugin_id.as_deref() {
                    self.render_cache
                        .invalidate_page(plugin_id, event.page_id.as_deref());
                }
            }
            HostEventKind::Global { .. } | HostEventKind::RouteChanged { .. } => {
                for plugin_id in targets {
                    self.render_cache.invalidate_plugin(plugin_id);
                }
                self.render_cache.invalidate_all_injections();
            }
        }
    }

    fn event_targets(&self, event: &HostEvent) -> Vec<String> {
        match &event.kind {
            HostEventKind::Global { name, .. } => self
                .plugins
                .values()
                .filter(|plugin| plugin.subscriptions.contains(name))
                .map(|plugin| plugin.manifest.id.clone())
                .collect(),
            HostEventKind::RouteChanged { .. } => self
                .plugins
                .values()
                .filter(|plugin| plugin.subscriptions.contains("route-changed"))
                .map(|plugin| plugin.manifest.id.clone())
                .collect(),
            HostEventKind::Action { .. } => event.plugin_id.iter().cloned().collect(),
        }
    }

    fn plugin_runtime(
        &self,
        plugin_id: &str,
    ) -> Result<(PluginManifest, Rc<RefCell<PluginExecution>>)> {
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
        if let Some(instance) = self.plugins.get_mut(plugin_id) {
            instance.state = PluginLoadState::Failed {
                generation: instance.generation,
                error,
            };
        }
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

    fn apply_http_refreshes(&mut self) -> bool {
        let invalidations = self.http_cache.drain_finished();
        if invalidations.is_empty() {
            return false;
        }
        for invalidation in invalidations {
            match invalidation {
                HttpInvalidationTarget::Page { plugin_id, page_id } => {
                    self.render_cache
                        .invalidate_plugin_page(&plugin_id, &page_id);
                }
                HttpInvalidationTarget::Injection {
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

    pub fn set_watcher(
        &mut self,
        sender: crate::plugins::watcher::PluginWatcherSender,
        task: gpui::Task<()>,
    ) {
        self.reload_tx = Some(sender);
        self.watcher_task = Some(task);
    }
}

impl PluginInstance {
    fn shutdown(&self, reason: abi::ShutdownReason) -> Result<()> {
        let Some(runtime) = self.runtime.clone() else {
            return Ok(());
        };

        let mut runtime = runtime.borrow_mut();
        runtime.call_entry::<()>(
            EntryCall::Shutdown(reason),
            SHUTDOWN_FUEL_BUDGET,
            EVENT_TIMEOUT,
        )?;
        let effects = runtime.drain_effects();
        if !effects.is_empty() {
            warn!(
                plugin_id = self.manifest.id,
                count = effects.len(),
                "plugin shutdown emitted host effects; effects are ignored during shutdown"
            );
        }
        Ok(())
    }
}

impl HostState {
    fn new(
        manifest: &PluginManifest,
        locale: String,
        translations: BTreeMap<String, BTreeMap<String, String>>,
        http_cache: PluginHttpFetchCache,
        storage_dir: PathBuf,
    ) -> Self {
        Self {
            plugin_id: manifest.id.clone(),
            capabilities: manifest.capabilities.clone(),
            manifest: manifest.clone(),
            locale,
            translations,
            session: BTreeMap::new(),
            http_cache,
            render_context: None,
            theme_snapshot: abi::ThemeSnapshot::light_default(),
            storage_dir,
            clipboard_text: None,
            effects: Vec::new(),
            next_window_id: 1,
        }
    }

    fn has_capability(&self, capability: &PluginCapability) -> bool {
        self.capabilities.contains(capability)
    }

    fn require_capability(
        &self,
        capability: PluginCapability,
    ) -> std::result::Result<(), abi::HostError> {
        if self.has_capability(&capability) {
            return Ok(());
        }

        Err(abi::HostError {
            code: "capability-denied".to_string(),
            message: format!(
                "plugin {} did not declare {}",
                self.plugin_id,
                capability.as_str()
            ),
        })
    }
}

impl PluginExecution {
    fn set_render_context(&mut self, context: Option<RenderContext>) {
        self.host_state.borrow_mut().render_context = context;
    }

    fn set_clipboard_snapshot(&mut self, text: Option<String>) {
        self.host_state.borrow_mut().clipboard_text = text;
    }

    fn memory_snapshot(&self) -> Result<PluginWasmMemorySnapshot> {
        Ok(PluginWasmMemorySnapshot {
            linear_bytes: self.memory.len(&self.store)?,
            page_count: self.memory.page_count(&self.store)?,
        })
    }

    fn drain_effects(&mut self) -> Vec<HostEffect> {
        std::mem::take(&mut self.host_state.borrow_mut().effects)
    }

    fn call_entry<R>(&mut self, call: EntryCall, fuel: u32, timeout: Duration) -> Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        let (function, request_bytes) = match call {
            EntryCall::Init(context) => (self.init.clone(), encode_request(&context)?),
            EntryCall::HandleEvent(event) => (self.handle_event.clone(), encode_request(&event)?),
            EntryCall::RenderPage(request) => (self.render_page.clone(), encode_request(&request)?),
            EntryCall::RenderInjection(request) => {
                (self.render_injection.clone(), encode_request(&request)?)
            }
            EntryCall::Shutdown(reason) => (self.shutdown.clone(), encode_request(&reason)?),
        };

        if request_bytes.len() > ABI_MESSAGE_MAX_BYTES {
            bail!("plugin request exceeds {ABI_MESSAGE_MAX_BYTES} bytes");
        }
        let request_ptr = self.alloc_bytes(&request_bytes)?;

        let result = (|| {
            let args = [
                (request_ptr as i32).into(),
                (request_bytes.len() as i32).into(),
            ];
            let mut execution = function
                .call_resumable(&mut self.store, &args)
                .map_err(|error| anyhow!("start plugin call failed: {error}"))?;
            let started = Instant::now();
            loop {
                match execution
                    .resume_with_fuel(fuel)
                    .map_err(|error| anyhow!("resume plugin call failed: {error}"))?
                {
                    tinywasm::ExecProgress::Completed(result) => {
                        let Some(WasmValue::I64(packed)) = result.first().copied() else {
                            bail!("plugin call returned unexpected result shape");
                        };
                        return self.read_plugin_result::<R>(packed);
                    }
                    tinywasm::ExecProgress::Suspended => {
                        if started.elapsed() > timeout {
                            bail!("plugin call exceeded {:?}", timeout);
                        }
                    }
                }
            }
        })();

        self.deallocate(request_ptr, request_bytes.len())?;
        result
    }

    fn alloc_bytes(&mut self, bytes: &[u8]) -> Result<u32> {
        let ptr = self
            .alloc
            .call(
                &mut self.store,
                &[(bytes.len() as i32).into(), 1_i32.into()],
            )
            .map_err(|error| anyhow!("plugin alloc failed: {error}"))?;
        let Some(WasmValue::I32(ptr)) = ptr.first().copied() else {
            bail!("plugin alloc returned unexpected value");
        };
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|error| anyhow!("write guest memory failed: {error}"))?;
        Ok(ptr as u32)
    }

    fn deallocate(&mut self, ptr: u32, len: usize) -> Result<()> {
        if ptr == 0 || len == 0 {
            return Ok(());
        }
        self.dealloc
            .call(
                &mut self.store,
                &[(ptr as i32).into(), (len as i32).into(), 1_i32.into()],
            )
            .map_err(|error| anyhow!("plugin dealloc failed: {error}"))?;
        Ok(())
    }

    fn read_plugin_result<R: serde::de::DeserializeOwned>(&mut self, packed: i64) -> Result<R> {
        let packed = packed as u64;
        let ptr = (packed >> 32) as u32;
        let len = (packed & 0xffff_ffff) as usize;
        if len > ABI_MESSAGE_MAX_BYTES {
            bail!("plugin response exceeds {ABI_MESSAGE_MAX_BYTES} bytes");
        }
        let bytes = self
            .memory
            .read_vec(&self.store, ptr as usize, len)
            .map_err(|error| anyhow!("read guest memory failed: {error}"))?;
        self.deallocate(ptr, len)?;

        let response = postcard::from_bytes::<abi::AbiResult<R>>(&bytes)
            .map_err(|error| anyhow!("decode plugin response failed: {error}"))?;
        match response {
            abi::AbiResult::Ok(value) => Ok(value),
            abi::AbiResult::Err(error) => Err(plugin_error_to_anyhow(error)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct PluginWasmMemorySnapshot {
    linear_bytes: usize,
    page_count: usize,
}

enum EntryCall {
    Init(abi::PluginContext),
    HandleEvent(abi::HostEvent),
    RenderPage(abi::PageRenderRequest),
    RenderInjection(abi::InjectionRequest),
    Shutdown(abi::ShutdownReason),
}

fn entry_function_export(instance: &ModuleInstance, store: &Store, name: &str) -> Result<Function> {
    instance
        .func::<(i32, i32), i64>(store, name)
        .map(|typed| typed.func)
        .map_err(|error| anyhow!("plugin missing export {name}: {error}"))
}

fn alloc_function_export(instance: &ModuleInstance, store: &Store) -> Result<Function> {
    instance
        .func::<(i32, i32), i32>(store, ABI_EXPORT_ALLOC)
        .map(|typed| typed.func)
        .map_err(|error| anyhow!("plugin missing export {ABI_EXPORT_ALLOC}: {error}"))
}

fn dealloc_function_export(instance: &ModuleInstance, store: &Store) -> Result<Function> {
    instance
        .func::<(i32, i32, i32), ()>(store, ABI_EXPORT_DEALLOC)
        .map(|typed| typed.func)
        .map_err(|error| anyhow!("plugin missing export {ABI_EXPORT_DEALLOC}: {error}"))
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
        &LightColors::colors(),
        &DarkColors::colors(),
        dark_factor,
        theme.accent,
    );
    theme_snapshot_from_colors(theme.target_dark, dark_factor, colors)
}

fn clipboard_text_snapshot(cx: &mut App) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
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

fn encode_request<T: serde::Serialize>(request: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(request).map_err(|error| anyhow!("encode plugin request failed: {error}"))
}

fn validate_module_abi(module: &Module, memory_limit_mb: u32) -> Result<()> {
    let imports = module.imports().collect::<Vec<_>>();
    if imports.len() != 1 {
        bail!("plugin must import exactly one host function");
    }
    let import = &imports[0];
    if !is_supported_host_import(import.module, import.name) {
        bail!(
            "plugin imports unsupported host function {}.{}; expected {}.{} or {}.{}",
            import.module,
            import.name,
            ABI_IMPORT_MODULE,
            ABI_IMPORT_NAME,
            ABI_LEGACY_IMPORT_MODULE,
            ABI_IMPORT_NAME
        );
    }
    let tinywasm::types::ImportType::Func(func_type) = import.ty else {
        bail!("plugin host import must be a function");
    };
    if func_type.params()
        != [
            WasmType::I32,
            WasmType::I32,
            WasmType::I32,
            WasmType::I32,
            WasmType::I32,
        ]
        || func_type.results() != [WasmType::I64]
    {
        bail!("plugin host import has invalid signature");
    }

    let memory_types = module.memory_types.as_ref();
    if memory_types.len() != 1 {
        bail!("plugin must define exactly one linear memory");
    }
    let memory_type = &memory_types[0];
    if memory_type.arch() != MemoryArch::I32 {
        bail!("plugin memory must use 32-bit addressing");
    }
    let initial_bytes = memory_type
        .page_count_initial()
        .checked_mul(memory_type.page_size())
        .unwrap_or(u64::MAX);
    let memory_limit_bytes = u64::from(memory_limit_mb).saturating_mul(1024 * 1024);
    if initial_bytes > memory_limit_bytes {
        bail!(
            "plugin initial memory must be <= {} bytes",
            memory_limit_bytes
        );
    }

    let exports = module.exports().collect::<Vec<_>>();
    for required in [
        ABI_EXPORT_MEMORY,
        ABI_EXPORT_ALLOC,
        ABI_EXPORT_DEALLOC,
        ABI_EXPORT_INIT,
        ABI_EXPORT_HANDLE_EVENT,
        ABI_EXPORT_RENDER_PAGE,
        ABI_EXPORT_RENDER_INJECTION,
        ABI_EXPORT_SHUTDOWN,
    ] {
        if !exports.iter().any(|export| export.name == required) {
            bail!("plugin missing required export {required}");
        }
    }

    Ok(())
}

fn is_supported_host_import(module: &str, name: &str) -> bool {
    name == ABI_IMPORT_NAME && (module == ABI_IMPORT_MODULE || module == ABI_LEGACY_IMPORT_MODULE)
}

fn plugin_memory_backend() -> MemoryBackend {
    MemoryBackend::custom(|memory_type| {
        let max_pages = plugin_memory_page_limit(&memory_type)?;
        let page_size = usize::try_from(memory_type.page_size()).map_err(|_| {
            tinywasm::Error::Other("plugin memory page size does not fit usize".into())
        })?;
        let max_len = usize::try_from(max_pages.saturating_mul(memory_type.page_size()))
            .map_err(|_| tinywasm::Error::Other("plugin memory limit does not fit usize".into()))?;
        let initial_len = usize::try_from(memory_type.initial_size()).map_err(|_| {
            tinywasm::Error::Other("plugin initial memory does not fit usize".into())
        })?;
        let inner = PagedMemory::try_new(initial_len, 64 * 1024).map_err(tinywasm::Error::Trap)?;
        Ok(LimitedPluginMemory {
            inner,
            max_len,
            page_size,
        })
    })
}

fn plugin_memory_page_limit(memory_type: &MemoryType) -> tinywasm::Result<u64> {
    let page_size = memory_type.page_size();
    if page_size == 0 {
        return Err(tinywasm::Error::Other(
            "plugin memory page size must be greater than zero".into(),
        ));
    }
    let host_page_limit = (PLUGIN_MEMORY_LIMIT_BYTES as u64) / page_size;
    if host_page_limit == 0 {
        return Err(tinywasm::Error::Other(
            "plugin memory page size exceeds host memory limit".into(),
        ));
    }
    Ok(memory_type.page_count_max().min(host_page_limit))
}

struct LimitedPluginMemory {
    inner: PagedMemory,
    max_len: usize,
    page_size: usize,
}

impl LinearMemory for LimitedPluginMemory {
    fn len(&self) -> usize {
        self.inner.len()
    }

    fn grow_to(&mut self, new_len: usize) -> std::result::Result<(), tinywasm::Trap> {
        if new_len > self.max_len || new_len % self.page_size != 0 {
            return Err(tinywasm::Trap::OutOfMemory);
        }
        self.inner.grow_to(new_len)
    }

    fn read(&self, addr: usize, dst: &mut [u8]) -> usize {
        self.inner.read(addr, dst)
    }

    fn write(&mut self, addr: usize, src: &[u8]) -> usize {
        self.inner.write(addr, src)
    }
}

fn host_imports(store: &mut Store, host_state: Rc<RefCell<HostState>>) -> Imports {
    let mut imports = Imports::new();
    let host_call = HostFunction::from_untyped(
        store,
        &tinywasm::types::FuncType::new(
            &[
                WasmType::I32,
                WasmType::I32,
                WasmType::I32,
                WasmType::I32,
                WasmType::I32,
            ],
            &[WasmType::I64],
        ),
        move |mut ctx: FuncContext<'_>, args| {
            let [
                WasmValue::I32(op),
                WasmValue::I32(req_ptr),
                WasmValue::I32(req_len),
                WasmValue::I32(resp_ptr),
                WasmValue::I32(resp_cap),
            ] = args
            else {
                return Err(tinywasm::Error::Other("invalid host-call signature".into()));
            };

            let memory = ctx.memory(ABI_EXPORT_MEMORY)?;
            let request = memory
                .read_vec(ctx.store(), *req_ptr as usize, *req_len as usize)
                .map_err(|error| {
                    tinywasm::Error::Other(format!("read host request failed: {error}"))
                })?;
            if request.len() > ABI_MESSAGE_MAX_BYTES {
                return Err(tinywasm::Error::Other(
                    "host request exceeds size limit".into(),
                ));
            }

            let request = postcard::from_bytes::<abi::HostRequest>(&request).map_err(|error| {
                tinywasm::Error::Other(format!("decode host request failed: {error}"))
            })?;
            let response = handle_host_request(&host_state, *op, request);
            let response_bytes = postcard::to_allocvec(&response).map_err(|error| {
                tinywasm::Error::Other(format!("encode host response failed: {error}"))
            })?;

            if response_bytes.len() > *resp_cap as usize {
                return Ok(vec![WasmValue::I64(-(response_bytes.len() as i64))]);
            }

            memory
                .write(ctx.store_mut(), *resp_ptr as usize, &response_bytes)
                .map_err(|error| {
                    tinywasm::Error::Other(format!("write host response failed: {error}"))
                })?;
            Ok(vec![WasmValue::I64(response_bytes.len() as i64)])
        },
    );
    imports.define(ABI_IMPORT_MODULE, ABI_IMPORT_NAME, host_call.clone());
    imports.define(ABI_LEGACY_IMPORT_MODULE, ABI_IMPORT_NAME, host_call);
    imports
}

fn handle_host_request(
    host_state: &Rc<RefCell<HostState>>,
    op: i32,
    request: abi::HostRequest,
) -> std::result::Result<abi::HostResponse, abi::HostError> {
    let mut state = host_state.borrow_mut();
    match (op, request) {
        (code, abi::HostRequest::Log { level, message }) if code == abi::HostOp::Log.code() => {
            let plugin_id = state.plugin_id.clone();
            state.effects.push(HostEffect::Log {
                plugin_id: plugin_id.clone(),
                level,
                message: message.clone(),
            });
            match level {
                abi::LogLevel::Debug => debug!(plugin_id, "{message}"),
                abi::LogLevel::Info => tracing::info!(plugin_id, "{message}"),
                abi::LogLevel::Warn => warn!(plugin_id, "{message}"),
                abi::LogLevel::Error => error!(plugin_id, "{message}"),
            }
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::ShowToast { kind, message })
            if code == abi::HostOp::ShowToast.code() =>
        {
            state.require_capability(PluginCapability::Toast)?;
            state.effects.push(HostEffect::Toast { kind, message });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::Navigate { target }) if code == abi::HostOp::Navigate.code() => {
            state.effects.push(HostEffect::Navigate { target });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::OpenWindow { request })
            if code == abi::HostOp::OpenWindow.code() =>
        {
            state.require_capability(PluginCapability::UiWindow)?;
            let plugin_id = state.plugin_id.clone();
            if request.plugin_id != plugin_id {
                return Err(abi::HostError {
                    code: "invalid-window-request".to_string(),
                    message: "plugin can only open its own windows".to_string(),
                });
            }

            let window_id = state.next_window_id;
            state.next_window_id = state.next_window_id.saturating_add(1);
            state.effects.push(HostEffect::OpenWindow { request });
            Ok(abi::HostResponse::WindowId(window_id))
        }
        (code, abi::HostRequest::OpenModal { request })
            if code == abi::HostOp::OpenModal.code() =>
        {
            state.require_capability(PluginCapability::UiPage)?;
            let plugin_id = state.plugin_id.clone();
            if request.plugin_id != plugin_id {
                return Err(abi::HostError {
                    code: "invalid-modal-request".to_string(),
                    message: "plugin can only open its own modal pages".to_string(),
                });
            }

            state.effects.push(HostEffect::OpenModal { request });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::CloseWindow { window_id })
            if code == abi::HostOp::CloseWindow.code() =>
        {
            state.require_capability(PluginCapability::UiWindow)?;
            state.effects.push(HostEffect::CloseWindow { window_id });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::EmitEvent { name, payload })
            if code == abi::HostOp::EmitEvent.code() =>
        {
            state.require_capability(PluginCapability::EventGlobal)?;
            state.effects.push(HostEffect::EmitEvent { name, payload });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::Invalidate { target })
            if code == abi::HostOp::Invalidate.code() =>
        {
            let plugin_id = state.plugin_id.clone();
            state
                .effects
                .push(HostEffect::Invalidate { plugin_id, target });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::CurrentLocale) if code == abi::HostOp::CurrentLocale.code() => {
            Ok(abi::HostResponse::String(state.locale.clone()))
        }
        (code, abi::HostRequest::Translate { key, args })
            if code == abi::HostOp::Translate.code() =>
        {
            Ok(abi::HostResponse::String(translate_plugin_key(
                &state.translations,
                &state.locale,
                &key,
                &args,
            )))
        }
        (code, abi::HostRequest::ReadConfig) if code == abi::HostOp::ReadConfig.code() => {
            state.require_capability(PluginCapability::ConfigRead)?;
            let config =
                crate::plugins::manifest::read_user_config(&state.manifest).map_err(|error| {
                    abi::HostError {
                        code: "config-read-failed".to_string(),
                        message: error.to_string(),
                    }
                })?;
            Ok(abi::HostResponse::String(config))
        }
        (
            code,
            abi::HostRequest::HttpGetText {
                url,
                ttl_seconds,
                max_bytes,
            },
        ) if code == abi::HostOp::HttpGetText.code() => {
            let snapshot = state.http_cache.snapshot(
                &state.manifest,
                &state.plugin_id,
                state.render_context.as_ref(),
                &url,
                ttl_seconds,
                max_bytes,
            )?;
            Ok(abi::HostResponse::HttpTextResponse {
                state: snapshot.state,
                body: snapshot.body,
                error: snapshot.error,
                fetched_at_unix_ms: snapshot.fetched_at_unix_ms,
            })
        }
        (code, abi::HostRequest::WriteClipboardText { text })
            if code == abi::HostOp::WriteClipboardText.code() =>
        {
            state.require_capability(PluginCapability::ClipboardWrite)?;
            state.effects.push(HostEffect::EmitEvent {
                name: "__plugin_write_clipboard".to_string(),
                payload: text,
            });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::ReadClipboardText)
            if code == abi::HostOp::ReadClipboardText.code() =>
        {
            state.require_capability(PluginCapability::ClipboardRead)?;
            Ok(abi::HostResponse::SessionValue(
                state.clipboard_text.clone(),
            ))
        }
        (code, abi::HostRequest::CurrentUnixMs) if code == abi::HostOp::CurrentUnixMs.code() => {
            Ok(abi::HostResponse::U64(current_unix_ms()))
        }
        (code, abi::HostRequest::OpenExternalUrl { url })
            if code == abi::HostOp::OpenExternalUrl.code() =>
        {
            state.require_capability(PluginCapability::ExternalOpen)?;
            if !url.starts_with("https://") {
                return Err(abi::HostError {
                    code: "invalid-url".to_string(),
                    message: "external URLs must use https".to_string(),
                });
            }
            if !state.manifest.allows_external_url(&url) {
                return Err(abi::HostError {
                    code: "external-url-denied".to_string(),
                    message: format!("plugin {} is not allowed to open {url}", state.plugin_id),
                });
            }
            state.effects.push(HostEffect::EmitEvent {
                name: "__plugin_open_external_url".to_string(),
                payload: url,
            });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::ReadResourceText { path })
            if code == abi::HostOp::ReadResourceText.code() =>
        {
            state.require_capability(PluginCapability::ResourceRead)?;
            let bytes = read_plugin_resource(&state.manifest, &path)?;
            let text = String::from_utf8(bytes).map_err(|error| abi::HostError {
                code: "resource-not-utf8".to_string(),
                message: format!("plugin resource {path} is not utf-8: {error}"),
            })?;
            Ok(abi::HostResponse::String(text))
        }
        (code, abi::HostRequest::ReadResourceBytes { path })
            if code == abi::HostOp::ReadResourceBytes.code() =>
        {
            state.require_capability(PluginCapability::ResourceRead)?;
            Ok(abi::HostResponse::Bytes(read_plugin_resource(
                &state.manifest,
                &path,
            )?))
        }
        (code, abi::HostRequest::SessionGet { key }) if code == abi::HostOp::SessionGet.code() => {
            Ok(abi::HostResponse::SessionValue(
                state.session.get(&key).cloned(),
            ))
        }
        (code, abi::HostRequest::SessionSet { key, value })
            if code == abi::HostOp::SessionSet.code() =>
        {
            match value {
                Some(value) => {
                    state.session.insert(key, value);
                }
                None => {
                    state.session.remove(&key);
                }
            }
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::StorageGet { key }) if code == abi::HostOp::StorageGet.code() => {
            state.require_capability(PluginCapability::StorageKv)?;
            Ok(abi::HostResponse::SessionValue(storage_get(
                &state.storage_dir,
                &key,
            )?))
        }
        (code, abi::HostRequest::StorageSet { key, value })
            if code == abi::HostOp::StorageSet.code() =>
        {
            state.require_capability(PluginCapability::StorageKv)?;
            storage_set(
                &state.storage_dir,
                &key,
                &value,
                state.manifest.limits.max_storage_bytes,
            )?;
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::StorageDelete { key })
            if code == abi::HostOp::StorageDelete.code() =>
        {
            state.require_capability(PluginCapability::StorageKv)?;
            storage_delete(&state.storage_dir, &key)?;
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::StorageList { prefix })
            if code == abi::HostOp::StorageList.code() =>
        {
            state.require_capability(PluginCapability::StorageKv)?;
            Ok(abi::HostResponse::StringList(storage_list(
                &state.storage_dir,
                prefix.as_deref(),
            )?))
        }
        (code, abi::HostRequest::WriteConfig { text })
            if code == abi::HostOp::WriteConfig.code() =>
        {
            state.require_capability(PluginCapability::ConfigWrite)?;
            crate::plugins::manifest::write_user_config(&state.manifest, &text).map_err(
                |error| abi::HostError {
                    code: "config-write-failed".to_string(),
                    message: error.to_string(),
                },
            )?;
            let plugin_id = state.plugin_id.clone();
            state.effects.push(HostEffect::Invalidate {
                plugin_id,
                target: abi::InvalidateTarget::All,
            });
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::CreateTask { request })
            if code == abi::HostOp::CreateTask.code() =>
        {
            state.require_capability(PluginCapability::TaskProgress)?;
            let task_id = crate::tasks::task_manager::create_task_with_details(
                request.task_id,
                request.title,
                request.detail,
                &request.stage,
                request.total,
                request.supports_pause,
            );
            Ok(abi::HostResponse::TaskId(task_id))
        }
        (code, abi::HostRequest::UpdateTask { request })
            if code == abi::HostOp::UpdateTask.code() =>
        {
            state.require_capability(PluginCapability::TaskProgress)?;
            crate::tasks::task_manager::update_progress(
                &request.task_id,
                request.done_delta,
                request.total,
                request.stage.as_deref(),
            );
            if request.message.is_some() {
                crate::tasks::task_manager::set_task_message(&request.task_id, request.message);
            }
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::FinishTask { request })
            if code == abi::HostOp::FinishTask.code() =>
        {
            state.require_capability(PluginCapability::TaskProgress)?;
            crate::tasks::task_manager::finish_task(
                &request.task_id,
                &request.status,
                request.message,
            );
            Ok(abi::HostResponse::Unit)
        }
        (code, abi::HostRequest::AppInfo) if code == abi::HostOp::AppInfo.code() => {
            Ok(abi::HostResponse::AppInfo(abi::AppInfo {
                version: crate::utils::app_info::get_version().to_string(),
                build_info: crate::utils::app_info::get_build_info(),
                api_version: abi::API_VERSION.to_string(),
            }))
        }
        (code, abi::HostRequest::ThemeSnapshot) if code == abi::HostOp::ThemeSnapshot.code() => {
            Ok(abi::HostResponse::ThemeSnapshot(state.theme_snapshot))
        }
        (_code, _request) => Err(abi::HostError {
            code: "invalid-host-operation".to_string(),
            message: "plugin issued an unsupported host operation".to_string(),
        }),
    }
}

fn manifest_has_any_readme(manifest: &PluginManifest) -> bool {
    manifest.readme_path().is_some_and(|path| path.exists())
        || manifest
            .readme_locales
            .values()
            .any(|path| manifest.root_dir.join(path).exists())
}

fn read_plugin_resource(
    manifest: &PluginManifest,
    path: &str,
) -> std::result::Result<Vec<u8>, abi::HostError> {
    let resource_path = manifest
        .resource_path(path)
        .map_err(|error| abi::HostError {
            code: "resource-denied".to_string(),
            message: error.to_string(),
        })?;
    let bytes = fs::read(&resource_path).map_err(|error| abi::HostError {
        code: "resource-read-failed".to_string(),
        message: format!(
            "read plugin resource {} failed: {error}",
            resource_path.display()
        ),
    })?;
    let max_bytes = usize::try_from(manifest.limits.max_resource_bytes).unwrap_or(usize::MAX);
    if bytes.len() > max_bytes {
        return Err(abi::HostError {
            code: "resource-too-large".to_string(),
            message: format!("plugin resource {path} exceeds {max_bytes} bytes"),
        });
    }
    Ok(bytes)
}

fn storage_get(
    storage_dir: &Path,
    key: &str,
) -> std::result::Result<Option<String>, abi::HostError> {
    let path = storage_path(storage_dir, key)?;
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(&path)
        .map(Some)
        .map_err(|error| abi::HostError {
            code: "storage-read-failed".to_string(),
            message: format!("read plugin storage key {key} failed: {error}"),
        })
}

fn storage_set(
    storage_dir: &Path,
    key: &str,
    value: &str,
    quota_bytes: u64,
) -> std::result::Result<(), abi::HostError> {
    let path = storage_path(storage_dir, key)?;
    let existing_len = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let used = storage_used_bytes(storage_dir)?;
    let next_used = used
        .saturating_sub(existing_len)
        .saturating_add(value.len() as u64);
    if next_used > quota_bytes {
        return Err(abi::HostError {
            code: "storage-quota-exceeded".to_string(),
            message: format!("plugin storage quota exceeded ({next_used}/{quota_bytes} bytes)"),
        });
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| abi::HostError {
            code: "storage-create-failed".to_string(),
            message: format!("create plugin storage {} failed: {error}", parent.display()),
        })?;
    }
    fs::write(&path, value).map_err(|error| abi::HostError {
        code: "storage-write-failed".to_string(),
        message: format!("write plugin storage key {key} failed: {error}"),
    })
}

fn storage_delete(storage_dir: &Path, key: &str) -> std::result::Result<(), abi::HostError> {
    let path = storage_path(storage_dir, key)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| abi::HostError {
            code: "storage-delete-failed".to_string(),
            message: format!("delete plugin storage key {key} failed: {error}"),
        })?;
    }
    Ok(())
}

fn storage_list(
    storage_dir: &Path,
    prefix: Option<&str>,
) -> std::result::Result<Vec<String>, abi::HostError> {
    if let Some(prefix) = prefix {
        validate_storage_key_prefix(prefix)?;
    }
    if !storage_dir.exists() {
        return Ok(Vec::new());
    }
    let mut keys = Vec::new();
    for entry in fs::read_dir(storage_dir).map_err(|error| abi::HostError {
        code: "storage-list-failed".to_string(),
        message: format!(
            "list plugin storage {} failed: {error}",
            storage_dir.display()
        ),
    })? {
        let entry = entry.map_err(|error| abi::HostError {
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

fn storage_path(storage_dir: &Path, key: &str) -> std::result::Result<PathBuf, abi::HostError> {
    validate_storage_key(key)?;
    Ok(storage_dir.join(storage_file_name(key)))
}

fn validate_storage_key_prefix(prefix: &str) -> std::result::Result<(), abi::HostError> {
    if prefix.len() > STORAGE_KEY_MAX_BYTES {
        return Err(abi::HostError {
            code: "invalid-storage-key".to_string(),
            message: format!("storage key prefix must be <= {STORAGE_KEY_MAX_BYTES} bytes"),
        });
    }
    if prefix.bytes().any(|byte| !is_storage_key_byte(byte)) {
        return Err(abi::HostError {
            code: "invalid-storage-key".to_string(),
            message: "storage key prefix contains unsupported characters".to_string(),
        });
    }
    Ok(())
}

fn validate_storage_key(key: &str) -> std::result::Result<(), abi::HostError> {
    if key.is_empty() || key.len() > STORAGE_KEY_MAX_BYTES {
        return Err(abi::HostError {
            code: "invalid-storage-key".to_string(),
            message: format!("storage key must be 1..={STORAGE_KEY_MAX_BYTES} bytes"),
        });
    }
    if key.bytes().any(|byte| !is_storage_key_byte(byte)) {
        return Err(abi::HostError {
            code: "invalid-storage-key".to_string(),
            message: "storage key contains unsupported characters".to_string(),
        });
    }
    Ok(())
}

fn is_storage_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_' | b':')
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

fn storage_used_bytes(storage_dir: &Path) -> std::result::Result<u64, abi::HostError> {
    if !storage_dir.exists() {
        return Ok(0);
    }
    let mut used = 0_u64;
    for entry in fs::read_dir(storage_dir).map_err(|error| abi::HostError {
        code: "storage-quota-check-failed".to_string(),
        message: format!(
            "read plugin storage {} failed: {error}",
            storage_dir.display()
        ),
    })? {
        let entry = entry.map_err(|error| abi::HostError {
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
    fn clear(&mut self) {
        self.pages.clear();
        self.page_errors.clear();
        self.injections.clear();
    }

    fn invalidate_plugin(&mut self, plugin_id: &str) {
        self.pages
            .retain(|key, _tree| key.plugin_id.as_str() != plugin_id);
        self.page_errors
            .retain(|key, _error| key.plugin_id.as_str() != plugin_id);
        for trees in self.injections.values_mut() {
            trees.retain(|tree| tree.plugin_id.as_str() != plugin_id);
        }
        self.injections.retain(|_key, trees| !trees.is_empty());
    }

    fn invalidate_page(&mut self, plugin_id: &str, page_id: Option<&str>) {
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
        self.injections.clear();
    }

    fn invalidate_all_injections(&mut self) {
        self.injections.clear();
    }

    fn invalidate_plugin_page(&mut self, plugin_id: &str, page_id: &str) {
        self.pages.retain(|key, _tree| {
            key.plugin_id.as_str() != plugin_id || key.page_id.as_str() != page_id
        });
        self.page_errors.retain(|key, _error| {
            key.plugin_id.as_str() != plugin_id || key.page_id.as_str() != page_id
        });
        self.invalidate_plugin_injections(plugin_id);
    }

    fn invalidate_plugin_injections(&mut self, plugin_id: &str) {
        for trees in self.injections.values_mut() {
            trees.retain(|tree| tree.plugin_id.as_str() != plugin_id);
        }
        self.injections.retain(|_key, trees| !trees.is_empty());
    }

    fn invalidate_plugin_injection(
        &mut self,
        plugin_id: &str,
        slot: InjectionSlot,
        page: Option<&str>,
    ) {
        self.injections.retain(|key, trees| {
            if key.slot != slot {
                return true;
            }
            if key.page.as_deref() != page {
                return true;
            }
            trees.retain(|tree| tree.plugin_id.as_str() != plugin_id);
            !trees.is_empty()
        });
    }

    fn plugin_memory(&self, plugin_id: &str) -> PluginRenderCacheMemory {
        let mut memory = PluginRenderCacheMemory::default();
        for (key, tree) in &self.pages {
            if key.plugin_id == plugin_id {
                memory.entries = memory.entries.saturating_add(1);
                memory.estimated_bytes = memory
                    .estimated_bytes
                    .saturating_add(tree.estimated_retained_bytes());
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
            for tree in trees {
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
struct PluginLogMemory {
    entries: usize,
    bytes: usize,
}

impl PluginHttpFetchCache {
    fn snapshot(
        &self,
        manifest: &PluginManifest,
        plugin_id: &str,
        render_context: Option<&RenderContext>,
        url: &str,
        ttl_seconds: u32,
        max_bytes: u32,
    ) -> std::result::Result<HttpCacheSnapshot, abi::HostError> {
        manifest
            .require_capability(PluginCapability::NetworkHttp)
            .map_err(|error| abi::HostError {
                code: "capability-denied".to_string(),
                message: error.to_string(),
            })?;
        if !manifest.allows_network_url(url) {
            return Err(abi::HostError {
                code: "network-url-denied".to_string(),
                message: format!("plugin {plugin_id} is not allowed to request {url}"),
            });
        }
        if !url.starts_with("https://") {
            return Err(abi::HostError {
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
            RenderContext::Page { page_id } => HttpInvalidationTarget::Page {
                plugin_id: plugin_id.to_string(),
                page_id: page_id.clone(),
            },
            RenderContext::Injection { slot, page } => HttpInvalidationTarget::Injection {
                plugin_id: plugin_id.to_string(),
                slot: *slot,
                page: page.clone(),
            },
        });

        let mut state = self.state.lock().map_err(|_| abi::HostError {
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
            spawn_http_refresh(url.to_string(), max_bytes, sender);
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

    fn drain_finished(&self) -> Vec<HttpInvalidationTarget> {
        let receiver = {
            let Ok(state) = self.state.lock() else {
                return Vec::new();
            };
            state.receiver.clone()
        };

        let mut results = Vec::new();
        let Ok(receiver) = receiver.lock() else {
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

fn spawn_http_refresh(url: String, max_bytes: usize, sender: mpsc::Sender<HttpRefreshResult>) {
    tokio::spawn(async move {
        let result = fetch_http_text(&url, max_bytes).await;
        if let Err(error) = sender.send(HttpRefreshResult { url, result }) {
            warn!(error = ?error, "plugin HTTP refresh result receiver dropped");
        }
        notify_http_refresh_finished();
    });
}

async fn fetch_http_text(
    url: &str,
    max_bytes: usize,
) -> std::result::Result<(String, u64), String> {
    let client = crate::http::proxy::get_client_for_proxy()
        .map_err(|error| format!("build HTTP client failed: {error}"))?;
    let response = client
        .get(url)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("HTTP request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP status {status}"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read HTTP body failed: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("HTTP body exceeds {max_bytes} bytes"));
    }
    let text = String::from_utf8(bytes.to_vec())
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
}

fn bootstrap_registration_from_abi(
    plugin_id: &str,
    registration: abi::Registration,
) -> Result<BootstrapRegistration> {
    match registration {
        abi::Registration::Page(page) => Ok(BootstrapRegistration::Page {
            page_id: page.page_id,
            title: page.title,
            navigation: page.navigation.map(|navigation| PluginNavigationEntry {
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

fn host_event_to_abi(event: &HostEvent) -> abi::HostEvent {
    abi::HostEvent {
        plugin_id: event.plugin_id.clone(),
        page_id: event.page_id.clone(),
        kind: match &event.kind {
            HostEventKind::RouteChanged { path } => {
                abi::HostEventKind::RouteChanged(abi::RouteChangedEvent { path: path.clone() })
            }
            HostEventKind::Action { action_id, value } => {
                abi::HostEventKind::Action(abi::ActionEvent {
                    action_id: action_id.clone(),
                    value: value.clone(),
                })
            }
            HostEventKind::Global { name, payload } => {
                abi::HostEventKind::Global(abi::GlobalEvent {
                    name: name.clone(),
                    payload: payload.clone(),
                })
            }
        },
    }
}

fn view_tree_from_abi(tree: abi::ViewTree) -> Result<ViewTree> {
    let root = node_from_abi_index(tree.root, &tree.nodes, 0)?;
    let view_tree = ViewTree { root };
    view_tree.validate()?;
    Ok(view_tree)
}

fn node_from_abi_index(
    index: u32,
    nodes: &[abi::ViewNode],
    depth: usize,
) -> Result<ui_dsl::ViewNode> {
    if depth > ui_dsl::MAX_VIEW_DEPTH {
        bail!("plugin view exceeds max depth {}", ui_dsl::MAX_VIEW_DEPTH);
    }

    let index = usize::try_from(index).context("plugin view node index overflow")?;
    let Some(node) = nodes.get(index) else {
        bail!("plugin view references missing node index {index}");
    };

    Ok(match node {
        abi::ViewNode::Container(container) => ui_dsl::ViewNode::Container {
            style: view_style_from_abi(container.style),
            children: children_from_abi(&container.children, nodes, depth)?,
        },
        abi::ViewNode::Text(text) => ui_dsl::ViewNode::Text {
            text: text.text.clone(),
            style: view_style_from_abi(text.style),
        },
        abi::ViewNode::Button(button) => ui_dsl::ViewNode::Button {
            label: button.label.clone(),
            action_id: button.action_id.clone(),
            action_value: button.action_value.clone(),
            style: view_style_from_abi(button.style),
        },
        abi::ViewNode::Input(input) => ui_dsl::ViewNode::Input {
            value: input.value.clone(),
            placeholder: input.placeholder.clone(),
            action_id: input.action_id.clone(),
            style: view_style_from_abi(input.style),
        },
        abi::ViewNode::Checkbox(checkbox) => ui_dsl::ViewNode::Checkbox {
            label: checkbox.label.clone(),
            checked: checkbox.checked,
            action_id: checkbox.action_id.clone(),
            action_value: checkbox.action_value.clone(),
            style: view_style_from_abi(checkbox.style),
        },
        abi::ViewNode::Toggle(toggle) => ui_dsl::ViewNode::Toggle {
            label: toggle.label.clone(),
            enabled: toggle.enabled,
            action_id: toggle.action_id.clone(),
            action_value: toggle.action_value.clone(),
            style: view_style_from_abi(toggle.style),
        },
        abi::ViewNode::Select(select) => ui_dsl::ViewNode::Select {
            label: select.label.clone(),
            action_id: select.action_id.clone(),
            options: select
                .options
                .iter()
                .map(|option| ui_dsl::SelectOption {
                    label: option.label.clone(),
                    value: option.value.clone(),
                })
                .collect(),
            selected: select.selected.clone(),
            style: view_style_from_abi(select.style),
        },
        abi::ViewNode::Progress(progress) => ui_dsl::ViewNode::Progress {
            label: progress.label.clone(),
            value: progress.value,
            total: progress.total,
            style: view_style_from_abi(progress.style),
        },
        abi::ViewNode::Link(link) => ui_dsl::ViewNode::Link {
            label: link.label.clone(),
            url: link.url.clone(),
            tooltip: link.tooltip.clone(),
            style: view_style_from_abi(link.style),
        },
        abi::ViewNode::ItemList(list) => ui_dsl::ViewNode::List {
            items: children_from_abi(&list.items, nodes, depth)?,
            style: view_style_from_abi(list.style),
        },
        abi::ViewNode::Separator => ui_dsl::ViewNode::Separator,
        abi::ViewNode::Badge(badge) => ui_dsl::ViewNode::Badge {
            label: badge.label.clone(),
            style: view_style_from_abi(badge.style),
        },
        abi::ViewNode::Icon(icon) => ui_dsl::ViewNode::Icon {
            name: icon.name.clone(),
            style: view_style_from_abi(icon.style),
        },
        abi::ViewNode::Image(image) => ui_dsl::ViewNode::Image {
            src: image.src.clone(),
            alt: image.alt.clone(),
            caption: image.caption.clone(),
            placeholder: image.placeholder.clone(),
            fallback: image.fallback.clone(),
            style: view_style_from_abi(image.style),
            height: image.height,
            min_height: image.min_height,
            max_height: image.max_height,
            aspect_ratio_x: image.aspect_ratio_x,
            aspect_ratio_y: image.aspect_ratio_y,
            corner_radius: image.corner_radius,
            fit: match image.fit {
                abi::ImageFit::Cover => ui_dsl::ImageFit::Cover,
                abi::ImageFit::Contain => ui_dsl::ImageFit::Contain,
            },
        },
        abi::ViewNode::Spacer(spacer) => ui_dsl::ViewNode::Spacer { size: spacer.size },
    })
}

fn children_from_abi(
    children: &[u32],
    nodes: &[abi::ViewNode],
    depth: usize,
) -> Result<Vec<ui_dsl::ViewNode>> {
    children
        .iter()
        .copied()
        .map(|child| node_from_abi_index(child, nodes, depth + 1))
        .collect()
}

fn view_style_from_abi(style: abi::ViewStyle) -> ui_dsl::ViewStyle {
    ui_dsl::ViewStyle {
        direction: match style.direction {
            abi::LayoutDirection::Row => ui_dsl::LayoutDirection::Row,
            abi::LayoutDirection::Column => ui_dsl::LayoutDirection::Column,
        },
        gap: style.gap,
        padding: style.padding,
        align: match style.align {
            abi::Align::Start => ui_dsl::Align::Start,
            abi::Align::Center => ui_dsl::Align::Center,
            abi::Align::End => ui_dsl::Align::End,
            abi::Align::Stretch => ui_dsl::Align::Stretch,
        },
        color: style.color.map(theme_token_from_abi),
        background: style.background.map(theme_token_from_abi),
        text_size: match style.text_size {
            abi::TextSizeToken::Small => ui_dsl::TextSizeToken::Small,
            abi::TextSizeToken::Body => ui_dsl::TextSizeToken::Body,
            abi::TextSizeToken::Title => ui_dsl::TextSizeToken::Title,
        },
        emphasis: style.emphasis,
        full_width: style.full_width,
        corner_radius: style.corner_radius,
    }
}

fn theme_token_from_abi(token: abi::ThemeToken) -> ui_dsl::ThemeToken {
    match token {
        abi::ThemeToken::PrimaryText => ui_dsl::ThemeToken::PrimaryText,
        abi::ThemeToken::SecondaryText => ui_dsl::ThemeToken::SecondaryText,
        abi::ThemeToken::MutedText => ui_dsl::ThemeToken::MutedText,
        abi::ThemeToken::Accent => ui_dsl::ThemeToken::Accent,
        abi::ThemeToken::Surface => ui_dsl::ThemeToken::Surface,
        abi::ThemeToken::Border => ui_dsl::ThemeToken::Border,
        abi::ThemeToken::Danger => ui_dsl::ThemeToken::Danger,
    }
}

pub fn init(cx: &mut App) {
    cx.default_global::<PluginRegistry>();
    reload_all(cx);
    start_watcher(cx);
}

pub fn reload_all(cx: &mut App) {
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        if let Err(error) = registry.reload_all() {
            let error_message = crate::plugins::manifest::format_error_chain(&error);
            error!(error = %error_message, "plugin reload failed");
            registry.last_error = Some(SharedString::from(error_message));
        } else {
            registry.set_theme_snapshot(theme_snapshot);
        }
    });
}

pub fn ensure_manifest_index(cx: &mut App) {
    if !cx.global::<PluginRegistry>().loaded_once() {
        reload_all(cx);
    }
    start_watcher(cx);
}

pub fn ensure_loaded(cx: &mut App) {
    ensure_manifest_index(cx);
}

pub fn start_watcher(cx: &mut App) {
    if cx.global::<PluginRegistry>().reload_tx.is_some() {
        return;
    }

    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();
    match crate::plugins::watcher::spawn_plugin_watcher(plugins_dir, cx) {
        Ok((sender, task)) => {
            set_http_refresh_sender(sender.clone());
            cx.update_global(|registry: &mut PluginRegistry, _cx| {
                registry.set_watcher(sender, task);
            });
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

fn set_http_refresh_sender(sender: crate::plugins::watcher::PluginWatcherSender) {
    let slot = HTTP_REFRESH_NOTIFICATION.get_or_init(|| Mutex::new(None));
    if let Ok(mut current) = slot.lock() {
        *current = Some(sender);
    }
}

fn notify_http_refresh_finished() {
    let Some(slot) = HTTP_REFRESH_NOTIFICATION.get() else {
        return;
    };
    let Ok(current) = slot.lock() else {
        return;
    };
    let Some(sender) = current.as_ref() else {
        return;
    };
    let _ = sender.unbounded_send(crate::plugins::watcher::PluginWatcherMessage::HttpRefresh);
}

pub fn render_page(cx: &mut App, plugin_id: &str, page_id: &str) -> Result<Arc<ViewTree>> {
    ensure_loaded(cx);
    drain_http_refreshes(cx);
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);

    let registry = cx.global::<PluginRegistry>();
    if let Some(tree) = registry.cached_page(plugin_id, page_id) {
        return Ok(tree);
    }
    if let Some(error) = registry.cached_page_error(plugin_id, page_id) {
        return Err(anyhow!("{}", error));
    }

    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.render_page(plugin_id, page_id)
    })
}

pub fn render_injections(
    cx: &mut App,
    slot: InjectionSlot,
    page: Option<&str>,
) -> Vec<RenderedInjection> {
    ensure_loaded(cx);
    drain_http_refreshes(cx);
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);

    if let Some(trees) = cx.global::<PluginRegistry>().cached_injections(slot, page) {
        return trees;
    }

    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.render_injections(slot, page)
    })
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

pub(crate) fn drain_http_refreshes(cx: &mut App) -> bool {
    cx.update_global(|registry: &mut PluginRegistry, _cx| registry.apply_http_refreshes())
}

pub fn has_injections(cx: &App, slot: InjectionSlot, page: Option<&str>) -> bool {
    cx.global::<PluginRegistry>().has_injections(slot, page)
}

pub fn navigation_pages(cx: &App) -> Vec<PluginPage> {
    cx.global::<PluginRegistry>().navigation_pages()
}

pub fn statuses(cx: &App) -> Vec<PluginStatus> {
    cx.global::<PluginRegistry>().statuses()
}

pub fn plugin_readme(cx: &App, plugin_id: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .plugin_readme(plugin_id)
        .ok()
        .flatten()
}

pub fn plugin_readme_for_locale(cx: &App, plugin_id: &str, locale: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .plugin_readme_for_locale(plugin_id, locale)
        .ok()
        .flatten()
}

pub fn plugin_config_text(cx: &App, plugin_id: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .plugin_config_text(plugin_id)
        .ok()
        .flatten()
}

pub fn plugin_config_schema(cx: &App, plugin_id: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .plugin_config_schema(plugin_id)
        .ok()
        .flatten()
}

pub fn plugin_logs(cx: &App, plugin_id: &str) -> Vec<PluginLogEntry> {
    cx.global::<PluginRegistry>().plugin_logs(plugin_id)
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
    cx.update_global(|registry: &mut PluginRegistry, _cx| registry.open_modal(request))?;
    cx.refresh_windows();
    Ok(())
}

pub fn translate_plugin_resource(cx: &App, plugin_id: &str, key: &str) -> Option<String> {
    cx.global::<PluginRegistry>()
        .translate_plugin_resource(plugin_id, key)
}

pub fn translate_plugin_resource_for_locale(
    cx: &App,
    plugin_id: &str,
    locale: &str,
    key: &str,
) -> Option<String> {
    cx.global::<PluginRegistry>()
        .translate_plugin_resource_for_locale(plugin_id, locale, key)
}

pub fn save_plugin_config(cx: &mut App, plugin_id: String, content: String) -> Result<()> {
    let effects = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.write_plugin_config(&plugin_id, &content)
    })?;
    apply_host_effects(cx, effects);
    Ok(())
}

pub fn set_plugin_enabled(cx: &mut App, plugin_id: String, enabled: bool) -> Result<()> {
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.set_plugin_enabled(&plugin_id, enabled)
    })?;
    cx.refresh_windows();
    Ok(())
}

pub fn uninstall_plugin(cx: &mut App, plugin_id: String) -> Result<()> {
    cx.update_global(|registry: &mut PluginRegistry, _cx| registry.uninstall_plugin(&plugin_id))?;
    cx.refresh_windows();
    Ok(())
}

pub fn reload_plugin(cx: &mut App, plugin_id: String) -> Result<()> {
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.reload_plugin(&plugin_id)
    })?;
    cx.refresh_windows();
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

pub fn import_plugin_package(cx: &mut App, source_path: impl AsRef<Path>) -> Result<()> {
    ensure_loaded(cx);

    let source_path = source_path.as_ref();
    let extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case(crate::plugins::manifest::PLUGIN_PACKAGE_EXTENSION) {
        bail!(
            "plugin package must use .{} extension",
            crate::plugins::manifest::PLUGIN_PACKAGE_EXTENSION
        );
    }

    let plugins_dir = cx.global::<PluginRegistry>().plugins_dir().to_path_buf();
    std::fs::create_dir_all(&plugins_dir)
        .with_context(|| format!("create plugin directory {}", plugins_dir.display()))?;
    let file_name = source_path.file_name().ok_or_else(|| {
        anyhow!(
            "plugin package path has no file name: {}",
            source_path.display()
        )
    })?;
    let destination = plugins_dir.join(file_name);
    let source = std::fs::canonicalize(source_path)
        .with_context(|| format!("canonicalize plugin package {}", source_path.display()))?;
    let destination_matches_source = destination
        .exists()
        .then(|| std::fs::canonicalize(&destination))
        .transpose()
        .with_context(|| format!("canonicalize plugin package {}", destination.display()))?
        .is_some_and(|destination| destination == source);
    if !destination_matches_source {
        std::fs::copy(source_path, &destination).with_context(|| {
            format!(
                "copy plugin package {} to {}",
                source_path.display(),
                destination.display()
            )
        })?;
    }

    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        let result = registry.reload_all();
        if result.is_ok() {
            registry.set_theme_snapshot(theme_snapshot);
        }
        result
    })?;
    Ok(())
}

pub fn dispatch_plugin_action(
    cx: &mut App,
    plugin_id: String,
    page_id: Option<String>,
    action_id: String,
    value: Option<String>,
) {
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    let effects = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.handle_event(HostEvent {
            plugin_id: Some(plugin_id),
            page_id,
            kind: HostEventKind::Action { action_id, value },
        })
    });
    apply_host_effects(cx, effects);
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
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    let effects = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.handle_event(HostEvent {
            plugin_id: None,
            page_id: None,
            kind: HostEventKind::Global { name, payload },
        })
    });
    apply_host_effects(cx, effects);
}

pub(crate) fn dispatch_route_changed(cx: &mut App, path: String) {
    let theme_snapshot = current_theme_snapshot(cx);
    let clipboard_text = clipboard_text_snapshot(cx);
    let effects = cx.update_global(|registry: &mut PluginRegistry, _cx| {
        registry.set_theme_snapshot(theme_snapshot);
        registry.set_clipboard_snapshot(clipboard_text);
        registry.handle_event(HostEvent {
            plugin_id: None,
            page_id: None,
            kind: HostEventKind::RouteChanged { path },
        })
    });
    apply_host_effects(cx, effects);
}

pub(crate) fn apply_host_effects(cx: &mut App, effects: Vec<HostEffect>) {
    let mut pending_effects = Vec::new();
    for effect in effects {
        match effect {
            HostEffect::Toast { kind, message } => {
                let kind = match kind {
                    abi::ToastKind::Info => crate::ui::components::toast::ToastKind::Info,
                    abi::ToastKind::Success => crate::ui::components::toast::ToastKind::Success,
                    abi::ToastKind::Error => crate::ui::components::toast::ToastKind::Error,
                };
                crate::ui::components::toast::push_kind(cx, kind, SharedString::from(message));
            }
            HostEffect::Navigate { target } => {
                let target = route_target_from_abi(target);
                crate::ui::navigation::navigate_target(cx, target);
            }
            HostEffect::OpenWindow { request } => {
                let title = request.title.clone();
                if let Err(error) = crate::plugins::window::open_plugin_window(
                    cx,
                    request.plugin_id.clone(),
                    request.page_id.clone(),
                    title,
                ) {
                    warn!(error = ?error, "plugin open-window effect failed");
                    if let Err(error) = show_toast(
                        cx,
                        &request.plugin_id,
                        "Unable to open plugin window".to_string(),
                    ) {
                        warn!(error = ?error, "plugin open-window fallback toast failed");
                    }
                }
            }
            HostEffect::OpenModal { request } => {
                if let Err(error) = open_modal(cx, request.clone()) {
                    let error = error.to_string();
                    warn!(
                        plugin_id = %request.plugin_id,
                        error = %error,
                        "plugin open-modal effect failed"
                    );
                    if let Err(error) = show_toast(
                        cx,
                        &request.plugin_id,
                        "Unable to open plugin modal".to_string(),
                    ) {
                        warn!(
                            error = %error,
                            "plugin open-modal fallback toast failed"
                        );
                    }
                }
            }
            HostEffect::CloseWindow { window_id } => {
                warn!(
                    window_id,
                    "plugin close-window effect requested; close by id is not wired yet"
                );
            }
            HostEffect::EmitEvent { name, payload } => {
                if name == "__plugin_write_clipboard" {
                    cx.write_to_clipboard(ClipboardItem::new_string(payload));
                    continue;
                }
                if name == "__plugin_open_external_url" {
                    cx.open_url(&payload);
                    continue;
                }
                let mut effects = cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry.handle_event(HostEvent {
                        plugin_id: None,
                        page_id: None,
                        kind: HostEventKind::Global { name, payload },
                    })
                });
                pending_effects.append(&mut effects);
            }
            HostEffect::Invalidate { plugin_id, target } => {
                cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry.invalidate_from_plugin(&plugin_id, target);
                });
            }
            HostEffect::Log {
                plugin_id,
                level,
                message,
            } => {
                cx.update_global(|registry: &mut PluginRegistry, _cx| {
                    registry.push_log(plugin_id, level, message);
                });
            }
        }
    }

    if !pending_effects.is_empty() {
        apply_host_effects(cx, pending_effects);
    }
}

fn current_locale_code() -> String {
    crate::config::config::read_config()
        .ok()
        .map(|config| config.launcher.language)
        .filter(|language| !language.trim().eq_ignore_ascii_case("auto"))
        .unwrap_or_else(crate::utils::system_info::get_system_language)
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
    fn storage_rejects_quota_overflow() {
        let root = unique_temp_dir("bmcbl-plugin-storage-quota");
        storage_set(&root, "first", "12345", 8).expect("first write should fit");
        let error =
            storage_set(&root, "second", "12345", 8).expect_err("second write should exceed quota");

        assert_eq!(error.code, "storage-quota-exceeded");
    }

    #[test]
    fn storage_lists_keys_with_prefix() {
        let root = unique_temp_dir("bmcbl-plugin-storage-list");
        storage_set(&root, "note:one", "1", 128).expect("write should succeed");
        storage_set(&root, "note:two", "2", 128).expect("write should succeed");
        storage_set(&root, "other", "3", 128).expect("write should succeed");

        let keys = storage_list(&root, Some("note:")).expect("list should succeed");

        assert_eq!(keys, vec!["note:one".to_string(), "note:two".to_string()]);
    }

    #[test]
    fn resource_read_respects_manifest_allowlist() {
        let root = unique_temp_dir("bmcbl-plugin-resource");
        fs::create_dir_all(root.join("assets")).expect("assets dir should be created");
        fs::write(root.join("assets/readme.txt"), "hello").expect("resource should be written");
        let manifest = PluginManifest::parse(
            &root,
            &format!(
                r#"
schema_version = 2
id = "hello-plugin"
name = "Hello"
version = "0.1.0"
api_version = "{}"
entry = "plugin.wasm"
capabilities = ["resource.read"]

[permissions.resource]
allow = ["assets/"]

[limits]
max_resource_bytes = 16
"#,
                crate::plugins::manifest::CURRENT_API_VERSION
            ),
        )
        .expect("manifest should parse");

        let bytes =
            read_plugin_resource(&manifest, "assets/readme.txt").expect("resource should read");
        assert_eq!(bytes, b"hello");

        let error = read_plugin_resource(&manifest, "plugin.toml")
            .expect_err("unlisted resource should be denied");
        assert_eq!(error.code, "resource-denied");
    }

    #[test]
    fn memory_report_empty_registry_is_stable() {
        let root = unique_temp_dir("bmcbl-plugin-memory-report");
        let registry = PluginRegistry::new(
            root.join("plugins"),
            root.join("cache"),
            root.join("packages"),
        );

        let report = registry.memory_report();

        assert!(report.plugins.is_empty());
        assert_eq!(
            report.total_estimated_bytes,
            report.module_cache_estimated_bytes
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }
}

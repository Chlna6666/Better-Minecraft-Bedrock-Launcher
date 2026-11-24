//! 插件执行体：Store 状态、宿主接口实现（capability 门禁在此）、入口调用编排。

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use wasmtime::{Store, Trap};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::bindings::PluginWorld;
use crate::bindings::bmcbl::plugin::host_clipboard::Host as HostClipboard;
use crate::bindings::bmcbl::plugin::host_config::Host as HostConfig;
use crate::bindings::bmcbl::plugin::host_downloads::Host as HostDownloads;
use crate::bindings::bmcbl::plugin::host_events::Host as HostEvents;
use crate::bindings::bmcbl::plugin::host_log::Host as HostLog;
use crate::bindings::bmcbl::plugin::host_network::Host as HostNetwork;
use crate::bindings::bmcbl::plugin::host_resources::Host as HostResources;
use crate::bindings::bmcbl::plugin::host_services::Host as HostServices;
use crate::bindings::bmcbl::plugin::host_storage::Host as HostStorage;
use crate::bindings::bmcbl::plugin::host_system::Host as HostSystem;
use crate::bindings::bmcbl::plugin::host_tasks::Host as HostTasks;
use crate::bindings::bmcbl::plugin::host_toast::Host as HostToast;
use crate::bindings::bmcbl::plugin::host_ui::Host as HostUi;
use crate::bindings::bmcbl::plugin::types::{
    AppInfo, DownloadRequest, HttpTextResponse, I18nArg, InjectionRequest, InvalidateTarget,
    LogLevel, ModalRequest, PageRenderRequest, PluginContext, PluginError, RouteTarget,
    ServiceCallRequest, ShutdownReason, TaskCreateRequest, TaskDefinitionRequest, TaskFinishRequest,
    TaskUpdateRequest, ThemeColor, ThemeSnapshot, ToastKind, ViewTree, WindowRequest,
};
use crate::context::{PluginCallInfo, PluginEffect, PluginHostContext, PluginInitState, PluginRenderContext};
use crate::engine::{ComponentArtifact, EntryTimeout, PluginEngine};
use crate::policy::PluginPolicy;
use crate::sandbox::{
    PluginResourceLimiter, storage_snapshot_next_used_bytes, validate_storage_key,
    validate_storage_key_prefix,
};

/// 捕获插件 stderr 的环形上限；溢出内容被丢弃（std panic 输出足够）。
const STDERR_CAPTURE_BYTES: usize = 64 * 1024;

/// Store 数据：策略 + 运行时托管的纯状态 + 宿主服务桥。
pub struct PluginState {
    pub(crate) policy: PluginPolicy,
    pub(crate) limiter: PluginResourceLimiter,
    wasi_table: ResourceTable,
    wasi_ctx: wasmtime_wasi::WasiCtx,
    pub(crate) context: Arc<dyn PluginHostContext>,
    locale: String,
    translations: BTreeMap<String, BTreeMap<String, String>>,
    theme: ThemeSnapshot,
    clipboard_text: Option<String>,
    session: BTreeMap<String, String>,
    storage_values: Result<BTreeMap<String, String>, String>,
    config_text: Result<String, String>,
    pub(crate) effects: Vec<PluginEffect>,
    next_window_id: u64,
    render_context: Option<PluginRenderContext>,
    stderr_pipe: MemoryOutputPipe,
    stderr_reported: usize,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.wasi_table,
        }
    }
}

impl PluginState {
    fn info(&self) -> PluginCallInfo<'_> {
        PluginCallInfo {
            policy: &self.policy,
            render_context: self.render_context.clone(),
        }
    }

    pub(crate) fn require_render_safe(&self, operation: &'static str) -> Result<(), PluginError> {
        if self.render_context.is_none() {
            return Ok(());
        }
        Err(PluginError::new(
            "blocking-io-render-denied",
            format!(
                "{operation} is not allowed while rendering plugin UI; load or persist data during init/event handling and keep render state in memory"
            ),
        ))
    }

    fn push_log(&mut self, level: LogLevel, message: String) {
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::Log {
            plugin_id: plugin_id.clone(),
            level,
            message: message.clone(),
        });
        match level {
            LogLevel::Debug => tracing::debug!(plugin_id = %plugin_id, message = %message, "plugin log"),
            LogLevel::Info => tracing::info!(plugin_id = %plugin_id, message = %message, "plugin log"),
            LogLevel::Warn => tracing::warn!(plugin_id = %plugin_id, message = %message, "plugin log"),
            LogLevel::Error => tracing::error!(plugin_id = %plugin_id, message = %message, "plugin log"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 宿主接口实现（capability 门禁）
// ─────────────────────────────────────────────────────────────────────────────

impl HostLog for PluginState {
    fn log(&mut self, level: LogLevel, message: String) {
        self.push_log(level, message);
    }
}

impl HostToast for PluginState {
    fn show_toast(&mut self, kind: ToastKind, message: String) -> Result<(), PluginError> {
        self.policy.require(self.policy.capabilities.toast, "show-toast")?;
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::Toast {
            plugin_id,
            kind,
            message,
        });
        Ok(())
    }
}

impl HostUi for PluginState {
    fn navigate(&mut self, target: RouteTarget) -> Result<(), PluginError> {
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::Navigate { plugin_id, target });
        Ok(())
    }

    fn open_window(&mut self, request: WindowRequest) -> Result<u64, PluginError> {
        self.policy.require(self.policy.capabilities.ui_window, "open-window")?;
        if request.plugin_id != self.policy.plugin_id {
            return Err(PluginError::invalid_input(
                "plugin can only open its own windows",
            ));
        }
        let window_id = self.next_window_id;
        self.next_window_id = self.next_window_id.saturating_add(1);
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::OpenWindow {
            plugin_id,
            request,
            window_id,
        });
        Ok(window_id)
    }

    fn close_window(&mut self, window_id: u64) -> Result<(), PluginError> {
        self.policy.require(self.policy.capabilities.ui_window, "close-window")?;
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::CloseWindow { plugin_id, window_id });
        Ok(())
    }

    fn open_modal(&mut self, request: ModalRequest) -> Result<(), PluginError> {
        self.policy.require(self.policy.capabilities.ui_page, "open-modal")?;
        if request.plugin_id != self.policy.plugin_id {
            return Err(PluginError::invalid_input(
                "plugin can only open its own modal pages",
            ));
        }
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::OpenModal { plugin_id, request });
        Ok(())
    }

    fn get_theme_snapshot(&mut self) -> ThemeSnapshot {
        self.theme
    }

    fn theme_accent(&mut self) -> Option<ThemeColor> {
        Some(self.theme.accent)
    }

    fn current_locale(&mut self) -> String {
        self.locale.clone()
    }

    fn translate(&mut self, key: String, args: Vec<I18nArg>) -> String {
        translate_plugin_key(&self.translations, &self.locale, &key, &args)
    }
}

impl HostEvents for PluginState {
    fn emit_event(&mut self, name: String, payload: String) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.event_global, "emit-event")?;
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::EmitEvent {
            plugin_id,
            name,
            payload,
        });
        Ok(())
    }

    fn invalidate(&mut self, target: InvalidateTarget) -> Result<(), PluginError> {
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::Invalidate { plugin_id, target });
        Ok(())
    }
}

impl HostClipboard for PluginState {
    fn read_text(&mut self) -> Result<Option<String>, PluginError> {
        self.policy
            .require(self.policy.capabilities.clipboard_read, "clipboard-read")?;
        Ok(self.clipboard_text.clone())
    }

    fn write_text(&mut self, text: String) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.clipboard_write, "clipboard-write")?;
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::WriteClipboard { plugin_id, text });
        Ok(())
    }
}

impl HostNetwork for PluginState {
    fn get_text(
        &mut self,
        url: String,
        ttl_seconds: u32,
        max_bytes: u32,
    ) -> Result<HttpTextResponse, PluginError> {
        self.policy
            .require(self.policy.capabilities.network_http, "http-get-text")?;
        if !self.policy.network_allow.allows_url(&url) {
            return Err(PluginError::denied(format!(
                "plugin {} is not allowed to fetch {url}",
                self.policy.plugin_id
            )));
        }
        self.context.http_get_text(self.info(), &url, ttl_seconds, max_bytes)
    }
}

impl HostConfig for PluginState {
    fn read(&mut self) -> Result<String, PluginError> {
        self.policy
            .require(self.policy.capabilities.config_read, "config-read")?;
        self.config_text
            .clone()
            .map_err(|error| PluginError::app(error))
    }

    fn write(&mut self, text: String) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.config_write, "config-write")?;
        self.require_render_safe("config write")?;
        self.context.write_config(self.info(), &text)?;
        self.config_text = Ok(text.clone());
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::PersistConfig { plugin_id, text });
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::Invalidate {
            plugin_id,
            target: InvalidateTarget::All,
        });
        Ok(())
    }
}

impl HostStorage for PluginState {
    fn session_get(&mut self, key: String) -> Option<String> {
        self.session.get(&key).cloned()
    }

    fn session_set(&mut self, key: String, value: Option<String>) {
        match value {
            Some(value) => {
                self.session.insert(key, value);
            }
            None => {
                self.session.remove(&key);
            }
        }
    }

    fn kv_get(&mut self, key: String) -> Result<Option<String>, PluginError> {
        self.policy.require(self.policy.capabilities.storage_kv, "storage-get")?;
        validate_storage_key(&key)?;
        match &self.storage_values {
            Ok(values) => Ok(values.get(&key).cloned()),
            Err(error) => Err(PluginError::app(error.clone())),
        }
    }

    fn kv_set(&mut self, key: String, value: String) -> Result<(), PluginError> {
        self.policy.require(self.policy.capabilities.storage_kv, "storage-set")?;
        self.require_render_safe("storage set")?;
        validate_storage_key(&key)?;
        let quota_bytes = self.policy.limits.max_storage_bytes;
        let snapshot = self.storage_values.as_ref().map_err(|error| {
            PluginError::new("storage-state-unavailable", error.clone())
        })?;
        let next_used = storage_snapshot_next_used_bytes(snapshot, &key, &value);
        if next_used > quota_bytes {
            return Err(PluginError::new(
                "storage-quota-exceeded",
                format!("plugin storage quota exceeded ({next_used}/{quota_bytes} bytes)"),
            ));
        }
        if let Ok(values) = &mut self.storage_values {
            values.insert(key.clone(), value.clone());
        }
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::PersistStorage {
            plugin_id,
            key,
            value,
        });
        Ok(())
    }

    fn kv_delete(&mut self, key: String) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.storage_kv, "storage-delete")?;
        self.require_render_safe("storage delete")?;
        validate_storage_key(&key)?;
        let snapshot = self.storage_values.as_ref().map_err(|error| {
            PluginError::new("storage-state-unavailable", error.clone())
        })?;
        if !snapshot.contains_key(&key) {
            return Ok(());
        }
        if let Ok(values) = &mut self.storage_values {
            values.remove(&key);
        }
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::DeleteStorage { plugin_id, key });
        Ok(())
    }

    fn kv_list(&mut self, prefix: Option<String>) -> Result<Vec<String>, PluginError> {
        self.policy
            .require(self.policy.capabilities.storage_kv, "storage-list")?;
        if let Some(prefix) = prefix.as_deref() {
            validate_storage_key_prefix(prefix)?;
        }
        match &self.storage_values {
            Ok(values) => Ok(values
                .keys()
                .filter(|key| {
                    prefix
                        .as_deref()
                        .is_none_or(|prefix| key.starts_with(prefix))
                })
                .cloned()
                .collect()),
            Err(error) => Err(PluginError::app(error.clone())),
        }
    }
}

impl HostResources for PluginState {
    fn read_text(&mut self, path: String) -> Result<String, PluginError> {
        self.policy
            .require(self.policy.capabilities.resource_read, "resource-read")?;
        self.context.read_resource_text(self.info(), &path)
    }

    fn read_bytes(&mut self, path: String) -> Result<Vec<u8>, PluginError> {
        self.policy
            .require(self.policy.capabilities.resource_read, "resource-read")?;
        self.context.read_resource_bytes(self.info(), &path)
    }
}

impl HostSystem for PluginState {
    fn current_unix_ms(&mut self) -> u64 {
        unix_now_ms()
    }

    fn get_app_info(&mut self) -> AppInfo {
        self.context.app_info()
    }

    fn open_external_url(&mut self, url: String) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.external_open, "open-external-url")?;
        if !url.starts_with("https://") {
            return Err(PluginError::invalid_input(
                "external URLs must use https",
            ));
        }
        if !self.policy.external_allow.allows_url(&url) {
            return Err(PluginError::denied(format!(
                "plugin {} is not allowed to open {url}",
                self.policy.plugin_id
            )));
        }
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::OpenExternalUrl { plugin_id, url });
        Ok(())
    }
}

impl HostTasks for PluginState {
    fn create(&mut self, request: TaskCreateRequest) -> Result<String, PluginError> {
        self.policy
            .require(self.policy.capabilities.task_progress, "task-create")?;
        self.context.task_create(self.info(), request)
    }

    fn update(&mut self, request: TaskUpdateRequest) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.task_progress, "task-update")?;
        self.context.task_update(self.info(), request)
    }

    fn finish(&mut self, request: TaskFinishRequest) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.task_progress, "task-finish")?;
        self.context.task_finish(self.info(), request)
    }

    fn register_definition(&mut self, request: TaskDefinitionRequest) -> Result<(), PluginError> {
        self.policy
            .require(self.policy.capabilities.task_progress, "task-register-definition")?;
        self.context.task_register_definition(self.info(), request)
    }

    fn create_typed(
        &mut self,
        request: TaskCreateRequest,
        definition: String,
    ) -> Result<String, PluginError> {
        self.policy
            .require(self.policy.capabilities.task_progress, "task-create-typed")?;
        // 命名空间校验：定义必须属于当前插件。
        if !definition.starts_with(&format!("{}.", self.policy.plugin_id)) {
            return Err(PluginError::denied(format!(
                "task definition does not belong to plugin {}",
                self.policy.plugin_id
            )));
        }
        self.context.task_create_typed(self.info(), request, &definition)
    }
}

impl HostServices for PluginState {
    fn register(&mut self, name: String) -> Result<(), PluginError> {
        if name.trim().is_empty() {
            return Err(PluginError::invalid_input(
                "service name must not be empty",
            ));
        }
        let plugin_id = self.policy.plugin_id.clone();
        self.effects.push(PluginEffect::RegisterService {
            plugin_id,
            service_name: name,
        });
        Ok(())
    }

    fn call(&mut self, request: ServiceCallRequest) -> Result<Vec<u8>, PluginError> {
        self.require_render_safe("cross-plugin service")?;
        if request.payload.len() > 4 * 1024 * 1024 {
            return Err(PluginError::invalid_input("service payload exceeds 4 MiB"));
        }
        self.context.service_call(self.info(), request)
    }
}

impl HostDownloads for PluginState {
    fn start(&mut self, request: DownloadRequest) -> Result<String, PluginError> {
        self.policy
            .require(self.policy.capabilities.download_start, "download-start")?;
        self.require_render_safe("download")?;
        if !self.policy.network_allow.allows_url(&request.url) {
            return Err(PluginError::denied("download URL is outside the plugin allowlist"));
        }
        self.context.download_resource(self.info(), request)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 执行体
// ─────────────────────────────────────────────────────────────────────────────

pub struct PluginExecution {
    store: Store<PluginState>,
    world: PluginWorld,
    poisoned: bool,
}

impl PluginExecution {
    /// 实例化插件组件（受限 WASI p2 + capability 门禁）。
    pub fn instantiate(
        engine: &PluginEngine,
        artifact: &ComponentArtifact,
        policy: PluginPolicy,
        context: Arc<dyn PluginHostContext>,
        init: PluginInitState,
    ) -> anyhow::Result<Self> {
        let stderr_pipe = MemoryOutputPipe::new(STDERR_CAPTURE_BYTES);
        let wasi_ctx = WasiCtxBuilder::new().stderr(stderr_pipe.clone()).build();

        let state = PluginState {
            limiter: PluginResourceLimiter::default(),
            policy,
            wasi_table: ResourceTable::new(),
            wasi_ctx,
            context,
            locale: init.locale,
            translations: init.translations,
            theme: init.theme,
            clipboard_text: init.clipboard_text,
            session: BTreeMap::new(),
            storage_values: init.storage_snapshot,
            config_text: init.config_text,
            effects: Vec::new(),
            next_window_id: 1,
            render_context: None,
            stderr_pipe,
            stderr_reported: 0,
        };

        let mut store = Store::new(engine.engine(), state);
        store.limiter(|state| &mut state.limiter);
        store.set_epoch_deadline(PluginEngine::deadline_ticks(EntryTimeout::Init));
        let world = artifact.pre.instantiate(&mut store)?;

        Ok(Self {
            store,
            world,
            poisoned: false,
        })
    }

    fn entry_guard(&mut self) -> Result<(), PluginError> {
        if self.poisoned {
            return Err(PluginError::timeout(
                "plugin execution terminated by a previous trap; reload the plugin",
            ));
        }
        Ok(())
    }

    fn set_deadline(&mut self, timeout: EntryTimeout) {
        self.store
            .set_epoch_deadline(PluginEngine::deadline_ticks(timeout));
    }

    fn map_trap(error: wasmtime::Error) -> PluginError {
        if error.downcast_ref::<Trap>() == Some(&Trap::Interrupt) {
            return PluginError::timeout(
                "plugin execution exceeded its time budget and was interrupted",
            );
        }
        if error.downcast_ref::<Trap>().is_some() {
            return PluginError::app(format!("plugin execution failed: {error}"));
        }
        PluginError::app(format!("plugin call failed: {error}"))
    }

    /// 处理一次入口调用结果：外层是 wasmtime（trap 层），内层是 WIT `result`（插件错误）。
    fn finish<T>(
        &mut self,
        result: wasmtime::Result<Result<T, PluginError>>,
    ) -> Result<T, PluginError> {
        match result {
            Ok(inner) => {
                self.drain_stderr_into_log();
                inner
            }
            Err(error) => {
                self.poisoned = true;
                self.discard_non_log_effects();
                self.drain_stderr_into_log();
                self.store.data().context.revoke_services();
                Err(Self::map_trap(error))
            }
        }
    }

    pub fn init(&mut self, context: PluginContext) -> Result<Vec<crate::bindings::bmcbl::plugin::types::Registration>, PluginError> {
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Init);
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_init(&mut self.store, &context);
        self.finish(result)
    }

    pub fn handle_event(&mut self, event: crate::bindings::bmcbl::plugin::types::PluginEvent) -> Result<(), PluginError> {
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Event);
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_handle_event(&mut self.store, &event);
        self.finish(result)
    }

    pub fn render_page(&mut self, request: PageRenderRequest) -> Result<ViewTree, PluginError> {
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Render);
        let request_clone = request.clone();
        self.store.data_mut().render_context = Some(PluginRenderContext::Page {
            page_id: request_clone.page_id,
        });
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_render_page(&mut self.store, &request);
        self.store.data_mut().render_context = None;
        self.finish(result)
    }

    pub fn render_injection(&mut self, request: InjectionRequest) -> Result<Option<ViewTree>, PluginError> {
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Render);
        let request_clone = request.clone();
        self.store.data_mut().render_context = Some(PluginRenderContext::Injection {
            slot: request_clone.slot,
            page: request_clone.page,
        });
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_render_injection(&mut self.store, &request);
        self.store.data_mut().render_context = None;
        self.finish(result)
    }

    pub fn call_service(&mut self, request: ServiceCallRequest) -> Result<Vec<u8>, PluginError> {
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Rpc);
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_call_service(&mut self.store, &request);
        self.finish(result)
    }

    pub fn shutdown(&mut self, reason: ShutdownReason) -> Result<(), PluginError> {
        self.store.data().context.revoke_services();
        self.entry_guard()?;
        self.set_deadline(EntryTimeout::Event);
        let plugin = self.world.bmcbl_plugin_plugin();
        let result = plugin.call_shutdown(&mut self.store, reason);
        self.finish(result)
    }

    /// 取走本次入口调用期间累积的宿主副作用。
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Keep diagnostic effects even when other effects cannot be applied.
    pub fn discard_non_log_effects(&mut self) -> usize {
        let effects = &mut self.store.data_mut().effects;
        let before = effects.len();
        effects.retain(|effect| matches!(effect, PluginEffect::Log { .. }));
        before - effects.len()
    }

    pub fn take_effects(&mut self) -> Vec<PluginEffect> {
        std::mem::take(&mut self.store.data_mut().effects)
    }

    /// 观测当前线性内存占用（预算评估用）。
    #[must_use]
    pub fn observed_memory_bytes(&self) -> usize {
        self.store.data().limiter.observed_bytes()
    }

    pub fn set_locale(&mut self, locale: String) {
        self.store.data_mut().locale = locale;
    }

    pub fn set_translations(&mut self, translations: BTreeMap<String, BTreeMap<String, String>>) {
        self.store.data_mut().translations = translations;
    }

    pub fn set_theme(&mut self, theme: ThemeSnapshot) {
        self.store.data_mut().theme = theme;
    }

    pub fn set_clipboard_text(&mut self, text: Option<String>) {
        self.store.data_mut().clipboard_text = text;
    }

    pub fn set_config_text(&mut self, text: Result<String, String>) {
        self.store.data_mut().config_text = text;
    }

    pub fn set_storage_snapshot(&mut self, snapshot: Result<BTreeMap<String, String>, String>) {
        self.store.data_mut().storage_values = snapshot;
    }

    pub fn policy(&self) -> &PluginPolicy {
        &self.store.data().policy
    }

    /// 把本次调用新产生的插件 stderr 内容以 Info 日志形式并入副作用。
    fn drain_stderr_into_log(&mut self) {
        let state = self.store.data_mut();
        let contents = state.stderr_pipe.contents();
        if contents.len() <= state.stderr_reported {
            return;
        }
        let fresh = &contents[state.stderr_reported..];
        // 按行拆分，避免一条日志吞下整段 stderr。
        for line in String::from_utf8_lossy(fresh).lines() {
            if !line.trim().is_empty() {
                state.push_log(LogLevel::Info, format!("[stderr] {line}"));
            }
        }
        state.stderr_reported = contents.len();
    }
}

/// `MemoryOutputPipe` 直接实现 wasi `StdoutStream`，无需适配器。

#[must_use]
pub fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn translate_plugin_key(
    translations: &BTreeMap<String, BTreeMap<String, String>>,
    locale: &str,
    key: &str,
    args: &[I18nArg],
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

// Re-entrancy：跨插件服务调用由宿主 context 实现；这里保留对共享执行体的借用约定。
pub type SharedExecution = Rc<RefCell<PluginExecution>>;

// `types` 接口只承载共享类型（无函数），但 bindgen 的 add_to_linker 约束要求实现其 Host。
impl crate::bindings::bmcbl::plugin::types::Host for PluginState {}

//! 宿主能力桥：应用侧实现的 [`PluginHostContext`]，以及插件调用期间产生的副作用
//! [`PluginEffect`]。
//!
//! capability 门禁与纯状态操作（session/locale/theme/KV 快照等）由运行时完成；
//! 需要应用系统（任务管理器、下载、toast、GPUI 导航等）的操作通过 trait 委派。

use crate::bindings::bmcbl::plugin::types::{
    AppInfo, DownloadRequest, HttpTextResponse, InvalidateTarget, LogLevel, ModalRequest,
    RouteTarget, ServiceCallRequest, TaskCreateRequest, TaskDefinitionRequest, TaskFinishRequest,
    TaskUpdateRequest, ThemeSnapshot, ToastKind, WindowRequest,
};
use crate::policy::PluginPolicy;

/// 当前入口调用是否处于渲染期，以及渲染目标（用于异步刷新完成后的精确失效）。
#[derive(Clone, Debug)]
pub enum PluginRenderContext {
    Page {
        page_id: String,
    },
    Injection {
        slot: crate::bindings::bmcbl::plugin::types::InjectionSlot,
        page: Option<String>,
    },
}

/// 单次宿主调用的上下文：策略快照 + 渲染期上下文。
#[derive(Clone, Debug)]
pub struct PluginCallInfo<'a> {
    pub policy: &'a PluginPolicy,
    /// `Some` 表示当前处于 render_page/render_injection 调用中；实现方据此拒绝
    /// 阻塞 IO（配置写入、存储写入等），并把异步刷新的失效订阅到该渲染目标。
    pub render_context: Option<PluginRenderContext>,
}

/// 插件在宿主调用期间产生的单向动作；宿主在入口调用返回后统一消费。
#[derive(Clone, Debug)]
pub enum PluginEffect {
    Log {
        plugin_id: String,
        level: LogLevel,
        message: String,
    },
    Toast {
        plugin_id: String,
        kind: ToastKind,
        message: String,
    },
    Navigate {
        plugin_id: String,
        target: RouteTarget,
    },
    OpenWindow {
        plugin_id: String,
        request: WindowRequest,
        window_id: u64,
    },
    CloseWindow {
        plugin_id: String,
        window_id: u64,
    },
    OpenModal {
        plugin_id: String,
        request: ModalRequest,
    },
    EmitEvent {
        plugin_id: String,
        name: String,
        payload: String,
    },
    Invalidate {
        plugin_id: String,
        target: InvalidateTarget,
    },
    RegisterService {
        plugin_id: String,
        service_name: String,
    },
    OpenExternalUrl {
        plugin_id: String,
        url: String,
    },
    WriteClipboard {
        plugin_id: String,
        text: String,
    },
    PersistStorage {
        plugin_id: String,
        key: String,
        value: String,
    },
    DeleteStorage {
        plugin_id: String,
        key: String,
    },
    PersistConfig {
        plugin_id: String,
        text: String,
    },
}

/// 应用侧实现的宿主服务桥。
///
/// capability 门禁由运行时完成；实现方负责各自的 allowlist 校验（网络 URL、资源
/// 路径、level.dat 路径约束）。所有方法都可能被插件在任意入口调用中
/// 触发，实现方需保证不重入当前插件（跨插件服务 RPC 由实现方用 RPC 栈防护）。
/// `Send + Sync` 是 wasmtime `WasiView` 对 Store 数据的要求。
#[allow(unused_variables)]
pub trait PluginHostContext: Send + Sync {
    fn invoke_service(&self, service: &str, arguments: Vec<u8>) -> Result<String, crate::PluginError> {
        Err(crate::PluginError::denied("BMCBL services are unavailable"))
    }

    fn take_service(&self, id: &str) -> Result<Option<Vec<u8>>, crate::PluginError> {
        Err(crate::PluginError::not_found("unknown service job"))
    }

    fn cancel_service(&self, id: &str) -> Result<(), crate::PluginError> {
        Err(crate::PluginError::not_found("unknown service job"))
    }

    fn revoke_services(&self) {}

    fn http_get_text(
        &self,
        info: PluginCallInfo<'_>,
        url: &str,
        ttl_seconds: u32,
        max_bytes: u32,
    ) -> Result<HttpTextResponse, crate::PluginError>;

    fn read_resource_text(&self, info: PluginCallInfo<'_>, path: &str)
        -> Result<String, crate::PluginError>;

    fn read_resource_bytes(
        &self,
        info: PluginCallInfo<'_>,
        path: &str,
    ) -> Result<Vec<u8>, crate::PluginError>;

    /// 校验 TOML schema 并排队持久化；不更新运行时快照（由运行时在 Ok 后写入）。
    fn write_config(&self, info: PluginCallInfo<'_>, text: &str) -> Result<(), crate::PluginError>;

    fn task_create(
        &self,
        info: PluginCallInfo<'_>,
        request: TaskCreateRequest,
    ) -> Result<String, crate::PluginError>;

    fn task_update(
        &self,
        info: PluginCallInfo<'_>,
        request: TaskUpdateRequest,
    ) -> Result<(), crate::PluginError>;

    fn task_finish(
        &self,
        info: PluginCallInfo<'_>,
        request: TaskFinishRequest,
    ) -> Result<(), crate::PluginError>;

    fn task_register_definition(
        &self,
        info: PluginCallInfo<'_>,
        request: TaskDefinitionRequest,
    ) -> Result<(), crate::PluginError>;

    fn task_create_typed(
        &self,
        info: PluginCallInfo<'_>,
        request: TaskCreateRequest,
        definition: &str,
    ) -> Result<String, crate::PluginError>;

    fn service_call(
        &self,
        info: PluginCallInfo<'_>,
        request: ServiceCallRequest,
    ) -> Result<Vec<u8>, crate::PluginError>;

    fn app_info(&self) -> AppInfo;

    fn download_resource(
        &self,
        info: PluginCallInfo<'_>,
        request: DownloadRequest,
    ) -> Result<String, crate::PluginError>;
}

/// 实例化插件执行体之前由应用注入的运行时状态快照。
#[derive(Clone, Debug)]
pub struct PluginInitState {
    pub locale: String,
    pub translations:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    pub theme: ThemeSnapshot,
    pub clipboard_text: Option<String>,
    /// 插件 KV 快照；加载失败时保留错误，读取调用返回失败。
    pub storage_snapshot: Result<std::collections::BTreeMap<String, String>, String>,
    /// 插件配置文本快照；读取失败时保留错误。
    pub config_text: Result<String, String>,
}

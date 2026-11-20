//! 应用侧 [`PluginHostContext`] 实现：把运行时的 WIT 宿主调用接到应用系统。
//!
//! capability 门禁在运行时完成；本模块负责 allowlist 校验（HTTP URL、资源路径、
//! level.dat 路径约束）、任务/下载/存档/版本等应用数据面的接入。

use std::path::PathBuf;

use bmcbl_plugin_runtime as abi;
use bmcbl_plugin_runtime::{PluginCallInfo, PluginHostContext, PluginRenderContext};

use crate::plugins::manifest::PluginManifest;
use crate::plugins::runtime::{PluginHttpFetchCache, PluginResourceCache, RenderContext, ServiceDispatcher};
use crate::tasks::task_manager::ProgressUpdate;
use crate::tasks::TaskStage;

/// 每个插件实例一份；持有共享缓存句柄与插件自己的清单/资源。
pub struct AppPluginHostContext {
    pub manifest: PluginManifest,
    pub http_cache: PluginHttpFetchCache,
    pub resource_cache: PluginResourceCache,
    pub storage_dir: PathBuf,
    pub service_dispatcher: Option<ServiceDispatcher>,
    pub(super) services: super::app_services::ServiceJobs,
}

impl AppPluginHostContext {
    fn render_context(&self, info: &PluginCallInfo<'_>) -> Option<RenderContext> {
        info.render_context.as_ref().map(|context| match context {
            PluginRenderContext::Page { page_id } => RenderContext::Page {
                page_id: page_id.clone(),
            },
            PluginRenderContext::Injection { slot, page } => RenderContext::Injection {
                slot: injection_slot_from_runtime(*slot),
                page: page.clone(),
            },
        })
    }

    /// level.dat/版本/存档扫描都是同步磁盘 IO，渲染期间拒绝。
    fn require_not_rendering(
        &self,
        info: &PluginCallInfo<'_>,
        operation: &'static str,
    ) -> Result<(), abi::PluginError> {
        if info.render_context.is_none() {
            return Ok(());
        }
        Err(abi::PluginError::new(
            "blocking-io-render-denied",
            format!(
                "{operation} is not allowed while rendering plugin UI; load data during init/event handling"
            ),
        ))
    }


}

fn plugin_error(code: &'static str, message: impl Into<String>) -> abi::PluginError {
    abi::PluginError::new(code, message)
}

fn injection_slot_from_runtime(
    slot: bmcbl_plugin_runtime::bindings::bmcbl::plugin::types::InjectionSlot,
) -> crate::plugins::events::InjectionSlot {
    use bmcbl_plugin_runtime::bindings::bmcbl::plugin::types::InjectionSlot as Rt;
    match slot {
        Rt::MainRootOverlay => crate::plugins::events::InjectionSlot::MainRootOverlay,
        Rt::PageHeader => crate::plugins::events::InjectionSlot::PageHeader,
        Rt::PageBody => crate::plugins::events::InjectionSlot::PageBody,
        Rt::HomeSidebar => crate::plugins::events::InjectionSlot::HomeSidebar,
    }
}

impl PluginHostContext for AppPluginHostContext {
    fn invoke_service(&self, service: &str, arguments: Vec<u8>) -> Result<String, abi::PluginError> {
        self.services.submit(&self.manifest, service, arguments)
    }

    fn take_service(&self, id: &str) -> Result<Option<Vec<u8>>, abi::PluginError> {
        self.services.take(id)
    }

    fn cancel_service(&self, id: &str) -> Result<(), abi::PluginError> {
        self.services.cancel(id)
    }

    fn revoke_services(&self) {
        self.services.revoke();
    }

    fn http_get_text(
        &self,
        info: PluginCallInfo<'_>,
        url: &str,
        ttl_seconds: u32,
        max_bytes: u32,
    ) -> Result<abi::HttpTextResponse, abi::PluginError> {
        let snapshot = self
            .http_cache
            .snapshot(
                &self.manifest,
                &self.manifest.id,
                self.render_context(&info).as_ref(),
                url,
                ttl_seconds,
                max_bytes,
            )
            .map_err(|error| abi::PluginError::new(error.code, error.message))?;
        Ok(abi::HttpTextResponse {
            state: snapshot.state,
            body: snapshot.body,
            error: snapshot.error,
            fetched_at_unix_ms: snapshot.fetched_at_unix_ms,
        })
    }

    fn read_resource_text(
        &self,
        info: PluginCallInfo<'_>,
        path: &str,
    ) -> Result<String, abi::PluginError> {
        let bytes = self
            .resource_cache
            .read(
                &self.manifest,
                &self.manifest.id,
                self.render_context(&info).as_ref(),
                path,
            )
            .map_err(|error| abi::PluginError::new(error.code, error.message))?;
        let text = std::str::from_utf8(bytes.as_ref()).map_err(|error| {
            plugin_error(
                "resource-not-utf8",
                format!("plugin resource {path} is not utf-8: {error}"),
            )
        })?;
        Ok(text.to_owned())
    }

    fn read_resource_bytes(
        &self,
        info: PluginCallInfo<'_>,
        path: &str,
    ) -> Result<Vec<u8>, abi::PluginError> {
        let bytes = self
            .resource_cache
            .read(
                &self.manifest,
                &self.manifest.id,
                self.render_context(&info).as_ref(),
                path,
            )
            .map_err(|error| abi::PluginError::new(error.code, error.message))?;
        Ok(bytes.as_ref().to_vec())
    }

    fn write_config(&self, info: PluginCallInfo<'_>, text: &str) -> Result<(), abi::PluginError> {
        if info.render_context.is_some() {
            return Err(plugin_error(
                "blocking-io-render-denied",
                "config write is not allowed while rendering plugin UI",
            ));
        }
        crate::plugins::manifest::validate_user_config(&self.manifest, text)
            .map_err(|error| plugin_error("config-write-failed", error.to_string()))
    }

    fn task_create(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::TaskCreateRequest,
    ) -> Result<String, abi::PluginError> {
        let mut task_options = TaskOptionsBuilder::new(
            request.title,
            TaskStage::custom(request.stage.clone()),
        );
        if let Some(total) = request.total {
            task_options = task_options.total(total);
        }
        if let Some(task_id) = request.task_id {
            task_options = task_options.id(task_id);
        }
        if let Some(detail) = request.detail {
            task_options = task_options.detail(detail);
        }
        task_options = task_options.pausable(request.supports_pause);
        crate::tasks::task_manager::create_task_owned(
            crate::tasks::task_manager::TaskOwner::Plugin(self.manifest.id.clone().into()),
            task_options,
        )
        .map_err(|error| plugin_error("task-create-failed", error))
    }

    fn task_update(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::TaskUpdateRequest,
    ) -> Result<(), abi::PluginError> {
        if !crate::tasks::task_manager::task_owned_by(&request.task_id, &self.manifest.id) {
            return Err(plugin_error(
                "task-not-owned",
                format!("任务不存在或不属于插件 {}", self.manifest.id),
            ));
        }
        let mut progress = ProgressUpdate::advance(request.done_delta);
        if let Some(total) = request.total {
            progress = progress.total(total);
        }
        if let Some(stage) = request.stage.as_deref() {
            progress = progress.stage(TaskStage::custom(stage));
        }
        crate::tasks::task_manager::update_progress(&request.task_id, progress);
        if let Some(message) = request.message {
            crate::tasks::task_manager::set_task_message(&request.task_id, Some(message));
        }
        Ok(())
    }

    fn task_finish(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::TaskFinishRequest,
    ) -> Result<(), abi::PluginError> {
        if !crate::tasks::task_manager::task_owned_by(&request.task_id, &self.manifest.id) {
            return Err(plugin_error(
                "task-not-owned",
                format!("任务不存在或不属于插件 {}", self.manifest.id),
            ));
        }
        let status = crate::tasks::task_manager::TaskFinishStatus::from_status_str(&request.status)
            .ok_or_else(|| {
                plugin_error(
                    "task-invalid-status",
                    format!(
                        "无效的任务终态: {}（允许 completed | cancelled | error）",
                        request.status
                    ),
                )
            })?;
        crate::tasks::task_manager::finish_task(&request.task_id, status, request.message);
        Ok(())
    }

    fn task_register_definition(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::TaskDefinitionRequest,
    ) -> Result<(), abi::PluginError> {
        let pool = match request.pool.as_deref() {
            None => None,
            Some("download") => Some(crate::tasks::scheduler::TaskPool::Download),
            Some("archive") => Some(crate::tasks::scheduler::TaskPool::Archive),
            Some(other) => {
                return Err(plugin_error(
                    "task-definition-invalid-pool",
                    format!("未知的任务队列: {other}（允许 download | archive）"),
                ));
            }
        };
        let mut definition = crate::tasks::registry::TaskDefinition::new(request.type_id)
            .capabilities(crate::tasks::registry::TaskCapabilities {
                cancellable: request.cancellable,
                pausable: request.pausable,
                prioritizable: request.prioritizable,
                retryable: request.retryable,
            });
        if let Some(title_key) = request.title_key {
            definition = definition.title_key(title_key);
        }
        if let Some(pool) = pool {
            definition = definition.pool(pool);
        }
        for phase in request.phases {
            definition = definition.phase(phase, None);
        }
        crate::tasks::registry::register_for_plugin(&self.manifest.id, definition).map_err(
            |error| plugin_error("task-definition-register-failed", error.to_string()),
        )
    }

    fn task_create_typed(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::TaskCreateRequest,
        definition: &str,
    ) -> Result<String, abi::PluginError> {
        let registered = crate::tasks::registry::definition(definition)
            .ok_or_else(|| {
                plugin_error(
                    "task-definition-unknown",
                    format!("任务类型未注册: {definition}"),
                )
            })?;
        let mut task_options = TaskOptionsBuilder::new(
            request.title,
            TaskStage::custom(request.stage.clone()),
        )
        .definition(registered.type_id.clone())
        .pausable(registered.capabilities.pausable);
        if let Some(total) = request.total {
            task_options = task_options.total(total);
        }
        if let Some(task_id) = request.task_id {
            task_options = task_options.id(task_id);
        }
        if let Some(detail) = request.detail {
            task_options = task_options.detail(detail);
        }
        crate::tasks::task_manager::create_task_owned(
            crate::tasks::task_manager::TaskOwner::Plugin(self.manifest.id.clone().into()),
            task_options,
        )
        .map_err(|error| plugin_error("task-create-failed", error))
    }

    fn service_call(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::ServiceCallRequest,
    ) -> Result<Vec<u8>, abi::PluginError> {
        let dispatcher = self.service_dispatcher.as_ref().ok_or_else(|| {
            plugin_error("service-bus-unavailable", "launcher has no service dispatcher")
        })?;
        (dispatcher.0)(&self.manifest.id, request)
    }

    fn app_info(&self) -> abi::AppInfo {
        abi::AppInfo {
            version: crate::utils::app_info::version().to_string(),
            build_info: crate::utils::app_info::build_info(),
            api_version: crate::plugins::manifest::CURRENT_API_VERSION.to_string(),
        }
    }

    fn download_resource(
        &self,
        _info: PluginCallInfo<'_>,
        request: abi::DownloadRequest,
    ) -> Result<String, abi::PluginError> {
        let runtime = crate::tasks::runtime::initialize_app_runtime()
            .map_err(|error| plugin_error("download-schedule-failed", error.to_string()))?;
        let task_id = runtime
            .block_on(crate::downloads::api::download_resource(
                request.url,
                request.file_name,
                request.md5,
                Some(request.force_download),
                None,
            ))
            .map_err(|error| plugin_error("download-start-failed", error))?;
        self.services.register_download(&task_id);
        Ok(task_id)
    }
}

/// 复用 task_manager 的 builder 形状，避免在桥接层重复字段拼装。
type TaskOptionsBuilder = crate::tasks::task_manager::TaskOptions;


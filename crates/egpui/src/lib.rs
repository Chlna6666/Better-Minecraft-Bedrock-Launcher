#![doc = "Desktop application framework for the egpui GUI stack."]
#![deny(missing_docs)]

mod durable;
mod durable_workflow;
mod host;
mod i18n;
mod lifecycle;
mod resources;
mod runtime;
mod service_registry;
mod task_bridge;
mod ui;

pub use durable::{
    DurableTaskId, DurableTaskIdError, DurableTaskPhase, DurableTaskRecord, DurableTaskRecovery,
    DurableTaskStore, DurableTaskStoreError, FileDurableTaskStore, recover_interrupted_tasks,
};
pub use durable_workflow::{
    DurableTaskCompletion, DurableTaskHandler, DurableWorkflowCoordinator, DurableWorkflowError,
};
pub use egpui_manifest as manifest;
pub use gpui;
pub use host::{ApplicationContext, ApplicationHost, HostConfig, HostError};
pub use i18n::{
    I18nCatalog, I18nError, I18nService, LocaleDirection, LocaleSnapshot, MessageArguments,
    MessageValue,
};
pub use lifecycle::{ApplicationLifecycle, LifecycleState};
pub use resources::{
    DirectoryResourcePack, MemoryResourcePack, ResolverAssetSource, ResourceHandle, ResourceId,
    ResourceIdError, ResourceMetadata, ResourcePack, ResourceResolver, ResourceResolverError,
    ResourceSource,
};
pub use runtime::{
    AppTask, ApplicationRuntime, BlockingTaskOptions, DefaultRuntimeProvider, RuntimeConfig,
    RuntimeError, RuntimeProvider, ScheduledTask, ShutdownReport, ShutdownToken, TaskCancellation,
    TaskError, TaskOutcome, TaskScope,
};
pub use service_registry::{ServiceRegistry, ServiceRegistryError};
pub use task_bridge::{
    UiTaskBridge, UiTaskExecutionError, UiTaskFailure, UiTaskFailureKind, UiTaskProgress,
    UiTaskReporter, UiTaskTerminal, UiTaskUpdate,
};
pub use tokio_util::sync::CancellationToken;
pub use ui::{
    UiBridgeConfig, UiCallError, UiDispatchError, UiEntityUpdateError, UiHandle, UiQueueState,
    UiSnapshotBridge, UiStreamBridge,
};

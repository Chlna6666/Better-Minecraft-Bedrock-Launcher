//! 产品代码性能基准使用的稳定入口。
//!
//! 只在 `cargo bench --features bench-support` 下编译。这里只转发业务模块里已经公开的类型
//! 与函数，不复制实现，保证基准走的是产品代码同一条路径。

pub use crate::plugins::events::{
    EventCascade, InjectionSlot, PluginEvent, PluginEventKind, ROUTE_CHANGED_EVENT,
};
pub use crate::plugins::manifest::PluginManifest;
pub use crate::plugins::runtime::{
    PluginMemoryReport, PluginRegistry, PluginStatus, RenderedInjection,
};
pub use crate::plugins::services::DependencyGraph;
pub use crate::plugins::ui_dsl::ViewTree;

#[cfg(target_os = "windows")]
pub use crate::core::minecraft::appx::utils::{
    find_any_game_executable_in_dir, find_game_executable_in_dir,
};
pub use crate::core::minecraft::import::{inspect_archive, scan_archive};

/// Reads installed-version metadata through the same CPU runtime and scanner as startup.
/// Only the supplied directory is read; no version settings or caches are persisted.
///
/// # Errors
/// Returns runtime initialization/submission or version-directory enumeration errors.
pub fn scan_local_versions(path: &std::path::Path) -> anyhow::Result<serde_json::Value> {
    crate::tasks::runtime::initialize_app_runtime()?;
    let versions = crate::tasks::runtime::install_cpu(|| {
        crate::core::version::version_manager::get_appx_version_list_blocking(path)
    })
    .map_err(anyhow::Error::msg)??;
    Ok(serde_json::Value::Array(versions.into_iter().map(|version| serde_json::json!({
        "folder": version.folder, "name": version.name, "version": version.version,
        "manifest_version": version.manifest_version, "path": version.path, "kind": version.kind,
        "custom_icon_path": version.custom_icon_path,
        "mod_loaders": format!("{:?}", version.mod_loaders),
        "game_info": format!("{:?}", version.game_info),
    })).collect()))
}

/// 基准专用：派发一次宿主事件，返回本次派发产生的宿主效果数量。
///
/// `PluginRegistry::handle_event` 是 crate 内部接口，基准通过这里进去，避免为了测量而放宽
/// 生产代码的可见性。每次调用使用独立的事件级联预算，因此度量的是"单个插件收到一次事件"的
/// 成本，而不是级联总量。
pub fn dispatch_event(registry: &mut PluginRegistry, event: PluginEvent) -> usize {
    let mut cascade = EventCascade::new();
    registry.handle_event(event, &mut cascade).len()
}

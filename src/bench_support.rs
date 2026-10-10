//! 插件系统性能基准使用的稳定入口。
//!
//! 只在 `cargo bench --features bench-support` 下编译。这里只转发插件模块里已经公开的类型
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

/// 基准专用：派发一次宿主事件，返回本次派发产生的宿主效果数量。
///
/// `PluginRegistry::handle_event` 是 crate 内部接口，基准通过这里进去，避免为了测量而放宽
/// 生产代码的可见性。每次调用使用独立的事件级联预算，因此度量的是"单个插件收到一次事件"的
/// 成本，而不是级联总量。
pub fn dispatch_event(registry: &mut PluginRegistry, event: PluginEvent) -> usize {
    let mut cascade = EventCascade::new();
    registry.handle_event(event, &mut cascade).len()
}

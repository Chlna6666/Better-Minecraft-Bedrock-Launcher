// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
//! BMCBL 插件运行时（宿主侧）：wasmtime(Cranelift) 组件引擎、沙箱与宿主能力门禁。
//!
//! 职责：
//! - [`engine`]：Cranelift 引擎、epoch 中断线程、组件编译缓存；
//! - [`sandbox`]：内存硬熔断/软预算、存储键校验；
//! - [`execution`]：每插件独立 Store、受限 WASI p2（零 preopen/socket/env，stderr
//!   捕获进插件日志）、按 WIT 生成的宿主接口实现与 capability 门禁、入口调用编排；
//! - [`context`]：应用侧 [`PluginHostContext`] 服务桥与 [`PluginEffect`] 副作用。
//!
//! 边界：不依赖 GPUI/应用代码；任务系统、toast、导航等由应用实现
//! [`PluginHostContext`] 接入。WIT 契约归 `bmcbl_plugin_api` crate 所有。

pub mod bindings;
pub mod context;
mod app_services;
pub mod engine;
pub mod execution;
pub mod policy;
pub mod sandbox;
mod view;

pub use bindings::bmcbl::plugin::types::{
    ActionEvent, Align, AppInfo, BadgeNode, ButtonNode, CheckboxNode, CompactBehavior,
    ContainerNode, DownloadRequest, EventSubscription, GlobalEvent, HttpCacheState,
    HttpTextResponse, I18nArg, IconNode, ImageFit, ImageNode, InjectionLayout, InjectionRegistration,
    InjectionRequest, InjectionSlot, InputNode, InvalidateTarget, JobReadyEvent, LayoutDirection, LinkNode,
    ListNode, LogLevel, ModalRequest, ToolEntry, PageRegistration, PageRenderRequest,
    PluginContext, PluginError, PluginEvent, PluginEventKind, ProgressNode, Registration,
    RouteChangedEvent, RouteTarget, SelectNode, SelectOption, ServiceCallRequest,
    ServiceRegistration, ShutdownReason, SpacerNode, TaskCreateRequest, TaskDefinitionRequest,
    TaskFinishRequest, TaskUpdateRequest, TextNode, TextSizeToken, ThemeColor, ThemeMode,
    ThemeSnapshot, ThemeToken, ToastKind, ToggleNode, ViewNode, ViewStyle, ViewTree,
    WindowRequest,
};
pub use context::{PluginCallInfo, PluginEffect, PluginHostContext, PluginInitState, PluginRenderContext};

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
pub use engine::{
    EntryTimeout, PluginEngine, ComponentArtifact, INIT_TIMEOUT, EVENT_TIMEOUT, RENDER_TIMEOUT,
    RENDER_WARN_THRESHOLD, RPC_TIMEOUT,
};
pub use execution::{PluginExecution, SharedExecution, unix_now_ms};
pub use policy::{Allowlist, PluginCapabilities, PluginLimits, PluginPolicy};

impl ThemeSnapshot {
    #[must_use]
    pub const fn light_default() -> Self {
        Self {
            mode: ThemeMode::Light,
            dark_factor: 0.0,
            accent: ThemeColor { h: 0.60, s: 0.88, l: 0.60, a: 1.0 },
            primary_text: ThemeColor { h: 0.61, s: 0.30, l: 0.20, a: 1.0 },
            secondary_text: ThemeColor { h: 0.60, s: 0.16, l: 0.45, a: 1.0 },
            muted_text: ThemeColor { h: 0.59, s: 0.14, l: 0.61, a: 1.0 },
            surface: ThemeColor { h: 0.58, s: 0.24, l: 0.96, a: 0.86 },
            surface_hover: ThemeColor { h: 0.58, s: 0.18, l: 0.93, a: 0.90 },
            border: ThemeColor { h: 0.60, s: 0.20, l: 0.84, a: 0.72 },
            danger: ThemeColor { h: 0.06, s: 0.93, l: 0.53, a: 1.0 },
        }
    }

    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self.mode, ThemeMode::Dark)
    }
}

pub use sandbox::{
    DEFAULT_HARD_LIMIT_BYTES, DEFAULT_SOFT_BUDGET_BYTES, BudgetEvaluation, PluginResourceLimiter,
    evaluate_plugin_budget,
};

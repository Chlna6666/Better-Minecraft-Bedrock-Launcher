# AI Conventions

## English

This repository uses GPUI for the native Rust desktop interface. Application
code should use `gpui::...` APIs directly and keep GPUI framework changes
separate from BMCBL product behavior.

### Goals

- Prefer GPUI-native UI over WebView-based rendering.
- Keep the framework reusable and independent from application business logic.
- Keep UI components small, composable, and testable.
- Ship a Windows-tested desktop executable with embedded assets.

Framework module and public API names must follow
Project-wide Rust naming and refactoring rules are defined in
[`RUST_NAMING_CONVENTIONS.md`](RUST_NAMING_CONVENTIONS.md). GPUI-specific examples
are defined in [`GPUI_NAMING_CONVENTIONS.md`](GPUI_NAMING_CONVENTIONS.md). The ordered cleanup
inventory is tracked in [`GPUI_NAMING_AUDIT.md`](GPUI_NAMING_AUDIT.md).

### Primary Docs

- `docs/BMCBL_PROJECT_STRUCTURE.md`: current workspace and module structure.
- `docs/ARCHITECTURE_BOUNDARIES.md`: ownership boundaries and change rules.
- `docs/ASYNC_RUNTIME_MODEL.md`: mandatory background execution, task state,
  and GPUI event-bridge rules.
- `docs/GPUI_VENDOR_RENDERING.md`: GPUI structure and rendering pipeline.
- `src/ui/README.md`: UI placement rules and current UI tree.
- `docs/PROJECT_SPEC.md`: product-level project specification.
- `docs/PROJECT_PLAN.md`: current project plan, entity icon asset format, and
  script rendering pipeline.

### Layout

- `src/app.rs`: application bootstrap, globals, fonts, windows, and startup
  policy.
- `src/ui/views/`: top-level GPUI views and route screens.
- `src/ui/window/`: standalone tool windows and window-specific internals.
- `src/ui/components/`: reusable UI components.
- `src/ui/theme/`: application theme tokens and helpers.
- `crates/lucide-gpui`: Lucide icon asset crate built on GPUI.
- `crates/gpui-hooks`: GPUI hook support.
- `crates/nova-gfx`: cross-backend graphics abstraction used by the GPUI nova
  renderer path.
- `src/i18n/`: application-owned localization implementation.
- `assets/locales/`: translation source of truth.
- `assets/`: embedded resources.
- `src/core/`, `src/config/`, `src/utils/`: non-UI application logic.

### GPUI Rules

- Use `App`, `Context<T>`, `Window`, `Entity<T>`, `Render`, and `RenderOnce`
  with the current GPUI API style.
- Use `cx.spawn(async move |cx| ...)` and related async closure APIs.
- Do not add application routes, pages, launcher policy, product assets, or
  business colors to GPUI framework code.
- Application defaults such as renderer preference, embedded fonts, default
  backgrounds, main-window chrome, and startup services belong in application
  startup or UI code.

### Configuration And Logging Rules

- Do not add environment-variable switches for log levels, debug traces,
  renderer selection, rendering parameters, diagnostics, profiling, or test
  behavior. Use the existing static debug filter, typed configuration, CLI
  arguments, or explicit API parameters.
- BMCBL and GPUI logs must flow through the application logging bridge and use
  debug-level records for diagnostic detail; do not introduce `RUST_LOG`,
  `GPUI_*`, `ZED_*`, or `BMCBL_*` runtime overrides.
- Environment variables are allowed only at integration boundaries where the
  operating system, compiler/build system, or an external child process
  defines the contract. They must not become a second BMCBL configuration
  system.

### Async And State Rules

Before changing runtime, task, download, archive, long-running core work, or
background-to-UI state propagation, read `docs/ASYNC_RUNTIME_MODEL.md`.

- Submit business work through the semantic APIs in `src/tasks/runtime.rs`.
- Do not construct runtimes or Rayon pools, probe `Handle::try_current()`, call
  `tokio::task::spawn_blocking` from GPUI, or use a system thread as fallback.
- Durable workflows live outside GPUI and publish pure events or snapshots.
- Only `completed`, `cancelled`, and `error` are terminal task states.
- Domain modules expose streams with explicit lag and closure behavior.
- Bind streams with `Context::spawn_stream` for view-scoped Entity state or
  `App::spawn_stream` for application-lifetime Global bridges. Do not hand-roll
  channel receive, entity-release, update, and notify loops in pages.
- A GPUI foreground consumer updates Entity or Global state and triggers
  repaint; render reads only stable UI-owned snapshots.
- An invalidation received during an in-flight refresh must preserve one
  follow-up refresh, including when the in-flight refresh was already forced.
- Polling requires a documented external-system limitation and must not replace
  an available producer event.

### UI View Structure

Keep view entrypoints small. A route file should primarily expose rendering or
composition for one page. Split large pages into sibling modules when a file
starts mixing layout, animation, data snapshots, and sub-view rendering.

Prefer composition first:

- parent views decide layout and route/tab composition;
- child modules render one responsibility panel;
- common visual elements live in `src/ui/components`;
- page-only widgets stay near the page.

Render methods should not perform network IO, durable cache work, parsing,
decoding, or long-running workflows. Use application state, background tasks,
and core modules for those responsibilities.

### Localization

- Use `I18n` (`src/ui/state/i18n.rs`) as a GPUI `Global`.
- Read translations in render code through `cx.global::<I18n>().t("key")`.
- Update language through global state updates and refresh affected windows.
- Keep translation source files under `assets/locales/`.

### Embedded Assets

- Windows manifest and app icon are embedded through `build.rs`.
- Fonts are embedded and registered during app startup.
- Runtime payload metadata is embedded by `build.rs`.
- Framework asset loading stays generic through GPUI `AssetSource`.

### Validation

Use focused checks for the area changed:

```powershell
cargo fmt --all
cargo check --workspace --no-default-features
cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

Current local validation is Windows-only. Linux and macOS are planned but
unverified for this repository state.

## 中文

本仓库使用 GPUI 构建原生 Rust 桌面界面。应用代码应直接使用 `gpui::...` API，并将
GPUI 框架改动与 BMCBL 产品行为分离。

### 目标

- 优先使用 GPUI 原生 UI，而不是基于 WebView 的渲染。
- 保持框架可复用，并独立于应用业务逻辑。
- 保持 UI 组件小型、可组合、可测试。
- 交付经过 Windows 验证、带嵌入资源的桌面可执行文件。

### 主要文档

- `docs/BMCBL_PROJECT_STRUCTURE.md`：当前 workspace 与模块结构。
- `docs/ARCHITECTURE_BOUNDARIES.md`：职责边界与变更规则。
- `docs/ASYNC_RUNTIME_MODEL.md`：后台执行、任务状态与 GPUI 事件桥接的强制规范。
- `docs/GPUI_VENDOR_RENDERING.md`：GPUI 结构与渲染管线。
- `src/ui/README.md`：UI 放置规则与当前 UI 目录。
- `docs/PROJECT_SPEC.md`：项目规格。
- `docs/PROJECT_PLAN.md`：当前项目规划、实体图标格式与脚本渲染管线。

### 布局

- `src/app.rs`：应用启动、globals、字体、窗口和启动策略。
- `src/ui/views/`：顶层 GPUI view 和路由页面。
- `src/ui/window/`：独立工具窗口和窗口专属内部模块。
- `src/ui/components/`：可复用 UI 组件。
- `src/ui/theme/`：应用主题 token 和 helper。
- `crates/lucide-gpui`：基于 GPUI 的 Lucide 图标资源 crate。
- `crates/gpui-hooks`：GPUI hooks 支持。
- `crates/nova-gfx`：GPUI nova 渲染路径使用的跨后端图形抽象。
- `src/i18n/`：应用拥有的本地化实现。
- `assets/locales/`：翻译源数据。
- `assets/`：嵌入资源。
- `src/core/`、`src/config/`、`src/utils/`：非 UI 应用逻辑。

### GPUI 规则

- 按当前 GPUI API 风格使用 `App`、`Context<T>`、`Window`、`Entity<T>`、
  `Render` 和 `RenderOnce`。
- 使用 `cx.spawn(async move |cx| ...)` 及相关 async closure API。
- 不要把应用 routes、pages、launcher policy、product assets 或业务颜色加入 GPUI
  框架代码。
- renderer preference、嵌入字体、默认背景、主窗口 chrome、启动服务等应用默认值
  属于应用启动或 UI 代码。

### 配置与日志规则

- 不得为日志级别、debug trace、renderer 选择、渲染参数、诊断、性能分析或测试行为
  新增环境变量开关；应使用现有静态 debug filter、类型化配置、命令行参数或显式 API
  参数。
- BMCBL 与 GPUI 的日志必须通过应用日志桥接进入统一日志系统，诊断细节使用 debug
  级别；不得重新引入 `RUST_LOG`、`GPUI_*`、`ZED_*` 或 `BMCBL_*` 的运行时覆盖。
- 只有操作系统、编译/构建系统或外部子进程定义协议的集成边界可以使用环境变量；环境
  变量不能成为第二套 BMCBL 配置系统。

### 异步与状态规则

修改运行时、任务、下载、归档、长期 core 工作或后台到 UI 状态传播前，必须阅读
`docs/ASYNC_RUNTIME_MODEL.md`。

- 业务工作只能通过 `src/tasks/runtime.rs` 的语义化 API 提交。
- 禁止自行创建 Runtime/Rayon Pool、探测 `Handle::try_current()`、从 GPUI
  调用 `tokio::task::spawn_blocking`，或用系统线程作通用兜底。
- 持久工作流必须位于 GPUI 外部，并且只发布纯事件或快照。
- 只有 `completed`、`cancelled` 和 `error` 是任务终态。
- 领域模块必须暴露具有明确滞后恢复与关闭语义的事件流。
- View 生命周期内的 Entity 状态使用 `Context::spawn_stream`，应用生命周期内的
  Global 桥使用 `App::spawn_stream`。禁止页面自行编写 channel 接收、实体释放、
  update 与 notify 循环。
- GPUI 前台消费者负责更新 Entity/Global 并触发重绘；render 只能读取 UI
  自己持有的稳定快照。
- 刷新进行期间到达的失效事件必须保留一次后续刷新，即使当前已经是强制刷新；
  多个事件可以合并，但不能静默丢失。
- 轮询必须有明确的外部系统限制，不能替代已经可以生产的事件。

### UI View 结构

保持 view entrypoint 小而清晰。路由文件主要暴露某个页面的渲染或组合逻辑。当一个
大页面开始混合 layout、animation、data snapshot 和子视图渲染时，将它拆到同级
模块中。

优先组合：

- parent view 决定布局和 route/tab 组合；
- child module 渲染单一职责面板；
- 通用视觉元素放在 `src/ui/components`；
- 页面专用 widget 靠近对应页面。

Render 方法不应执行网络 IO、持久缓存、解析、解码或长期工作流。这些职责应放在
应用状态、后台任务和 core module 中。

### 本地化

- 使用 `I18n` (`src/ui/state/i18n.rs`) 作为 GPUI `Global`。
- render 代码通过 `cx.global::<I18n>().t("key")` 读取翻译。
- 通过 global state 更新语言，并刷新受影响窗口。
- 翻译源文件保存在 `assets/locales/`。

### 嵌入资源

- Windows manifest 和 app icon 通过 `build.rs` 嵌入。
- 字体在应用启动期间嵌入并注册。
- runtime payload metadata 由 `build.rs` 嵌入。
- 框架资源加载保持为通用 GPUI `AssetSource`。

### 验证

根据改动范围使用聚焦检查：

```powershell
cargo fmt --all
cargo check --workspace --no-default-features
cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

当前本地验证以 Windows 为准。Linux 和 macOS 计划支持，但此仓库状态尚未验证。

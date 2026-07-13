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

### Primary Docs

- `docs/BMCBL_PROJECT_STRUCTURE.md`: current workspace and module structure.
- `docs/ARCHITECTURE_BOUNDARIES.md`: ownership boundaries and change rules.
- `docs/GPUI_VENDOR_RENDERING.md`: GPUI structure and rendering pipeline.
- `src/ui/README.md`: UI placement rules and current UI tree.
- `docs/PROJECT_SPEC.md`: product-level project specification.

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
cargo check --manifest-path vendor/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
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
- `docs/GPUI_VENDOR_RENDERING.md`：GPUI 结构与渲染管线。
- `src/ui/README.md`：UI 放置规则与当前 UI 目录。
- `docs/PROJECT_SPEC.md`：项目规格。

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
cargo check --manifest-path vendor/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

当前本地验证以 Windows 为准。Linux 和 macOS 计划支持，但此仓库状态尚未验证。

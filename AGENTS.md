# Rust Design Conventions / Rust 设计规范

本项目使用 `skills/rust-design-conventions` 技能作为 Rust 全栈设计与性能权威指南。
当涉及编写 Rust 代码、项目结构、模块/Crate 划分、API 设计、Cargo.toml 配置、命名
规范、性能优化、内存与布局、并发（Send/Sync/原子）、异步（Future/Pin/Tokio）、
零成本抽象、unsafe/FFI、零拷贝、生命周期、宏系统、构建/Features/交叉编译、测试、
文档注释、Lint/clippy、SemVer、依赖管理与供应链等任务时，**优先读取该技能**。

技能位置：

- `skills/rust-design-conventions/SKILL.md` — 主文件：默认行为规则 + 场景路由索引。
- `skills/rust-design-conventions/references/*.md` — 按主题拆分的深度参考模块
  （api-design、async-programming、cargo-build-features、code-robustness、
  concurrency、dependency-management、documentation、file-layout、lifetimes、
  lint-and-clippy、macros、memory-and-layout、naming-conventions、
  performance-optimization、performance-pitfalls、testing-standards、unsafe-rust、
  zero-copy、zero-cost-abstractions）。

> 用法：不要一次性读取所有参考文件。根据当前任务场景，按主文件中的「场景 → 模块对照表」
> 按需读取最相关的 1–2 个模块。

## Default Project Settings

When creating Rust projects or Cargo.toml files, ALWAYS use:

```toml
[package]
edition = "2024"

[lints.rust]
unsafe_code = "warn"

[lints.clippy]
all = "warn"
pedantic = "warn"
```


# Rust coding guidelines

* Prioritize code correctness and clarity. Speed and efficiency are secondary priorities unless otherwise specified.
* Do not write organizational or comments that summarize the code. Comments should only be written in order to explain "why" the code is written in some way in the case there is a reason that is tricky / non-obvious.
* Prefer implementing functionality in existing files unless it is a new logical component. Avoid creating many small files.
* Avoid using functions that panic like `unwrap()`, instead use mechanisms like `?` to propagate errors.
* Be careful with operations like indexing which may panic if the indexes are out of bounds.
* Never silently discard errors with `let _ =` on fallible operations. Always handle errors appropriately:
  - Propagate errors with `?` when the calling function should handle them
  - Use `.log_err()` or similar when you need to ignore errors but want visibility
  - Use explicit error handling with `match` or `if let Err(...)` when you need custom logic
  - Example: avoid `let _ = client.request(...).await?;` - use `client.request(...).await?;` instead
* When implementing async operations that may fail, ensure errors propagate to the UI layer so users get meaningful feedback.
* Never create files with `mod.rs` paths - prefer `src/some_module.rs` instead of `src/some_module/mod.rs`.
* When creating new crates, prefer specifying the library root path in `Cargo.toml` using `[lib] path = "...rs"` instead of the default `lib.rs`, to maintain consistent and descriptive naming (e.g., `gpui.rs` or `main.rs`).
* Avoid creative additions unless explicitly requested
* Use full words for variable names (no abbreviations like "q" for "queue")
* Use variable shadowing to scope clones in async contexts for clarity, minimizing the lifetime of borrowed references.
  Example:
  ```rust
  executor.spawn({
      let task_ran = task_ran.clone();
      async move {
          *task_ran.borrow_mut() = true;
      }
  });
  ```

# BMCBL Project Structure / 项目结构

BMCBL 是一个基于 GPUI 的原生 Rust 桌面启动器（Windows 优先）。下面给出仓库的文件路径树，并说明每个目录与关键文件的功能。图标资源（`crates/lucide-gpui/icons/`）与 `vendor/` 第三方依赖在树中省略，避免噪声。

## Workspace Layout / 顶层布局

```
BMCBL/
├── Cargo.toml              # Workspace 根清单，声明成员 crate 与共享依赖
├── Cargo.lock              # 依赖锁定
├── build.rs                # 应用级构建脚本：嵌入 Windows 清单、图标、payload 元数据
├── src/                    # BMCBL 应用主 crate（二进制 + 库）
├── crates/                 # 本地工作空间成员 crate
│   ├── gpui-hooks/         # GPUI React 风格 hooks 适配层（use_state 等）
│   ├── gpui-hooks-macros/  # hooks 的过程宏
│   ├── lucide-gpui/        # Lucide 图标资源 crate（基于 GPUI）
│   ├── nova-gfx/           # 跨后端图形抽象（Vulkan/DX12/Metal）与示例
│   ├── bmcbl-plugin-api/   # 插件宿主/插件间公共 API 类型
│   ├── bmcbl-plugin-macros/# 插件开发派生宏
│   └── bmcbl-plugin-tools/ # 插件打包/校验工具
├── vendor/gpui/            # 内嵌的 GPUI 框架源码（独立子清单，勿直接耦合业务）
├── assets/                 # 嵌入资源（编译期通过 AssetSource 打包）
├── docs/                   # 架构与设计文档
├── examples/plugins/       # 插件示例（bedrock-notes、hello-wasm）
└── scripts/                # 构建/校验/性能脚本（PowerShell）
```

## Application Source / 应用主 crate (`src/`)

```
src/
├── main.rs                 # 二进制入口：解析参数并启动 app
├── lib.rs                  # 库根：重导出模块，供测试与集成
├── app.rs                  # 应用启动：globals、字体注册、窗口、启动策略
├── startup.rs              # 启动流程编排（初始化顺序、单实例检查等）
├── launch.rs               # Minecraft 进程拉起逻辑
├── result.rs               # 统一错误/结果类型别名
├── config/                 # 配置模型与持久化
│   ├── config.rs           # Config 结构与字段
│   ├── defaults.rs         # 默认值
│   ├── storage.rs          # 读写配置文件
│   └── test.rs             # 配置测试辅助
├── core/                   # 非 UI 业务逻辑（核心领域）
│   ├── mod.rs
│   ├── minecraft/          # MC 版本管理、mod 管理、地图、截图、服务器、
│   │                       #   level.dat、资源包、UWP/AppX/GDK 集成、
│   │                       #   key patcher、mouse lock、远程版本源等
│   ├── curseforge/         # CurseForge API 客户端与数据模型
│   ├── easytier/           # EasyTier 联网（虚拟局域网）集成
│   ├── inject/             # 注入/补丁相关底层工具
│   ├── online/             # 在线房间/对等连接业务
│   ├── version/            # 版本号解析与比较
│   ├── sponsors.rs         # 赞助者数据
│   └── ui_prefs.rs         # UI 偏好（桥接 config 与 UI）
├── downloads/              # 下载引擎
│   ├── manager.rs          # 下载管理器（调度、任务编排）
│   ├── single.rs / multi.rs # 单文件 / 多文件下载
│   ├── integrity.rs / md5.rs # 校验完整性、MD5
│   ├── api.rs / runtime.rs # 下载对外 API 与运行时支持
│   └── mod.rs
├── archive/                # 归档/解压（zip 等）
├── http/                   # HTTP 客户端
│   ├── request.rs          # 请求封装
│   ├── gpui_client.rs      # GPUI 线程友好的客户端
│   └── proxy.rs            # 代理支持
├── tasks/                  # 后台任务管理器（下载/解压/联网等统一调度）
├── music/                  # 内置音乐播放器（service/state/types）
├── plugins/                # 插件运行时（事件、清单、watcher、UI DSL、window）
├── i18n/                   # 本地化实现（读取 assets/locales）
├── assets/                 # 资源加载（asset_source / generated / mod）
└── utils/                  # 通用工具（日志、网络、内存、诊断、更新器、
                            #   文件操作、单实例、系统信息、Cloudflare 等）
```

## UI Layer / UI 层 (`src/ui/`)

遵循「页面/窗口根 view 只负责组合与生命周期」的原则，每个大页面按职责拆分到子模块。

```
src/ui/
├── mod.rs                  # UI 模块总装配
├── main_window.rs          # 主窗口入口（组合）
├── main_window/            # 主窗口职责模块
│   ├── background(.rs/_support.rs)  # 动态背景与支撑逻辑
│   ├── chrome(.rs/_view.rs)         # 标题栏/窗口边框 chrome
│   ├── controls.rs                  # 窗口控件（最小化/关闭等）
│   ├── music_player.rs              # 内嵌播放器面板
│   ├── page_loading.rs / page_registry.rs / route_effects.rs
│   ├── update_flow.rs / support.rs
├── window.rs               # 工具/子窗口入口
├── window/                 # 子窗口实现
│   ├── map_viewer/         # 地图查看器（含 3D 预览、WGSL 着色器、瓦片缓存、
│   │                       #   交互、预览面板渲染、测试）
│   ├── chrome.rs / debug(/) # 子窗口 chrome 与调试视图
│   ├── import(/)            # 导入流程窗口
│   └── level_dat(/)         # level.dat 编辑器窗口
├── views/                  # 顶层路由页面
│   ├── home(/)             # 首页
│   ├── download(/)         # 下载中心（游戏/mod/CurseForge 子页）
│   ├── manage/             # 存档管理（actions、tabs、layout、state、
│   │                        #   level_dat_editor/schema、version_settings…）
│   ├── settings(/)         # 设置（launcher/about/customization/game/plugins）
│   ├── tasks(/)            # 任务列表页
│   ├── tools(/)            # 工具页（在线联机 room/peers/widgets/sidebar）
│   └── plugin.rs           # 插件页面
├── components/             # 可复用 UI 组件（button、modal、dropdown、
│                            #   markdown_renderer、html_renderer、tabs、
│                            #   split_pane、virtual_list、color_picker…）
├── state/                  # 全局/共享 UI 状态（navigation、launcher、
│                            #   i18n、theme、update、diagnostics、agreement…）
├── theme/                  # 主题 tokens（colors）与 helper
├── runtime/                # 运行时根视图装配（root_view）
├── overlays/               # 全屏覆盖层（更新、诊断、启动前置、用户协议）
├── hooks.rs / hooks/       # GPUI hooks 适配与封装
├── animation.rs            # 动画辅助
├── navigation.rs           # 路由/导航状态机
├── overlays.rs             # 覆盖层装配
├── state.rs                # 状态装配
├── runtime.rs              # 运行时装配
├── update_check.rs         # 更新检查编排
└── README.md               # UI 层约定说明
```

## Assets & Docs / 资源与文档

```
assets/
├── fonts/                  # 嵌入字体（HarmonyOS Sans / MiSans / OPPO Sans）
├── icons/                  # 应用图标
├── images/                 # 内嵌图片（about、minecraft 等）
├── locales/                # 翻译源数据（含 agreement）
└── bin/                    # 需随应用分发的二进制

docs/
├── AI.md                   # AI 代码贡献约定（双语，GPUI 规则）
├── ARCHITECTURE_BOUNDARIES.md  # GPUI 框架与应用的边界（改框架前必读）
├── PROJECT_SPEC.md         # 项目规格
├── GPUI_ROUTER_HOOKS.md    # 路由与 hooks 用法
├── GPUI_VENDOR_RENDERING.md # vendor GPUI 渲染说明
├── GPUI_DEFAULT_FONT.md    # 默认字体策略
└── MAP_RENDERER.md         # 地图渲染器设计

scripts/
├── check_i18n_lang.ps1     # 校验多语言键完整性
├── profile_startup.ps1     # 启动性能分析
└── tmp_patch_bedrock_model_material.ps1  # 临时补丁脚本
```

## Boundary Rules Recap / 边界要点

- `src/ui` 只渲染与协调 UI 状态；网络 IO、解码、持久缓存、解析、下载与长流程放在 `src/core`、`src/downloads`、`src/tasks` 等非 UI 模块。
- 修改 `vendor/gpui`、`src/app.rs` 或 `src/ui` 顶层前，先读 `docs/ARCHITECTURE_BOUNDARIES.md`。
- GPUI 框架代码不得依赖 BMCBL 的 routes/pages/assets/默认背景/下载服务/窗口策略。
- 应用默认值（Vulkan 偏好、嵌入字体、默认背景、主窗口 chrome、启动服务）归应用启动或 UI 代码，而非 GPUI 框架默认。

# GPUI

## Git 提交规范

提交信息统一使用 Conventional Commits，描述内容使用中文，格式为
`类型(范围): 中文描述`。允许类型和 hook 使用方式见
[docs/COMMIT_CONVENTIONS.md](docs/COMMIT_CONVENTIONS.md)。

本仓库使用 Rust 编写的 Cocogitto 管理提交规范和 Git hook。开发者首次使用时执行
`cargo install cocogitto --locked` 和 `cog install-hook commit-msg`。详细规范见
[docs/COMMIT_CONVENTIONS.md](docs/COMMIT_CONVENTIONS.md) 与根目录 `cog.toml`。

GPUI is a UI framework which also provides primitives for state and concurrency management.

## Project boundaries

- Follow `docs/ARCHITECTURE_BOUNDARIES.md` before changing the GPUI framework,
  `src/app.rs`, or `src/ui`.
- GPUI framework code must not depend on BMCBL routes, pages, assets, default
  backgrounds, download services, or application window policy.
- BMCBL application defaults such as Vulkan preference, embedded fonts, default
  background selection, and main-window chrome behavior belong in application
  startup or UI code, not in GPUI framework defaults.
- `src/ui` renders and coordinates UI state. Network IO, decoding, persistent
  cache implementation, parsing, downloads, and durable program workflows belong
  outside render methods and generic UI components.

## UI modularity

- Page and window root `view.rs` files should only assemble the top-level UI,
  own the lifecycle entry points, and implement `Render`.
- Do not put state models, persistent IO, caches, decoding, parsing, background
  render tasks, panel rendering, and pointer/input behavior into one large
  `view.rs`.
- Split a UI file once it exceeds roughly 1,500 lines or contains more than two
  major responsibilities. Prefer responsibility modules such as `model.rs`,
  `interactions.rs`, `panels.rs`, `tile_cache.rs`, `tile_render.rs`,
  `viewport.rs`, or domain-specific equivalents.
- Keep page/window internals scoped as `pub(super)` by default. Do not expose
  internal UI state or helper types at crate level unless another ownership
  boundary genuinely needs them.
- New UI features should extend or add the relevant responsibility module
  instead of appending more logic to the root `view.rs`.

## Context

Context types allow interaction with global state, windows, entities, and system services. They are typically passed to functions as the argument named `cx`. When a function takes callbacks they come after the `cx` parameter.

* `App` is the root context type, providing access to global state and read and update of entities.
* `Context<T>` is provided when updating an `Entity<T>`. This context dereferences into `App`, so functions which take `&App` can also take `&Context<T>`.
* `AsyncApp` and `AsyncWindowContext` are provided by `cx.spawn` and `cx.spawn_in`. These can be held across await points.

## `Window`

`Window` provides access to the state of an application window. It is passed to functions as an argument named `window` and comes before `cx` when present. It is used for managing focus, dispatching actions, directly drawing, getting user input state, etc.

## Entities

An `Entity<T>` is a handle to state of type `T`. With `thing: Entity<T>`:

* `thing.entity_id()` returns `EntityId`
* `thing.downgrade()` returns `WeakEntity<T>`
* `thing.read(cx: &App)` returns `&T`.
* `thing.read_with(cx, |thing: &T, cx: &App| ...)` returns the closure's return value.
* `thing.update(cx, |thing: &mut T, cx: &mut Context<T>| ...)` allows the closure to mutate the state, and provides a `Context<T>` for interacting with the entity. It returns the closure's return value.
* `thing.update_in(cx, |thing: &mut T, window: &mut Window, cx: &mut Context<T>| ...)` takes a `AsyncWindowContext` or `VisualTestContext`. It's the same as `update` while also providing the `Window`.

Within the closures, the inner `cx` provided to the closure must be used instead of the outer `cx` to avoid issues with multiple borrows.

Trying to update an entity while it's already being updated must be avoided as this will cause a panic.

When  `read_with`, `update`, or `update_in` are used with an async context, the closure's return value is wrapped in an `anyhow::Result`.

`WeakEntity<T>` is a weak handle. It has `read_with`, `update`, and `update_in` methods that work the same, but always return an `anyhow::Result` so that they can fail if the entity no longer exists. This can be useful to avoid memory leaks - if entities have mutually recursive handles to each other they will never be dropped.

## Concurrency

All use of entities and UI rendering occurs on a single foreground thread.

`cx.spawn(async move |cx| ...)` runs an async closure on the foreground thread. Within the closure, `cx` is an async context like `AsyncApp` or `AsyncWindowContext`.

When the outer cx is a `Context<T>`, the use of `spawn` instead looks like `cx.spawn(async move |handle, cx| ...)`, where `handle: WeakEntity<T>`.

To do work on other threads, `cx.background_spawn(async move { ... })` is used. Often this background task is awaited on by a foreground task which uses the results to update state.

Both `cx.spawn` and `cx.background_spawn` return a `Task<R>`, which is a future that can be awaited upon. If this task is dropped, then its work is cancelled. To prevent this one of the following must be done:

* Awaiting the task in some other async context.
* Detaching the task via `task.detach()` or `task.detach_and_log_err(cx)`, allowing it to run indefinitely.
* Storing the task in a field, if the work should be halted when the struct is dropped.

A task which doesn't do anything but provide a value can be created with `Task::ready(value)`.

## Elements

The `Render` trait is used to render some state into an element tree that is laid out using flexbox layout. An `Entity<T>` where `T` implements `Render` is sometimes called a "view".

Example:

```
struct TextWithBorder(SharedString);

impl Render for TextWithBorder {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().border_1().child(self.0.clone())
    }
}
```

Since `impl IntoElement for SharedString` exists, it can be used as an argument to `child`. `SharedString` is used to avoid copying strings, and is either an `&'static str` or `Arc<str>`.

UI components that are constructed just to be turned into elements can instead implement the `RenderOnce` trait, which is similar to `Render`, but its `render` method takes ownership of `self`. Types that implement this trait can use `#[derive(IntoElement)]` to use them directly as children.

The style methods on elements are similar to those used by Tailwind CSS.

If some attributes or children of an element tree are conditional, `.when(condition, |this| ...)` can be used to run the closure only when `condition` is true. Similarly, `.when_some(option, |this, value| ...)` runs the closure when the `Option` has a value.

## Input events

Input event handlers can be registered on an element via methods like `.on_click(|event, window, cx: &mut App| ...)`.

Often event handlers will want to update the entity that's in the current `Context<T>`. The `cx.listener` method provides this - its use looks like `.on_click(cx.listener(|this: &mut T, event, window, cx: &mut Context<T>| ...)`.

## Actions

Actions are dispatched via user keyboard interaction or in code via `window.dispatch_action(SomeAction.boxed_clone(), cx)` or `focus_handle.dispatch_action(&SomeAction, window, cx)`.

Actions with no data defined with the `actions!(some_namespace, [SomeAction, AnotherAction])` macro call. Otherwise the `Action` derive macro is used. Doc comments on actions are displayed to the user.

Action handlers can be registered on an element via the event handler `.on_action(|action, window, cx| ...)`. Like other event handlers, this is often used with `cx.listener`.

## Notify

When a view's state has changed in a way that may affect its rendering, it should call `cx.notify()`. This will cause the view to be rerendered. It will also cause any observe callbacks registered for the entity with `cx.observe` to be called.

## Entity events

While updating an entity (`cx: Context<T>`), it can emit an event using `cx.emit(event)`. Entities register which events they can emit by declaring `impl EventEmittor<EventType> for EntityType {}`.

Other entities can then register a callback to handle these events by doing `cx.subscribe(other_entity, |this, other_entity, event, cx| ...)`. This will return a `Subscription` which deregisters the callback when dropped.  Typically `cx.subscribe` happens when creating a new entity and the subscriptions are stored in a `_subscriptions: Vec<Subscription>` field.

## Recent API changes

GPUI has had some changes to its APIs. Always write code using the new APIs:

* `spawn` methods now take async closures (`AsyncFn`), and so should be called like `cx.spawn(async move |cx| ...)`.
* Use `Entity<T>`. This replaces `Model<T>` and `View<T>` which no longer exist and should NEVER be used.
* Use `App` references. This replaces `AppContext` which no longer exists and should NEVER be used.
* Use `Context<T>` references. This replaces `ModelContext<T>` which no longer exists and should NEVER be used.
* `Window` is now passed around explicitly. The new interface adds a `Window` reference parameter to some methods, and adds some new "*_in" methods for plumbing `Window`. The old types `WindowContext` and `ViewContext<T>` should NEVER be used.


## General guidelines

- Use `./script/clippy` instead of `cargo clippy`

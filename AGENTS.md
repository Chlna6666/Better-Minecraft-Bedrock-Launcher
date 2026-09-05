# BMCBL Agent Instructions

## 项目定位与事实源

BMCBL 是面向 Minecraft Bedrock 的原生 Rust/GPUI 启动器，Windows 是当前首要
支持平台。应用以单个原生可执行文件为主要交付形态，资源由应用构建流程嵌入，
UI 使用仓库内维护的 GPUI 与 nova-gfx 渲染路径；业务逻辑应保持为普通 Rust
模块，不通过 WebView command 承载核心功能。

以下文档是分主题的事实源；本文件只保留协作时必须知道的边界和入口，细节变更时
应优先更新对应事实源，而不是复制一份更长的目录说明：

- [`docs/BMCBL_PROJECT_STRUCTURE.md`](docs/BMCBL_PROJECT_STRUCTURE.md)：当前 workspace 和模块结构。
- [`docs/PROJECT_SPEC.md`](docs/PROJECT_SPEC.md)：产品目标、资源嵌入和验证基线。
- [`docs/ARCHITECTURE_BOUNDARIES.md`](docs/ARCHITECTURE_BOUNDARIES.md)：GPUI 与应用的职责边界。
- [`docs/ASYNC_RUNTIME_MODEL.md`](docs/ASYNC_RUNTIME_MODEL.md)：运行时所有权、任务终态和 GPUI 状态桥接。
- [`src/ui/README.md`](src/ui/README.md)：UI 放置规则和当前 UI 细分结构。
- [`docs/AI.md`](docs/AI.md)：GPUI、日志、异步、资源和验证约定。
- [`docs/GPUI_ANIMATION_CONVENTIONS.md`](docs/GPUI_ANIMATION_CONVENTIONS.md)：动画帧时钟、retained animation、text geometry、Nova animation binding 与 invalidation 硬约束。
- [`docs/GPUI_VENDOR_RENDERING.md`](docs/GPUI_VENDOR_RENDERING.md)：GPUI/Nova 渲染管线与 frame lifecycle。
- [`docs/PROJECT_PLAN.md`](docs/PROJECT_PLAN.md)：当前计划以及实体图标/脚本流水线。
- [`docs/COMMIT_CONVENTIONS.md`](docs/COMMIT_CONVENTIONS.md)：提交信息和 Cocogitto hook。

## 修改边界

默认采用最小变更原则。开始修改前先明确 IN SCOPE / OUT OF SCOPE，并确认目标文件
属于正确的职责层；不要借任务之机升级依赖、改迁移或配置、重构无关模块、扩大
可见性或顺手修复另一个问题。

本次任务若只要求文档，默认只改文档和必要的链接，不改源码、Cargo 清单、锁文件、
构建脚本、CI 或资源。若发现文档与实现不一致，先以代码和事实源为准，再在本次
范围内更新最小必要的说明。

### 主要职责边界

| 区域 | 负责内容 | 不应放入 |
| --- | --- | --- |
| `src/app.rs`、`src/startup.rs` | GPUI 启动、globals、字体、窗口、renderer 和启动策略 | 页面业务、通用 GPUI 默认值 |
| `src/ui` | GPUI 页面、窗口、组件、覆盖层、UI 状态和交互协调 | HTTP 客户端、持久缓存、解析/解码、下载、归档、Minecraft 领域逻辑 |
| `src/core` | Minecraft、CurseForge、EasyTier、在线、注入、版本和平台领域逻辑 | 页面实体和具体 UI 组件 |
| `src/downloads`、`src/archive`、`src/tasks` | 下载、解压、完整性、运行时、任务状态和后台工作流 | render 内的调度和实时后台锁读取 |
| `src/http` | HTTP 请求封装和代理 | 页面专用网络实现 |
| `src/plugins` | 插件 manifest、运行时、事件、watcher、UI DSL、插件窗口与受限 sidecar 桥接 | GPUI 框架对 BMCBL 业务的依赖 |
| `src/i18n`、`src/assets`、`src/utils` | 本地化实现、嵌入资源辅助和通用工具 | 具体页面编排或跨层业务聚合 |
| `crates/gpui` | 通用 GPUI 框架、窗口、输入、布局、渲染和并发原语 | BMCBL routes、assets、默认背景、下载服务和窗口策略 |

修改 `crates/gpui`、`src/app.rs` 或 `src/ui` 顶层前，必须阅读
`docs/ARCHITECTURE_BOUNDARIES.md`；修改后台运行时、任务、下载、归档、长期 core
工作流或后台到 GPUI 的传播链路前，必须完整阅读
`docs/ASYNC_RUNTIME_MODEL.md`。

## Workspace 与应用入口

`Cargo.toml` 是 workspace 成员和 feature 的权威来源。根 package 使用 Rust 2024，
库入口为 `src/lib.rs`，二进制入口为 `src/main.rs`，构建脚本为 `build.rs`。
不要把 `crates/*` 下的每个目录都假设为 workspace member；当前 workspace 同时包含
应用、GPUI、渲染、世界数据和网络相关 crate，并排除了部分上游/供应商目录。

主要 workspace crate 分组如下：

- GPUI 与运行时：`crates/gpui`、`crates/gpui_tokio`、`crates/egpui`、
  `crates/egpui-build`、`crates/egpui-manifest`。
- Bedrock 数据与渲染：`crates/bedrock-leveldb`、`crates/bedrock-world`、
  `crates/bedrock-render`。
- 插件与 UI 支持：`crates/gpui-hooks`、`crates/lucide-gpui`、
  `crates/bmcbl-plugin-api`、`crates/bmcbl-plugin-macros`。
- 网络与连接：`crates/easytier-bmcbl`、`crates/nethernet`、
  `crates/raknet/raknet-tokio`。
- 图形抽象：`crates/nova-gfx/` 下的 gfx-* crate 和示例。

`vendor/` 存放被 patch 的第三方依赖；`crates/easytier`、`crates/raknet` 和
`vendor/sctk-adwaita` 当前不属于 workspace member。依赖、feature、平台条件和
patch 关系以 `Cargo.toml` 为准。

### 应用层模块

```text
src/
├── main.rs / lib.rs       二进制薄入口与库模块装配
├── app.rs / startup.rs    GPUI 启动、窗口、globals、字体和早期启动编排
├── launch.rs / result.rs  Minecraft 启动与统一结果类型
├── config/                配置模型、默认值、存储和测试辅助
├── core/                  非 UI 领域逻辑
├── downloads/             下载引擎、完整性和下载运行时
├── archive/               归档/解压
├── tasks/                 后台任务管理、运行时和任务快照/事件
├── http/                  HTTP 与代理
├── plugins/               插件运行时和窗口/事件桥
├── i18n/                  本地化实现
├── assets/                应用侧 AssetSource 和生成资源辅助
├── utils/                 日志、诊断、网络、文件、更新和系统工具
└── ui/                    GPUI 页面、窗口、组件、状态和覆盖层
```

`src/core` 当前主要按 `minecraft`、`curseforge`、`easytier`、`inject`、`online`、
`version`、`sponsors` 和 `ui_prefs` 组织。下载、归档和任务模块的长期工作流不应
下沉到页面或通用组件。

## UI 结构与放置

`src/ui/README.md` 是 UI 细分结构的优先事实源。顶层职责如下：

- `main_window/`：主窗口 background、chrome、controls、page registry、loading、
  route effects 和 update flow；它是组合/协调层。
- `views/`：`home`、`download`、`manage`、`settings`、`tasks`、`tools` 和
  `plugin` 路由页面；页面专用状态和 widget 靠近对应页面。
- `window/`：独立工具窗口，包括 debug、import、level.dat、map viewer、plugin
  和 skin pack；窗口根只负责装配和生命周期。
- `components/`：无页面依赖的可复用视觉组件，如 button、input、modal、tabs、
  markdown/html renderer、split pane、virtual list、toast 等。
- `state/`：跨页面 UI 状态，包括 navigation、launcher、i18n、theme、update、
  diagnostics、agreement、local versions 和 quit；持久业务状态不放这里。
- `theme/`、`overlays/`、`runtime/`、`hooks.rs`/`hooks/`、`navigation.rs`、
  `animation.rs`、`update_check.rs`：主题、覆盖层、根视图装配、hooks、路由和动画/
  更新辅助。

页面或窗口 root 应主要组合布局、生命周期、订阅和 `Render` 实现。出现 state model、
IO/cache、解码、后台任务、输入行为和多个面板混在一个文件时，按职责拆到 sibling
module；优先按类型族/变更理由组织，不要一函数一文件。`curseforge`、`map_viewer`
和 `skin_pack` 是高复杂度区域，新逻辑应放到已有的职责子模块。

组件必须通用，不能依赖具体页面；`src/core` 不得依赖具体 UI 页面。渲染阶段只投影
GPUI 自己拥有的稳定快照，不获取 task manager/service 的跨线程锁，不启动/取消后台
工作，也不执行网络、文件、解析、解码或持久化操作。状态改变后由前台消费者更新
Entity/Global 并调用 `cx.notify()`。

使用当前 GPUI API：`App`、`Context<T>`、`Window`、`Entity<T>`、`WeakEntity<T>`、
`Render` 和 `RenderOnce`。新代码使用 async closure 形式的 `cx.spawn(async move |cx| ...)`；
不要使用已废弃的 `Model`、`View`、`AppContext`、`ModelContext`、`WindowContext` 或
`ViewContext`。

## UI 动画、帧时钟与 retained rendering 硬规则

这部分是 **BMCBL 全仓应用层规则**，不仅适用于 `src/ui`。任何位于 `src/plugins`、
onboarding、独立窗口、主窗口、UI DSL 或其它应用模块中的 GPUI render/helper，只要
会产生当前帧可见输出，都必须遵守本节。框架内部细则见 `crates/gpui/AGENTS.md` 和
`crates/gpui/docs/animation_clock.md`。

修改 animation、theme interpolation、layout motion、retained rendering、text
positioning、frame scheduling 或 Nova-facing visual state 前，必须阅读
`docs/GPUI_ANIMATION_CONVENTIONS.md` 与 `docs/GPUI_VENDOR_RENDERING.md`。

### 一个 platform frame 只能有一个视觉动画时间样本

当前帧可见动画采样必须使用：

```rust
let now = window.animation_time();
```

以下内容只要用于构建当前帧可见输出，就必须使用同一个 `now`：

- `raw_progress` / `eased_progress`；
- `SpringValue::sample/value/is_animating`；
- `ThemeState::factor/is_animating`；
- opacity / translation / scale / rotation / transform；
- clip / reveal / visual bounds；
- layout animation geometry；
- navigation、modal、dropdown、tabs、toast、loading pulse 等 UI motion；
- CPU retained state 与 renderer/Nova animation state 的比较或采样。

禁止在 `Render`、`RenderOnce`、`Element::request_layout`、prepaint、paint，或这些路径
调用的 helper 中使用新的 `Instant::now()` / `std::time::Instant::now()` 来计算当前帧
可见状态。父元素用 `window.animation_time()`、child/helper 再自己调用
`Instant::now()` 同样属于 bug。

helper 不能因为签名里没有 `Window` 就偷偷使用实时钟。像 `theme_colors(cx)`、
`current_theme_colors(cx)`、`detached_theme_colors(cx)`、`render_*()`、
`build_render_model()`、`sync_*_animation()` 这类函数，如果输出依赖当前帧时间，必须由
调用者显式传入 `now: Instant`、`&Window` 或已经计算出的 visual sample。

### 真实 monotonic clock 仍然有自己的职责

以下语义继续使用 `Instant::now()`，不要机械替换成 frame clock：

- click/key/pointer 等真实输入事件发生时间；
- animation start / retarget 的事件时间锚点；
- timer/deadline/retry/timeout；
- frame throttle、backpressure、watchdog、recent-input age；
- profiling / elapsed measurement；
- task/I/O bookkeeping 和不属于当前帧视觉采样的后台逻辑。

原则是：**visual sample 用 frame clock；event/scheduler/profiling 用 monotonic clock。**

### 文字与 subtree animation

visual-only animation 不得通过改变祖先 `top/left/height/width/padding/margin` 让文字
layout origin、wrapping width、baseline 或 glyph subpixel variant 每帧变化。应优先使用：

```text
final/stable child layout
    -> retained clip or composite layer
    -> inherited subtree animation binding
    -> renderer/GPU presentation
```

Text / SVG / Image / Quad / Shadow 等属于同一个视觉对象时，优先让它们继承同一个
retained/composite animation owner，而不是给每个 glyph/child 单独重算运动。

### invalidation 与 frame scheduling

- 使用最窄的 retained target；child hover/active/focus/animation 不应无理由扩大到整个 view。
- 不得把 `cx.refresh_windows()` 当通用动画时钟。
- 不得重新引入全局 recent-input / frame-backpressure bypass。
- interactive animation 使用 per-target coalescing / latest-wins；不要排队旧动画样本。
- static sibling 与 static text 在输出未变化时必须保持 retained reuse。
- generic view invalidation 不应无理由清除兼容的 targeted replay plan。

### UI/render 修改完成前必须审计

不要只检查 `render()` 本体，必须沿 helper 调用链检查。至少搜索受影响路径中的：

```text
Instant::now()
std::time::Instant::now()
raw_progress(
eased_progress(
.sample(
.value(
.factor(
.is_animating(
theme_colors(
current_theme_colors(
request_animation_frame
request_layout_animation_frame_if
with_layout_animation_target
```

每一个时间读取都要按语义分类。存在 `Instant::now()` 不等于一定错误；**用 fresh
monotonic sample 计算当前帧 visual output 才是错误。**

动画/渲染性能修复保持小提交、可 A/B；不使用 GitHub CI 作为自动修复循环，由维护者
本地 build/profile/验证。

## 异步运行时与 GPUI 状态契约

### Runtime ownership

现有 BMCBL 下载、归档、任务管理和长期工作流由 `src/tasks/runtime.rs` 的
`AppRuntime` 统一拥有。业务模块只选择语义化 API，不创建物理 runtime、blocking pool、
Rayon pool 或额外全局 executor：

- `spawn_io`：网络、timer、进程和 orchestration；
- `run_io_blocking`：阻塞文件/平台调用；
- `spawn_download_task` / `spawn_download_blocking`：下载工作流及写入；
- `spawn_archive_task` / `run_archive_blocking`：归档/安装及解压；
- `run_cpu` / `install_cpu`：应用 CPU/Rayon 工作；
- `gpui_tokio::Tokio::spawn_result`：有界、view-scoped 的 Tokio 请求结果。

这些 runtime 调度函数的 `blocking` 描述的是**调度执行方式**，不是领域 API 的同步版本，
因此不属于下文“同步 API 禁止 `_blocking`”的范围。领域 API 不应通过这类 runtime 名称
反向决定自身命名。

新的 egpui host 使用 `egpui::ApplicationRuntime`/`RuntimeProvider`；不要把这套
新 host 生命周期与现有 BMCBL AppRuntime 工作流混成一个 runtime owner。

禁止在生产业务或 UI 代码中构造 `tokio::runtime::Runtime/Builder`、调用
`Handle::try_current()` 探测环境、从 GPUI 页面直接调用 `tokio::spawn` 或
`tokio::task::spawn_blocking`、构造 Rayon `ThreadPool`，或用 `std::thread::spawn`
作为通用兜底。只有文档明确的 Windows hook、进程退出 watchdog 或阻塞 foreign callback
等平台生命周期例外可以使用专用 OS thread。

### Task 与事件

- 只有 `completed`、`cancelled`、`error` 是终态；未知状态必须按活动状态处理。
- 等待子任务使用 `wait_for_task_terminal()`，不要枚举已知“运行态”推断完成。
- 持久工作流不能由 GPUI `Task` 拥有；生产者只能发布 `Send + 'static` 的纯事件/快照，
  不得捕获 `App`、`Context<T>`、`Window`、`Entity<T>` 或 render 元素。
- 有界请求可由 `gpui_tokio` 桥接；持久工作流应由 AppRuntime 运行并通过事件/快照回传。
- Domain stream 的 channel、滞后恢复和关闭语义由领域模块/适配器负责；页面不要手写
  `recv -> Entity::update -> cx.notify()` 循环。view-scoped Entity 使用
  `Context::spawn_stream`，应用生命周期 Global 使用 `App::spawn_stream`。
- 生产者不修改 GPUI 状态；前台消费者负责更新 Entity/Global、处理错误并通知重绘。
- 只有外部系统无法产生事件时才允许轮询，并且要明确间隔、单实例、去重、过期结果拒绝、
  teardown 取消和错误时保留最后快照。

## Rust、平台与代码质量

涉及 Rust、Cargo、模块、async、并发、测试、lint 或 API 设计时，优先使用
`rust-design-conventions` skill，并按其场景路由只读取必要参考文件。

### Dev 阶段 API、命名与验证债务

当前项目处于 dev 阶段，公开 API 以单一的新接口为准。删除或重命名接口时，默认直接
迁移全部仓库内调用方；除非用户明确要求兼容已发布版本，否则不得保留旧名称的 type
alias、兼容 re-export、deprecated wrapper、隐藏转发函数或双入口。公开 re-export 只能
保留一条权威路径，不能让旧名和新名同时可用。

Rust 命名整改必须遵循 [`docs/RUST_NAMING_CONVENTIONS.md`](docs/RUST_NAMING_CONVENTIONS.md)。
判断名称时把完整模块路径、receiver、参数类型和可见性一起考虑，优先采用 Rust 标准库、
成熟生态和上游项目的既有术语，并选择在该边界内最短且无歧义的名称。父模块已有上下文时
不得在叶模块重复前缀，例如使用 `app::events`、`app::windows`、`img::loader`，不扩写为
`event_observers`、`window_registry`、`image_asset_loader`。`State`、`Context`、`Options`、
`Config`、`Error` 等 Rust 惯用角色名在职责明确时允许使用，不能机械禁用或替换为另一种
泛词。命名需要增加单词时，新增部分必须表达真实、稳定且无法由路径或类型推断的区别。

### 全仓同步与异步 API 命名

这条规则适用于 BMCBL 根应用和全部 Rust crate，不只 `bedrock-world`。

**默认同步 API 使用 canonical 短名称，不添加 `_blocking`、`_sync` 或其它线程模型后缀。**
同步调用是普通 Rust API，不应被命名成特殊变体：

```rust
world.chunk(pos)
world.block_state(dimension, pos)
player.inventory()
archive.extract(...)
client.request(...)
```

禁止把普通同步 API 命名成：

```rust
chunk_blocking(...)
block_state_sync(...)
get_player_blocking(...)
read_level_dat_blocking(...)
```

当同一领域操作确实同时提供同步和返回 `Future` 的异步版本时，**异步版本可以并优先使用
`_async` 显式表达异步边界**：

```rust
world.chunk(pos)?;
world.chunk_async(pos).await?;

world.players()?;
world.players_async().await?;
```

`async fn` 本身虽然在签名中可见，但调用点通常看不到函数声明；`_async` 在存在同步同名
语义时提供有价值的调用侧区分，因此不是应被机械删除的实现细节。不要为了隐藏 `async`
强行创建无领域价值的 `io()`/`async_world()` facade；只有职责本身确实独立时才引入新类型。

如果所谓异步 API 只是 `tokio::task::spawn_blocking` 对同步实现的适配，文档必须明确：
它只是异步调度适配器，不代表底层 LevelDB、文件系统或第三方 API 已变成原生 async I/O。

`*_blocking` 仍可用于**运行时/执行器调度原语**，例如 `run_io_blocking`、
`spawn_download_blocking`，因为这里 `blocking` 描述的是“把闭包投递到阻塞执行资源”这一
稳定操作语义，而不是给某个领域同步 API 加后缀。两类名称不得混淆。

### Getter、集合、Options 与控制流命名

普通 getter 不机械使用 `get_`。receiver 和模块已经表达对象时直接使用名词，例如：

```rust
world.chunk(pos)
world.player(id)
world.block_state(dimension, pos)
```

只有真正的 keyed lookup / map-like API 或上游固定术语才保留 `get` / `get_mut`。
集合读取优先使用集合名，例如 `players()`、`chunk_positions()`，而不是
`list_players()`、`list_chunk_positions()`。

不要通过 `with_options`、`with_control`、`if_*`、`using_*` 等函数名排列组合参数。
可配置行为使用 typed options / conditions，并保留一个 canonical operation。禁止把整个
控制流写进函数名，例如：

```rust
prepare_block_edits_if_primary_states_match_blocking(...)
get_many_ordered_with_control(...)
audit_world_integrity_blocking(...)
```

应收敛为 operation + typed argument，例如：

```rust
prepare_block_edits(..., conditions, options)
audit(..., options)
```

`_repo`、`Repository`、`Service`、`Manager`、`Controller`、`Helper`、`Helpers`、
`Operations`、`Utils`、`Common` 以及无实际语义的 `Data`、`Info`、`Result`、`Record`
不能作为默认架构词。只有这些词确实表达稳定职责、外部协议/上游对象或 Rust 惯用角色时
才能保留。删除单词后如果不会丢失真实语义，就应该删除。

### Minecraft Bedrock 领域命名

Minecraft 相关代码优先使用 Mojang/Minecraft Bedrock 已存在的领域名称和实际持久化名称，
其次才采用通用软件术语。稳定领域词包括 `World`、`Dimension`、`Chunk`、`SubChunk`、
`Block`、`BlockState`、`BlockEntity`、`Biome`、`Entity`、`Actor`、`Player`、`Item`、
`Structure`、`LevelDat`、`LevelDB`、`LegacyTerrain`、`Data2D`、`Data3D`，以及真实 Bedrock
key/record 名称如 `digp`、`actorprefix`、`SubChunkPrefix`。

不要把这些领域对象改写成 `WorldRepository`、`PlayerService`、`ChunkManager`、
`BlockStateHelper` 之类软件分层名，也不要在 crate/父模块已经提供上下文时重复 `Bedrock`、
`World`、`Query`、`Storage` 前缀。例如在 `bedrock-world` crate 内优先：

```rust
World
OpenOptions
WorldFormat
BlockState
ChunkPos
```

而不是：

```rust
BedrockWorld
BedrockWorldOpenOptions
BedrockWorldChunkResult
```

只有在更宽的真实公共命名空间中会发生语义冲突时才增加限定词。

`bedrock-world` 中原始 Bedrock key/value、backend adapter 和事务契约统一属于 `storage`；
`database` 不是 Minecraft 世界领域边界。具体 Mojang LevelDB 引擎实现属于
`bedrock-leveldb`，应使用 `LevelDB`、WAL、manifest、table、batch 等真实技术术语，不能
把 Minecraft `World` 语义下沉到数据库引擎 crate。

玩家是独立公共 Minecraft 领域，不应把解析、枚举、saved-item compatibility、持久化来源
等能力散落为 `world/player_*` helper。`player` 模块应完整表达 `level.dat.Player`、
`~local_player`、`player_*` 等真实来源；`World` 只负责 world lifecycle 与跨领域协调。

### Bedrock 写入与并发边界

读取允许同步或异步执行，异步读取不得因此获得直接覆盖 authoritative storage 的权限。
所有基于旧快照产生的写入都必须在最终 mutation/transaction 边界验证 source state/version，
避免 lost update。

`~local_player`、`player_*`、map、chunk、entity 等同属 LevelDB 的修改应能够进入同一个
`WorldTransaction` / `StorageBatch`，从而共享一个提交顺序和 LevelDB 原子 batch。玩家对象
应保留读取时的 source snapshot；更新时序列化当前结构化值，并在提交锁内验证 source 仍然
匹配，不能把旧 raw bytes 当成编辑后的值写回。

`level.dat.Player` 位于独立的 `level.dat` 文件，不属于 LevelDB，不能伪装成与 LevelDB
同一个物理原子事务。涉及 `level.dat` 与 LevelDB 的跨存储迁移/写入必须明确顺序、冲突检测、
失败恢复和外部 Minecraft 进程并发边界。进程内锁只能协调本进程针对同一世界的写入，不能
宣称可以阻止游戏本体或其它进程同时修改世界。

### 公共 API 文档

新增或整改 public type、trait、enum variant、public field、function、method 时必须补齐真实
契约文档，不能只把名称换成一句英文。涉及存储、解析、修改或异步的 API，按适用范围说明：

- 对应的 Minecraft/Bedrock 记录、文件或协议来源；
- 是否只读，是否修改 LevelDB 或 `level.dat`；
- 是否保留未知/未来字段与原始 bytes；
- 原子性边界以及是否参与 transaction/batch；
- 并发冲突与 lost-update 保护；
- 是否发生版本转换，若不发生也要明确；
- async 是否只是阻塞实现的调度适配；
- `# Errors` 中列出有业务意义的错误类别。

禁止 `/// Get X.`、`/// Put X.`、`/// Foo blocking.` 这类不增加契约信息的注释。

API 删除、重命名或职责移动完成前，必须使用 `rg` 检查并同步更新以下位置，不能只让
library target 通过：

- `src/` 中的实现、公开文档注释和 re-export；
- `tests/`、`benches/`、examples、README、迁移文档及开发文档；
- feature-gated 代码和测试。`#[cfg(feature = "...")]` 必须与对应 `Cargo.toml` 中的真实
  feature 名完全一致，不得用不存在的 feature 隐藏编译失败；
- 受影响 crate 的 `cargo test --all-features --no-run` 与
  `cargo bench --all-features --no-run`。必要时还要验证 no-default-features 路径。

测试和 benchmark 是正式调用方，不是可延后维护的样例。发现失效 API、错误 re-export、
被错误 feature 隐藏的测试或无法编译的 bench 时，先恢复验证基线，再进行行为修复或文件
拆分。纯机械模块拆分应与行为变化分阶段完成，避免同一 diff 同时改变公开 API、存储语义
和文件布局。

针对真实 Minecraft 世界的兼容性测试和 benchmark 必须读取只读快照，不得直接写入原始
世界目录；结果需记录 fixture 路径标识或 hash、记录数、区块数、解析错误数和缓存条件，
避免不可复现的性能结论。

- 保持 Rust 2024、现有 workspace lints（`unsafe_code = "warn"`，Clippy `all`/
  `pedantic` = `warn`）和平台条件；不要无理由升级依赖或修改 feature matrix。
- 优先借用，避免为了通过 borrow checker 添加 clone；参数和变量使用完整、有领域意义的
  名称。可恢复失败用 `Result`/`Option` 表达；生产代码不使用 `unwrap()`，`expect()`
  只能用于有明确不变量的场景。
- 不用 `let _ =` 静默丢弃 fallible 操作；错误应传播、带上下文记录或转化为可见 UI 状态。
- 新模块使用标准 Rust 模块系统；`name.rs` 与 `name/mod.rs` 均可按职责选择，不机械禁止
  `mod.rs`，也不为了偏好某种布局引入 `#[path]`。
- 新增公共 API 前评估所有调用方、错误语义、可见性、平台/MSRV 和 breaking change。
- 函数超过约 50 行、实现文件超过约 500 行、参数超过 5 个或职责混杂时，先拆分到
  合理的类型族/职责模块；不要通过 helper 碎片化掩盖边界问题。
- 文件系统/进程/环境值使用 `Path`、`PathBuf`、`OsStr` 等跨平台类型；平台差异使用
  `cfg` 和目标特定模块隔离。

### Rust 模块、代码生成与文本包含

普通 Rust 模块必须使用标准模块系统：`mod name;` 或确实需要作为公共 API 时使用
`pub mod name;`，并让 rustc 按 `name.rs` 或 `name/mod.rs` 规则解析文件。不要为了绕过
目录结构、可见性、导入顺序或循环依赖使用 `#[path = "..."]`、`include!("*.rs")` 或
其它文本拼接方式导入手写 `.rs` 文件。

`#[path = "..."]` 只允许用于确有必要的测试/构建/平台隔离边界，例如测试 fixture、
构建生成代码接入，或确实无法用普通 `cfg` 模块表达的目标平台 shim，并且必须在代码旁
说明原因。普通源码、UI、core、renderer、world、LevelDB、player 和插件功能模块不得使用
它代替标准模块声明。

`include!` 只允许包含构建期生成代码，例如 `build.rs`、proc-macro 或 protobuf/codegen
输出到 `OUT_DIR` 后通过 `include!(concat!(env!("OUT_DIR"), ...))` 接入；生成代码必须隔离
在小模块内，不得反向依赖业务层。仓库中 `crates/easytier/src/proto/*` 这类生成 proto
绑定属于历史/供应商生成边界，不能作为手写模块拆分的示例。

静态资源嵌入使用 `include_str!`、`include_bytes!` 或资源生成表，不使用 `include!`。
新增 `include!`、`#[path]` 或 `OUT_DIR` 代码生成前必须先确认普通模块、`cfg` 子模块、
build script 生成资源表或显式 trait adapter 无法满足，并在变更摘要中说明理由。

## 资源、本地化与日志

- `assets/` 是构建期输入；`src/assets` 是应用侧加载/生成辅助。`build.rs` 负责 Windows
  manifest、图标和 payload metadata。必须落盘的 payload 使用运行时目录：优先
  `%LOCALAPPDATA%\\BMCBL\\runtime\\...`，fallback 为 `%TEMP%\\BMCBL\\runtime\\...`。
- 只读资源优先使用 `include_bytes!`、`include_str!`、生成表或通用 GPUI `AssetSource`；
  BMCBL 资源名和默认背景策略留在应用代码，不放进 GPUI 框架默认值。
- `I18n` (`src/ui/state/i18n.rs`) 是 GPUI Global；翻译源在 `assets/locales/`，render
  通过 Global 读取翻译，语言切换更新 Global 并刷新受影响窗口。
- 日志通过应用日志桥接进入统一系统。不要新增 `RUST_LOG`、`GPUI_*`、`ZED_*` 或
  `BMCBL_*` 运行时配置开关；环境变量只用于操作系统、编译/构建、CI 或外部子进程协议。

## 文档、脚本与验证

仓库常用脚本位于 `scripts/`，包括 `check_gpui_layout.ps1`、`check_i18n_lang.ps1`、
启动性能分析、实体图标生成、发布/打包和 Windows/Linux setup 脚本。执行脚本前先
阅读其参数和副作用；临时补丁脚本不要当作通用构建入口。

按改动风险选择最小验证集：

```powershell
cargo fmt --all --check
cargo check --workspace --no-default-features
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

涉及 GPUI framework 时补充：

```powershell
cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
```

文档-only 变更至少检查引用路径和链接；未运行的验证必须在交付摘要中说明。Windows
仍是 BMCBL 应用的主要目标平台，但跨平台纯 Rust crate 的重构按受影响范围选择验证平台；
当前 `bedrock-world` 命名/结构整改以 **Linux Rust 1.95** 的 storage-crates 检查作为继续
下一批的默认门槛，不要求等待 Windows，除非改动涉及 Windows 专有实现或用户另行要求。

## Git 约定

提交信息遵循 [`docs/COMMIT_CONVENTIONS.md`](docs/COMMIT_CONVENTIONS.md) 的
Conventional Commits 与 Cocogitto hook。除非用户明确要求，不要自行创建提交、分支、
推送或 PR；提交前确认 diff 只包含当前任务范围内的文件。
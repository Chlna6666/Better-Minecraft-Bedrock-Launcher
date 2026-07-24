# egpui 桌面应用框架任务总纲

> 状态：架构与迁移任务主文档  
> 日期：2026-07-24  
> 适用范围：`crates/gpui`、`crates/egpui*`、BMCBL 接入层
> 目标平台：Windows、macOS、Linux  
> 实施策略：先在 BMCBL workspace 内完成和验证，再迁移到独立仓库维护

> 实施状态（2026-07-24）：Phase 1 已启动。`vendor/gpui` 已移动到
> `crates/gpui`，工作区依赖、Nova 相对路径、架构文档和说明文档已完成切换；
> 正式 `Cargo.toml`、`NOTICE` 和 Windows/Nova DX12 聚焦构建已完成。`gpui`
> 包名暂保留用于上游 API 兼容，应用框架层统一命名为 `egpui`。

## 1. 背景与结论

当前 GPUI 是高性能 GUI 框架，已经提供窗口、输入、元素、布局、文本、渲染、
Entity 状态和与平台事件循环集成的 GUI 异步执行器。它还不是完整的桌面应用框架。

BMCBL 目前自行承担了大量通用桌面应用职责：

- `src/app.rs` 负责应用身份、启动编排、字体、资源、全局状态和窗口注册；
- `src/tasks/runtime.rs` 负责 Tokio、Rayon、阻塞池、并发预算和后台任务；
- `src/assets` 与 `build.rs` 负责嵌入资源、图标和生成代码；
- `src/i18n` 与 `assets/locales` 负责 BMCBL 自有本地化；
- `build.rs` 直接生成 Windows manifest、图标、VERSIONINFO 和运行时资源；
- 更新、单实例、诊断、配置、平台注册等能力分散在 BMCBL 业务代码中。

目标不是把这些 BMCBL 实现原样塞进 GPUI，也不是把 GPUI 改成 Tauri 的复刻。
目标是在 GPUI GUI 核心之上增加一个独立、可组合、可测试的桌面应用层：

```text
Application code
  -> Desktop application framework
     -> lifecycle, services, runtime, resources, platform integration
     -> build, bundle, signing, updater, diagnostics, testing
  -> GPUI GUI core
     -> entities, windows, input, layout, text, scene, renderer
  -> nova-gfx or another renderer backend
     -> DX12, Vulkan, Metal, headless
```

核心决定：

1. `gpui` 继续是上游兼容的 GUI 核心，不拥有业务后台工作流。
2. `egpui` 是独立的桌面应用框架层，负责宿主、资源、i18n、平台和发布能力；应用提供自己的 catalog 与消息 key。
3. 应用后台异步与 GPUI GUI 异步是两个执行域，使用明确桥接 API 兼容。
4. `nova-gfx` 是可替换的图形后端，不暴露为应用框架的公共语义。
5. 资源、应用身份、i18n、平台注册、打包和更新进入新的应用框架层；BMCBL 的产品消息仍由 BMCBL 维护，但通过 egpui i18n 接口接入。
6. BMCBL 只保留 Minecraft、下载、联机、插件等产品领域能力。
7. 新框架先在 `crates/` 中完善，但从第一天禁止依赖 `src/` 或 BMCBL 产品类型。

## 2. 目标与非目标

### 2.1 目标

- 开发者可以从模板快速创建可运行、可打包、可本地化的 GPUI 桌面应用。
- 应用元数据只声明一次，并能生成各平台所需的身份、图标和注册信息。
- 提供专业的应用生命周期、后台服务宿主、结构化并发和优雅关闭。
- 提供确定性资源管线、配置目录、安全存储和 domain-neutral i18n 抽象。
- 提供 Windows、macOS、Linux 的构建、打包、签名和更新扩展点。
- 提供系统托盘、菜单、通知、单实例、深链接和文件关联等桌面能力。
- 提供日志、诊断、崩溃处理、测试替身、无头测试和打包验证。
- 允许替换应用异步运行时、渲染后端和平台服务实现。
- 保证 GPUI 上游版权、Apache-2.0 许可和衍生修改归属清晰。
- BMCBL 完成接入后，可以将框架 crates 连同历史迁移到独立仓库。

### 2.2 非目标

- 不把 Minecraft、下载、EasyTier、BMCBL 路由或产品 UI 加入框架。
- 不要求 GPUI 核心依赖 Tokio、Rayon、Fluent、ICU 或具体打包器。
- 不把 GUI Entity 当作后台业务状态容器。
- 不保证第一阶段支持移动端。
- 不在第一阶段提供稳定 Rust 动态库 ABI。
- 不复制 Tauri 的 WebView IPC 模型或 WPF 的 XAML 模型。
- 不自研已有成熟实现可以可靠覆盖的安装包格式、加密算法或 Unicode 规则。

## 3. 面向三类使用者的质量目标

### 3.1 最终用户

- 启动、退出、更新和卸载行为符合操作系统习惯。
- 窗口在高 DPI、多屏、睡眠恢复、主题切换后保持正确。
- 应用空闲时不持续重绘，不因后台任务阻塞 UI。
- 安装包有正确名称、版本、发布者、图标、签名和卸载信息。
- 更新包必须验证签名，失败后保留可运行版本。
- 语言回退、复数、区域格式、RTL 和辅助功能行为可预测。
- 崩溃与后台失败有可理解的反馈，不静默丢失数据。

### 3.2 应用开发者

- 一个声明文件描述应用身份、窗口、资源、语言和打包目标。
- `new`、`dev`、`check`、`build`、`bundle`、`doctor` 命令行为一致。
- GUI 任务、后台任务、阻塞任务和 CPU 任务的 API 名称能表达语义。
- 生命周期、取消、错误、重试和关闭规则有类型与文档约束。
- 平台差异通过 capability 查询和 `Unsupported` 错误暴露，不伪装成功。
- 默认配置能运行，高级能力通过 feature、插件或显式配置启用。
- 构建错误能指出资源、翻译、图标、签名或平台配置的具体位置。

### 3.3 框架维护者

- GUI 核心、应用层、构建层、平台层和打包层依赖单向。
- 平台实现可单独测试，公共 API 不泄漏 Win32、Cocoa 或 Linux 后端类型。
- 上游 GPUI 同步与本地修改可审计，不覆盖上游版权。
- 可对启动、空闲、帧调度、资源加载和任务延迟做基准测试。
- crate 可独立执行 `cargo metadata`、`cargo check` 和 package 预检。
- 发布、SemVer、MSRV、feature matrix、SBOM 和许可证有固定流程。

## 4. 主流框架对比与采用原则

| 能力 | Tauri 2 | Qt 6 | WPF/.NET | 本项目采用方式 |
| --- | --- | --- | --- | --- |
| GUI/渲染 | 系统 WebView | Qt Widgets/QML | XAML + DirectX | GPUI + 可替换 renderer |
| 应用生命周期 | Builder、AppHandle、插件 | QCoreApplication/QApplication | Application + Generic Host 可组合 | `ApplicationHost` + 生命周期事件 |
| UI 线程 | WebView 事件循环 | QObject 线程亲和性 | Dispatcher 线程亲和性 | GPUI foreground executor |
| 后台异步 | Rust async commands/runtime | QThreadPool/QtConcurrent | Task/ThreadPool/HostedService | 独立 `AppRuntime`，不属于 GPUI |
| 资源 | 配置式 bundle resources | qrc/rcc | Resource/Content/Pack URI | 可组合资源包 + 确定性索引 |
| i18n | 前端/插件生态 | QTranslator/Qt Linguist | resx/卫星程序集/XAML 本地化 | egpui 提供 Fluent runtime、BCP 47 fallback 与 UI snapshot；应用拥有 catalog |
| 应用身份 | 配置、图标、bundle 元数据 | QCoreApplication 元数据 + 部署工具 | Assembly/MSBuild/manifest | 单一 `AppManifest` |
| 打包 | CLI + bundler + store 指南 | 平台部署工具 | MSIX/MSI/ClickOnce 等 | 后端适配，优先复用成熟 bundler |
| 更新 | 签名 updater 插件 | 通常由应用/部署系统选择 | MSIX/ClickOnce/自定义 | 独立签名更新模块 |
| 权限安全 | capabilities/permissions | 原生进程权限 | OS/.NET 安全边界 | 插件与危险 API 最小能力声明 |
| 开发工具 | scaffold、CLI、schema | CMake、Designer、Linguist | Visual Studio/MSBuild | Rust CLI、schema、doctor、模板 |
| 平台范围 | 桌面和移动 | 桌面和移动 | Windows only | 桌面三平台，Windows 优先 |

采用原则：

- 学习 Tauri 的 crate 分层、配置 schema、打包、签名更新和插件能力声明。
- 学习 Qt 的应用对象、平台服务、资源系统、翻译工具链和部署完整性。
- 学习 WPF Dispatcher 的 UI 线程约束，以及 Generic Host 的服务和关闭模型。
- 不复制其渲染、语言绑定、WebView IPC 或特定对象模型。
- 能复用成熟底层库时先做适配层评估，不手写安装器、Unicode 或密码学。

## 5. 名称、上游关系与许可证

### 5.1 名称策略

- `gpui` 是来自 Zed Industries 的上游项目名称，兼容 fork 内可继续使用 crate 名。
- 对外必须写明“independent fork based on GPUI”，不得暗示由 Zed 官方维护。
- 新应用框架统一命名为 `egpui`，公开发布前仍需完成 crate namespace 和商标检索。
- 不使用 `Zed`、`Tauri`、`Qt`、`WPF` 作为新项目或 crate 品牌。
- 不建议把新应用框架命名为 `Nova`。该名称已被 macOS 编辑器和 Unity UI
  框架等软件使用，容易造成混淆。
- `nova-gfx` 暂时只作为内部图形子系统名称。独立发布前仍需检查 crates.io、
  GitHub、搜索引擎和目标市场商标，必要时重命名。

### 5.2 GPUI 版权与 manifest 强制规则

迁移后的 `crates/gpui/Cargo.toml` 必须是人工维护的正式清单：

- 删除“Cargo 自动生成”和“请查看 Cargo.toml.orig”的头部；
- 将当前有效依赖、target 依赖、features、examples 和 lints 合并进正式清单；
- 源码仓库不再保留、读取、生成或引用 `Cargo.toml.orig`；
- 允许 `cargo package` 在发布归档内部自动生成 `Cargo.toml.orig`，该文件只用于保存
  未规范化的人工清单，不得回写源码树或成为构建输入；
- 明确 `edition = "2024"`、`rust-version`、lib path、features 和 publish 策略；
- 在独立维护前避免依赖 BMCBL 根包的隐式 workspace 字段；
- 本地 path 依赖同时规划可发布的 version，避免只能在当前目录编译；
- `publish = false` 保持到名称、依赖发布顺序和上游声明全部通过评审；
- `repository` 指向实际 fork 仓库，不能把修改版错误指向 Zed 官方仓库；
- 用 `package.metadata.upstream` 或 README 明确上游仓库和基准 revision；
- 保留上游作者信息，另行标注当前 fork maintainers，不冒充原作者。

许可证与版权：

- 原样保留 `LICENSE-APACHE` 中的 Zed Industries 版权；
- 新增修改版权时使用追加声明，不替换或删除上游声明；
- 新增 `NOTICE`，记录上游 GPUI 来源、基准 commit、主要修改类别和独立维护声明；
- 新增 `THIRD_PARTY_LICENSES` 或自动生成的第三方许可清单；
- 审计带独立 SPDX/双许可证头的文件，保留其原始声明；
- 审计示例、字体、图像、shader 和平台代码是否允许再分发；
- README 和文档不得使用 Zed logo、品牌素材或暗示官方兼容认证；
- CI 执行许可证扫描、SBOM 生成和 `cargo package --list` 审核。

## 6. 目标 crate 与模块边界

第一阶段控制 crate 数量，避免过早碎片化：

```text
crates/
  gpui/                  # GUI 核心和平台窗口/渲染集成
  egpui/                 # 应用宿主、runtime、资源、平台服务
  egpui-build/           # manifest 解析、资源索引/嵌入和平台元数据
  egpui-cli/             # scaffold、doctor、build、bundle、icons
  nova-gfx/              # 可替换的底层图形抽象和后端
```

只有出现独立发布、依赖隔离或编译成本需求后，才从 `egpui` 提取：

- `egpui-resources`
- `egpui-runtime`
- `egpui-updater`
- `egpui-testing`

依赖方向：

```text
egpui-cli -> egpui-build
egpui-build -> manifest/schema types
application -> egpui -> gpui
gpui -> renderer abstraction -> nova-gfx backends
egpui -X-> BMCBL src
gpui -X-> egpui
gpui -X-> Tokio/Rayon/application services
```

## 7. 双异步模型

### 7.1 执行域

| 执行域 | 所有者 | 适用工作 | 生命周期 |
| --- | --- | --- | --- |
| GPUI foreground | `gpui` | Entity 更新、窗口、输入、渲染、UI timer | App/Window/View |
| Application async | `egpui` runtime | 网络、进程、业务编排、长期服务 | Application/task scope |
| Blocking IO | runtime provider | 文件、注册表、同步系统 API | operation scope |
| CPU pool | runtime provider | 解码、哈希、压缩、计算 | operation scope |
| Renderer/GPU | `gpui` + backend | frame upload、GPU commands、present | Window/device |

### 7.2 不变量

- `cx.spawn` 和 `window.spawn` 只表示 GUI 前台任务，不等于业务后台调度。
- 持久任务不能由 View 中保存的 GPUI `Task` 拥有。
- 后台 future、event 和 snapshot 必须是 `Send + 'static` 的纯数据。
- 后台线程不得持有或更新 `App`、`Window`、`Context<T>`、`Entity<T>`。
- 后台结果通过 `UiHandle::dispatch` 或 stream bridge 回到前台。
- 前台闭包是唯一可更新 GPUI 状态并触发 `notify` 的位置。
- 取消、超时、join error、panic 和业务失败是不同结果。
- 超时不表示阻塞操作已经停止，permit 必须持有到实际退出。
- render 不读取后台锁，不启动 IO，不轮询业务状态。
- framework API 不依赖当前线程恰好存在 Tokio context。

### 7.3 公共语义

需要设计并验证以下 API，名称可在 ADR 中调整：

- `ApplicationRuntime`: 应用执行器句柄，不泄漏具体 Tokio runtime；
- `RuntimeProvider`: 创建、启动和关闭执行域；
- `TaskScope`: application、service、window-request 等结构化任务域；
- `AppTask<T>`: 可等待、可取消、可观察终态的后台任务；
- `CancellationToken`: 协作取消；
- `UiHandle`: 从后台安全投递纯数据到 GUI 前台；
- `UiStreamBridge<T>`: 将 stream 绑定到前台消费者；
- `UiTaskBridge`: 高频进度使用 latest-wins，终态使用独立通道并优先投递；
- `DurableWorkflowCoordinator` / `DurableTaskHandler`: 注册应用处理器、
  重启恢复和终态 checkpoint 持久化，不拥有下载、归档等产品语义；
- `BlockingTaskOptions`: 分类、超时、并发预算和诊断标签；
- `TaskOutcome<T>`: `Completed(T)`、`Cancelled`、`Failed(E)`；
- `ShutdownToken`: 停止接收新任务并等待关键服务退出。

默认实现可以使用 Tokio 和 Rayon，但必须做到：

- Tokio/async-std/smol 等替代实现可以通过 provider 接入；
- `gpui` 本身不增加 Tokio 依赖；
- 应用代码通过语义 API 选择 IO、blocking、CPU 或 durable work；
- 并发预算集中配置，不能由各业务模块私建 runtime、pool 或 semaphore；
- 支持优先级、背压、任务命名、trace span 和 shutdown deadline；
- 后台服务启动失败必须阻止应用进入伪 Ready 状态或显式进入降级模式。

## 8. 应用宿主与生命周期

设计 `ApplicationHost`，将 GPUI `Application` 与应用服务组合，但不合并执行域。

生命周期至少包含：

```text
build
  -> validate manifest
  -> bootstrap platform
  -> initialize runtime and services
  -> initialize GPUI
  -> ready
  -> activated / opened / suspended / resumed
  -> shutdown requested
  -> stop accepting work
  -> drain or cancel services
  -> close windows and renderer
  -> finalize
```

任务：

- 定义一次性启动阶段和可重复的 activate/open 事件；
- 区分“最后窗口关闭”“用户退出”“系统注销”“更新重启”和“崩溃”；
- 支持可取消的退出请求和有 deadline 的最终关闭；
- 提供 `AppPlugin`/`Service` 注册接口，明确初始化和关闭顺序；
- 服务依赖使用显式构造或类型注册，不引入隐藏的全局 service locator；
- 提供命令行、环境、配置文件和平台激活参数；
- 提供未处理 panic/error hook，不允许恢复后继续运行已破坏的 UI 状态；
- 为睡眠、唤醒、锁屏、网络变化和主题变化提供平台事件；
- 支持 headless/service mode，而不初始化窗口和渲染器。

## 9. 单一应用清单

新增版本化 `App.toml` 或等价 manifest，作为应用元数据的唯一事实源：

```toml
schema_version = 1

[application]
id = "com.example.app"
name = "Example"
version = "0.1.0"
publisher = "Example Organization"
copyright = "Copyright (C) 2026 Example Organization"
default_locale = "en-US"

[runtime]
provider = "tokio"
shutdown_timeout_seconds = 15
ui_queue_capacity = 256

[resources]
embedded = ["assets/ui/**"]
bundled = ["assets/data/**"]

[i18n]
source_locale = "en-US"
locales = ["en-US", "zh-CN"]
catalog_pattern = "locales/{locale}/main.ftl"

[windows.main]
title = "Example"
width = 1100
height = 720

[bundle.windows]
execution_level = "asInvoker"

[bundle.icons]
source = "assets/icons/app.png"
windows_ico = "assets/icons/app.ico"
```

任务：

- 提供 JSON Schema 和严格反序列化，未知字段默认报错；
- schema 带版本和迁移工具；
- `Cargo.toml` 版本与 App manifest 版本冲突时构建失败；
- application id、publisher、文件名和平台标识符做平台级校验；
- 支持 debug/release 和平台 overlay，但合并顺序必须固定；
- 敏感值、签名密钥和 token 禁止写入普通 manifest；
- build script、CLI 和 runtime 共享同一套 schema types；
- 生成内容必须确定性，不嵌入当前时间等不可重现数据。

## 10. 资源系统

GPUI `AssetSource` 保持为 GUI 读取字节的低层接口。`egpui` 在其上提供应用资源：

- `ResourceId`: 规范化、无平台分隔符的逻辑资源标识；
- `ResourcePack`: embedded、bundled、development overlay、external；
- `ResourceResolver`: 按 namespace 和优先级组合多个 provider；
- `ResourceMetadata`: MIME、长度、hash、compression、scale；
- `ResourceHandle`: 避免无界复制，支持共享和缓存；
- build-time index: 检查重复、大小写冲突、非法路径和缺失资源；
- runtime directory: 通过平台标准路径解析，不依赖当前工作目录。

任务：

- 支持嵌入二进制与安装包外置资源，两者使用相同逻辑路径；
- 支持按阈值和文件类型选择压缩，已压缩图像默认不重复压缩；
- 大资源允许 memory map 或流式读取，不强制 `&'static [u8]`；
- 资源包带完整性 hash，可选签名；
- 开发模式可启用目录 overlay 和热重载，release 默认关闭；
- 资源 namespace 防止应用、框架、图标库和插件互相覆盖；
- 字体注册、fallback 和卸载由资源服务协调；
- 图标源可生成 Windows ICO、macOS ICNS 和 Linux PNG 尺寸集合；
- 删除 BMCBL `build.rs` 中带 UTC 时间戳的生成输出，保证可重现构建；
- 禁止静默 fallback 到源码目录，开发 fallback 必须显式开启。

## 11. i18n 架构与所有权边界

egpui 管理本地化运行时，但不拥有任何产品文案。应用通过 manifest 声明
`source_locale`、支持的 locale 和 catalog 路径，通过 `I18nService` 注册
Fluent catalog；消息 key、翻译内容和术语仍属于应用或插件。

运行时契约包括：

- `LocaleId` 使用规范化 BCP 47 标识，locale fallback 按 `zh-Hant-TW ->
  zh-Hant -> zh -> source_locale` 顺序执行；
- `I18nService` 支持消息值、选择器/复数、属性格式化和显式 RTL/LTR 方向；
- catalog 可来自 `ResourceResolver`，读取具有限制大小、UTF-8 校验和明确错误；
- locale 切换通过 `watch` 发布 `LocaleSnapshot`，UI 只消费快照并触发重绘；
- 不在 render 中读取锁、文件或异步服务，语言切换不会阻塞 GPUI foreground；
- ICU4X 作为后续区域数字、日期、货币和复杂分词扩展；Fluent 负责消息语法；
- catalog namespace 必须隔离框架、应用和插件，重复消息由构建/注册阶段发现。

BMCBL 可以继续维护 `src/i18n` 的产品适配和 key 组织，但应将加载、fallback、
格式化和 UI 状态传播切换到 egpui 接口；迁移期间只能保留一个实际语言状态源。

## 12. 平台服务

公共 API 返回 capability 或明确的 `Unsupported`，不得在不支持的平台空操作成功。

P0/P1 服务：

- application identity 和标准数据/cache/config/log/runtime 目录；
- clipboard、open URL/path、原生文件/消息对话框；
- 菜单、托盘、通知、badge/progress；
- 单实例与第二实例参数转发；
- 深链接、URL scheme、文件关联和 activate/open-file 事件；
- 全局快捷键；
- 启动项/autostart；
- 电源抑制、睡眠/恢复和会话结束；
- 系统主题、强调色、locale、辅助功能偏好；
- 凭据和 secret storage 抽象；
- 窗口状态持久化，多屏变化后安全恢复；
- 受控 sidecar/process 启动和退出。

后续插件：

- 打印；
- 摄像头/麦克风和屏幕捕获权限；
- OS 分享、搜索、recent documents；
- 数据库、HTTP、WebSocket 等非核心服务；
- dynamic/WASM plugin host。

## 13. 平台注册、打包与发布

### 13.1 Windows

- 从 App manifest 生成 application manifest、DPI、compatibility 和 execution level；
- 生成 ICO、VERSIONINFO、ProductName、FileDescription、CompanyName、
  OriginalFilename、版权和 build metadata；
- 设置并验证 AppUserModelID；
- 支持普通 exe、portable、MSI/NSIS，MSIX 作为独立后端评估；
- 支持开始菜单/桌面快捷方式、卸载信息、协议和文件关联；
- 支持代码签名、timestamp server 和签名验证；
- 通知 identity 在未安装和已安装模式下行为明确；
- 安装、升级、降级、修复和卸载均有 smoke test。

### 13.2 macOS

- 生成 `.app`、Info.plist、bundle id、版本、ICNS 和 document/url types；
- entitlements 由 capability 生成并可审计；
- 支持签名、hardened runtime、notarization、stapling；
- 支持 DMG 或 PKG 后端；
- 正确处理 activate、reopen、open files、open URLs 和 App Nap。

### 13.3 Linux

- 生成 `.desktop`、AppStream metainfo、icons、MIME 和 URL handler；
- 遵守 XDG data/config/cache/state/runtime 目录；
- 支持 Wayland 和 X11 capability 差异；
- 首批后端选择 AppImage + deb，rpm/Flatpak 后续；
- 处理系统托盘、通知和 secret service 可用性差异；
- 不假设桌面环境、systemd user service 或 X11 一定存在。

### 13.4 打包器策略

- 建立 ADR 比较复用 `tauri-bundler`、cargo-bundle 和平台原生工具；
- 通过 `BundlerBackend` 隔离实现，应用 API 不直接依赖某一打包器；
- 不 fork 安装包实现，除非已有方案无法满足且有测试能力；
- bundle 输出必须包含 manifest 摘要、文件列表、hash、SBOM 和许可证；
- 构建与打包可分离，支持 CI 在受控签名环境完成最终签名。

## 14. 安全、更新与供应链

安全任务：

- 对 shell、sidecar、文件系统范围、更新、secret、全局快捷键等危险能力显式声明；
- Rust 原生 View 默认不需要 Tauri 式 WebView IPC 权限，但插件和远程内容必须隔离；
- 外部 URL、路径、资源和翻译输入做规范化与边界校验；
- release 禁止从源码目录、当前目录或不可信网络隐式加载代码/资源；
- 敏感日志字段做 redaction；
- updater key、code signing key 与普通配置分离；
- CI 使用 `cargo-deny` 或等价工具检查 license、advisory、source 和 duplicate；
- 生成 CycloneDX 或 SPDX SBOM；
- 定义依赖更新、漏洞响应和安全公告流程。

更新任务：

- update manifest 含 version、channel、target、arch、URL、hash、signature、notes；
- production endpoint 强制 HTTPS；
- 更新包使用成熟签名算法和库，不自研密码学；
- 下载、验证、stage、安装、重启分阶段，状态可恢复；
- 支持 stable/preview/nightly channel，但版本身份与 channel 分离；
- 原子替换或平台安装器失败后保留旧版本；
- 支持取消、断点续传、磁盘空间检查和代理；
- 防降级策略可配置，显式 rollback 需要单独授权；
- updater 是后台应用任务，UI 只消费 snapshot/event；
- 关键状态可持久化，关闭页面不取消更新下载。

## 15. 配置、日志、诊断与崩溃

- 配置优先级固定：defaults < file < environment < CLI；
- unknown key 默认报错，配置迁移带 schema version；
- settings 写入采用原子替换和备份，不直接覆盖；
- 提供 typed path API，禁止业务代码拼接平台目录；
- tracing 初始化、filter、rolling files 和 console provider 可配置；
- 日志目录、保留期、最大体积和敏感字段策略明确；
- 提供应用、runtime、window、renderer、GPU、locale、resource pack 诊断快照；
- 提供用户可导出的诊断包，默认排除 token、路径隐私和个人数据；
- panic hook 捕获最小上下文并避免递归崩溃；
- crash reporter 为可选插件，遥测默认 opt-in；
- debug inspector 与 release diagnostics 权限分开。

## 16. 可访问性与桌面体验

- 保持 GPUI accessibility tree 与 Windows UIA、macOS AX、Linux AT-SPI 的路线；
- 所有标准控件具备 role、name、value、state、action；
- 键盘导航、焦点顺序、快捷键冲突和菜单 mnemonic 可测试；
- 支持高对比度、reduce motion、screen reader、text scale；
- i18n 切换后重新计算 layout、direction 和 accessibility labels；
- 原生窗口、托盘、通知和对话框符合平台交互规则；
- 自定义 chrome 不得破坏 resize、snap、系统菜单和辅助技术；
- 多窗口、模态、退出动画和焦点恢复遵循明确状态机。

## 17. CLI、模板和开发者体验

首批命令：

```text
egpui new
egpui dev
egpui check
egpui build
egpui bundle
egpui icons
egpui i18n check
egpui doctor
egpui inspect bundle
egpui licenses
```

任务：

- 生成最小应用、带导航应用和后台服务应用模板；
- 模板默认 Rust 2024、明确 lints、无 `unwrap()` 的生产路径；
- `doctor` 检查 Rust toolchain、平台 SDK、linker、签名和打包依赖；
- `check` 不构建安装包即可验证 manifest、icons、resources、locales；
- `dev` 支持资源/i18n overlay 和可选热重载；
- 错误输出带文件、字段、平台和修复建议；
- 提供 API 文档、概念指南、平台指南、迁移指南和 runnable examples；
- 示例不得依赖 BMCBL assets、routes 或服务。

## 18. 测试与质量门禁

### 18.1 测试层次

- unit：manifest、locale negotiation、paths、resource index、task state；
- compile test：feature matrix、public API、unsupported platform stubs；
- integration：runtime shutdown、UI bridge、service failure、resource overlay；
- headless GUI：窗口生命周期、焦点、输入、accessibility snapshot；
- platform smoke：identity、dialogs、tray、notifications、single instance；
- package smoke：安装、启动、升级、卸载、签名验证；
- chaos：channel lag、runtime shutdown、disk full、corrupt update、sleep/resume；
- benchmark：startup、idle CPU、memory、first window、frame time、task latency。

### 18.2 CI 矩阵

- Windows x86_64：DX12 和 Vulkan；
- Windows aarch64：至少 compile，具备环境后执行 smoke；
- macOS aarch64：Metal；
- macOS x86_64：按 CI 可用性决定 compile 或 smoke；
- Linux x86_64：Wayland、X11、Vulkan、headless；
- `--no-default-features` 与受支持 feature 组合；
- MSRV 与 stable Rust；
- package、docs、licenses、SBOM、format、clippy、tests。

### 18.3 性能门禁

先建立基线，再定义绝对预算。第一阶段至少要求：

- 空闲窗口保持事件驱动，不出现持续 frame loop；
- 后台任务压力下 UI input/frame latency 有自动化报告；
- 新框架不得让 BMCBL first-window p95 回退超过商定阈值；
- 每个 resource pack 和 locale data 的体积可见；
- bundle size、startup、idle CPU、peak memory 进入 CI 趋势；
- 性能声明必须有对应 target、硬件、样本和命令。

## 19. 分阶段任务

### Phase 0: 决策与基线

- [x] ADR-001：GUI 核心与应用框架边界。
- [x] ADR-002：双异步模型、runtime provider 和 UI bridge。
- [ ] ADR-003：应用 manifest/schema。
- [ ] ADR-004：资源包格式和 embedded/bundled 策略。
- [ ] ADR-005：Fluent、ICU4X 或组合方案。
- [ ] ADR-006：打包器复用策略。
- [ ] ADR-007：公开项目名、crate namespace 和商标检查。
- [ ] 记录 BMCBL 启动、空闲、内存、bundle 和任务延迟基线。
- [x] 固定 GPUI 上游基准 revision，并建立本地改动分类规则。

完成标准：所有会影响公共 API 或仓库拆分的决策有 ADR，不靠口头约定。

### Phase 1: GPUI 移入 crates 并独立维护

- [x] 使用保留历史的移动方式将 `vendor/gpui` 移到 `crates/gpui`。
- [x] 将 `gpui` 加入 workspace members，更新 path dependency 和文档。
- [x] 重写正式 `Cargo.toml`，不再使用 `Cargo.toml.orig`。
- [x] 删除 `Cargo.toml.orig`。
- [x] 增加 CI 断言，防止 `Cargo.toml.orig` 恢复。
- [x] 修正移动后的 `nova-gfx` 相对路径。
- [x] 使 manifest 不依赖 Zed workspace 隐式字段。
- [x] 保留 Zed 作者、版权、Apache-2.0，并新增 NOTICE/upstream metadata。
- [ ] 审计第三方文件、assets、examples 和许可证。
- [x] `publish = false`，直到发布 ADR 全部通过。
- [x] 更新 `ARCHITECTURE_BOUNDARIES.md`、项目结构和验证命令。
- [x] 建立 upstream sync 文档：基准 commit、patch 分类、冲突处理、验证。

完成标准：

```text
cargo metadata --manifest-path crates/gpui/Cargo.toml
cargo check --manifest-path crates/gpui/Cargo.toml
cargo check --manifest-path crates/gpui/Cargo.toml --examples
cargo package --manifest-path crates/gpui/Cargo.toml --list
```

以上命令不从源码树读取 `Cargo.toml.orig`，crate 内不存在 BMCBL `src` 依赖。
`cargo package` 在归档内自动生成的同名文件属于 Cargo 的规范化机制，不是仓库源文件。

### Phase 2: ApplicationHost 与 runtime

- [x] 创建 `egpui` 最小 facade。
- [x] 实现 lifecycle、service 注册、shutdown 和 headless mode。
- [x] 定义 runtime provider、default Tokio/Rayon provider 和并发预算。
- [x] 实现 `UiHandle` 和 stream bridge。
- [x] 实现 `UiTaskBridge`，保证进度背压、终态优先和 producer drop 取消语义。
- [x] 实现 `DurableWorkflowCoordinator`，保证 Running 恢复、处理器分派、
  终态 checkpoint 持久化和处理器失败可见。
- [x] 实现 task scope、取消、终态、panic/join error 语义。
- [ ] 定义 BMCBL `AppRuntime` 到 egpui 的适配接口；BMCBL 继续拥有现有工作流执行器。
- [x] 保留 download/archive/task-manager 业务语义在 BMCBL。
- [x] 增加 runtime、关闭和 UI bridge 单元测试。
- [ ] 增加 lag recovery 和跨线程 GPUI Entity 集成测试。

当前实现位于 `crates/egpui`，不依赖 BMCBL `src`。`ApplicationHost::run` 在
GPUI event loop 内只安装 quit signal 与前台 UI consumer；应用 runtime 的实际
drain/shutdown 在 `Application::run` 返回后执行。`UiHandle` 和 `UiStreamBridge<T>`
使用有界 Tokio channel，`try_*` API 会显式报告 backpressure。

完成标准：示例应用可以在后台执行 IO/CPU 工作并安全更新 Entity，关闭窗口不误取消
application-scope 任务，应用退出可以在 deadline 内收敛。

### Phase 3: Manifest、resources 与 i18n

- [x] 创建共享 manifest schema types。
- [x] 创建 `egpui-build` 和确定性生成管线。
- [x] 实现资源 namespace、embedded/bundled pack 和 development overlay。
- [x] 实现 Fluent i18n service、locale fallback、方向元数据和 UI snapshot。
- [ ] 实现字体与图标管线。
- [x] 实现严格 manifest schema、资源 namespace、embedded/bundled pack 和 development overlay。
- [x] 实现确定性资源索引、嵌入表生成和跨平台 bundle plan/executor 接口。
- [ ] 实现字体与图标转换工具及平台 smoke。
- [ ] BMCBL 资源逐步改用 egpui 资源接口。
- [ ] BMCBL `src/i18n` 增加 egpui adapter，迁移完成后删除重复 formatter 和状态源。

完成标准：BMCBL 资源和 catalog 可通过 egpui 的资源接口与构建索引接入，构建可
重现且缺失资源/翻译会在编译前得到明确错误；产品 key 和文案仍属于 BMCBL。

### Phase 4: 平台服务与 Windows 完整链路

- [ ] 实现 typed application paths 和 identity。
- [ ] 实现单实例、参数转发、深链接、文件关联。
- [ ] 实现 menu、tray、notifications、dialogs、clipboard、open。
- [ ] 从 manifest 生成 Windows manifest、VERSIONINFO 和 icons。
- [ ] 实现 Windows bundler backend、签名和 package smoke。
- [ ] BMCBL 删除 `SetCurrentProcessExplicitAppUserModelID` 和 winres 私有逻辑。
- [ ] 增加 Windows installer upgrade/uninstall CI。

完成标准：新模板应用和 BMCBL 均由同一声明生成完整 Windows 分发产物。

### Phase 5: macOS 与 Linux

- [ ] 完成 macOS lifecycle、bundle、identity、sign/notarize 扩展点。
- [ ] 完成 Linux XDG、desktop entry、AppStream、MIME 和 bundle。
- [ ] 平台 capability 查询和 `Unsupported` 行为一致。
- [ ] 建立 Wayland/X11 和 Metal smoke。
- [ ] 修复所有从 Windows 路径、编码、注册表或窗口模型泄漏到公共 API 的设计。

完成标准：同一示例源码在三平台通过 check 和窗口 smoke，平台包元数据正确。

### Phase 6: updater、security 与 diagnostics

- [ ] 实现签名 update manifest 和 channel。
- [ ] 实现下载、验证、stage、install、restart 和 rollback 保护。
- [ ] 实现危险 capability 声明与审计输出。
- [ ] 集成 license、SBOM、advisory 和 secret scanning。
- [ ] 实现 diagnostics snapshot、export 和可选 crash reporter。
- [ ] 为 corrupt update、network failure、disk full、cancel 和 power loss 建测试。

完成标准：更新失败不破坏当前版本，所有 release bundle 可追溯到 manifest、SBOM 和签名。

### Phase 7: CLI、模板、文档与独立仓库

- [ ] 完成 `new/dev/check/build/bundle/doctor`。
- [ ] 发布三类 runnable template。
- [ ] 完成 API、概念、平台、迁移、发布和贡献文档。
- [ ] 建立 SemVer、MSRV、feature、deprecation 和 release policy。
- [ ] 从 BMCBL 历史中提取框架 crates 到独立仓库。
- [ ] BMCBL 改为 git/version dependency，不依赖相对 sibling 布局。
- [ ] 独立仓库 CI 通过后再评估 crates.io 发布。

完成标准：一个没有 BMCBL 源码的干净环境能创建、测试并打包示例应用。

## 20. BMCBL 迁移映射

| 当前所有者 | 目标所有者 | 说明 |
| --- | --- | --- |
| `vendor/gpui` | `crates/gpui` | 保留 GUI 核心和上游许可 |
| `src/tasks/runtime.rs` 通用执行器 | BMCBL runtime adapter | egpui 只提供可替换接口，下载/归档/任务管理所有权不迁移 |
| `src/app.rs` 生命周期编排 | `ApplicationHost` + BMCBL plugin | 产品窗口和 route 留在 BMCBL |
| `configure_platform_app_identity` | platform identity service | manifest 驱动 |
| `build.rs` Windows resources | `egpui-build` | BMCBL 只声明元数据 |
| `build.rs` 图片/图标生成 | resource compiler | 确定性、可组合 |
| `src/assets` | resource service + BMCBL resource pack | 不再私建 AssetSource 管线 |
| `src/i18n` | BMCBL catalog adapter + egpui `I18nService` | egpui 拥有运行时契约，BMCBL 拥有产品 key/文案；迁移期间禁止双状态源 |
| BMCBL updater core | updater plugin | BMCBL 保留更新 UI |
| BMCBL single-instance helpers | platform single-instance service | 第二实例事件进入 host |
| BMCBL diagnostics UI | framework snapshot + BMCBL view | UI 不进入框架 |

迁移期间允许短期 adapter，但必须：

- 标注删除阶段；
- 不复制状态源；
- 不形成旧、新 runtime 双重所有权；
- 不让 render 同时读取旧 service 和新 snapshot；
- 每移除一个私有实现就增加对应框架集成测试。

## 21. 全局完成定义

框架达到“工程级桌面应用框架”至少满足：

- `gpui` GUI 核心与应用框架依赖分层明确；
- GUI 异步与应用异步分别拥有执行器、生命周期和取消语义；
- 新应用不需要手写平台资源 build script；
- 一个 manifest 能生成三平台身份、资源和 package metadata；
- egpui 提供统一 i18n runtime，应用拥有 catalog、key 和术语；
- Windows 有完整安装、签名、升级和卸载链路；
- macOS/Linux 有可用 bundle 与平台集成；
- updater 使用签名并有失败恢复；
- single instance、tray、menu、notification、deep link、file association 可用；
- headless、runtime、resource、platform 和 package 测试进入 CI；
- release 产物有 SBOM、许可证、hash 和签名；
- BMCBL 不再拥有通用资源、平台注册和打包实现；BMCBL 只维护产品 catalog，并通过 egpui i18n runtime 运行；
- 框架 crates 不依赖 BMCBL 业务模块，可迁移到独立仓库；
- 上游 GPUI/Zed 版权、许可证和独立 fork 声明完整且可审计。

## 22. 待确认决策与默认建议

以下问题不阻塞 Phase 0 文档工作；未明确回复时采用右侧默认值：

| 决策 | 默认建议 |
| --- | --- |
| 新框架公开名称 | `egpui`；公开发布前完成 crate namespace 和商标复核 |
| GPUI crate 名 | workspace 内保持 `gpui` 兼容，暂不发布 |
| 首发平台优先级 | Windows 完整，macOS/Linux 同期保证架构与 compile/smoke |
| 默认应用 runtime | Tokio + 独立 blocking/CPU budget，可替换 provider |
| i18n | egpui Fluent runtime + BCP 47 fallback；应用维护 catalog，ICU4X 作为区域格式扩展 |
| Windows 包格式 | portable + NSIS/MSI，MSIX 后续独立评估 |
| Linux 包格式 | AppImage + deb，rpm/Flatpak 后续 |
| macOS 包格式 | app + DMG，保留 PKG backend |
| 插件模型 | 首期编译期 Rust plugin；动态/WASM plugin 后续 |
| 打包实现 | 优先复用成熟 bundler，通过 backend 隔离 |
| 公共 API 稳定性 | pre-1.0 允许 breaking change，但必须迁移文档和 changelog |

## 23. 官方参考资料

- [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)
- [Zed software licensing overview](https://zed.dev/software-overview)
- [Tauri architecture](https://v2.tauri.app/concept/architecture/)
- [Tauri async commands](https://v2.tauri.app/develop/calling-rust/)
- [Tauri capabilities](https://v2.tauri.app/security/capabilities/)
- [Tauri resources](https://v2.tauri.app/develop/resources/)
- [Tauri distribution](https://v2.tauri.app/distribute/)
- [Tauri updater](https://v2.tauri.app/plugin/updater/)
- [Qt QCoreApplication](https://doc.qt.io/qt-6/qcoreapplication.html)
- [Qt resource system](https://doc.qt.io/qt-6/resources.html)
- [Qt QTranslator](https://doc.qt.io/qt-6/qtranslator.html)
- [Qt deployment](https://doc.qt.io/qt-6/deployment.html)
- [Qt threading](https://doc.qt.io/qt-6/threads.html)
- [WPF application management](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/app-development/application-management-overview)
- [WPF threading model](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/advanced/threading-model)
- [WPF globalization and localization](https://learn.microsoft.com/en-us/dotnet/desktop/wpf/advanced/wpf-globalization-and-localization-overview)
- [.NET Generic Host](https://learn.microsoft.com/en-us/dotnet/core/extensions/generic-host)

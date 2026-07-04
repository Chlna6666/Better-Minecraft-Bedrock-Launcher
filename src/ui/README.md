# `src/ui` 模块说明与放置规范

本文档用于说明当前 `src/ui` 目录的职责边界、目录分层和每个文件的大致用途。
跨 `crates/egpui`、`src/app.rs`、`src/ui` 和核心程序模块的总边界见
`docs/ARCHITECTURE_BOUNDARIES.md`。

## 放置规范

### 1. 总体原则

- `src/app.rs` 负责应用启动装配、全局资源注册、主窗口创建、生命周期接线。
- `src/ui/main_window.rs` 只负责主窗口根视图和主窗口内部编排。
- `src/ui/views/*` 只放页面级视图。
- `src/ui/components/*` 只放可复用的通用组件。
- `src/ui/overlays/*` 只放覆盖层、弹层、模态层。
- `src/ui/theme/*` 只放主题 token、颜色系统、主题工具。
- `src/ui/state/*` 只放全局 UI 状态或跨页面状态。
- `src/ui/runtime/*` 只放窗口根挂载、资源管理、UI 偏好等运行时支撑模块。
- `src/ui/views/<page>/state.rs` 只放该页面私有或页面主导的 UI 状态。

### 2. 依赖方向

- `src/app.rs` 可以依赖 `src/ui/*`，但 `src/ui/*` 不应反向承担应用启动职责。
- `src/ui/main_window/*` 可以编排 `views / overlays / state`，但不应直接吞掉下载、更新、图片、音乐的真实实现。
- `src/ui/views/*` 可以依赖 `components / theme / overlays / 页面 state / 全局 state`。
- `src/ui/components/*` 不应依赖具体页面模块。
- `src/ui/state/*` 只描述 UI 表现状态，不负责 IO、缓存、网络、解析。
- 一旦模块出现 `network / cache / decode / loader / parse / sort / prefetch / persist` 为主的职责，应优先判断是否应移出 `src/ui`。

### 3. 禁止越界

- 不要在 `src/app.rs` 里写页面 `render()`。
- 不要在 `src/app.rs` 里写下载逻辑、图片解码、音乐控制细节。
- 不要在 `src/ui/components/*` 里放只服务单一页面的业务组件。
- 不要把页面私有状态继续堆回 `src/ui/state/*`。
- 不要在 `src/ui/views/*` 里直接 new 基础设施客户端，优先走 `app` 装配和全局状态。
- 不要把 HTTP 实现细节放在 `src/ui`，应放在 `src/http`。
- `src/ui/views/*` 不应继续增长出 `cache / decode / loader / prefetch` 这类实现词汇；一旦出现，优先判断是否应下沉到非 UI 层。
- 已标记为越界的模块进入冻结状态，只允许拆分、外移或删除，不允许继续新增职责。
- 不要用 `cx.refresh_windows()` 作为动画驱动或局部状态变化的重绘手段。
- 需要动画下一帧时，优先通过 `src/ui/animation.rs` 的 helper 走 GPUI animation engine。
- 需要局部刷新时，优先调用对应实体的 `cx.notify()` 或更新该实体的本地状态，不要整窗刷新。

### 4. 落点判断

#### 放进 `src/ui/components/`

必须同时满足：

- 可复用。
- 无页面语义。
- 不依赖具体业务数据源。
- 不做 IO / 缓存 / 解码 / 网络。

#### 放进 `src/ui/views/<page>/widgets/` 或页面内部模块

满足任一即可：

- 只被一个页面使用。
- 名字带页面或业务语义。
- 依赖某个页面特有状态。

#### 放进 `src/ui/state/`

必须同时满足：

- 跨页面共享。
- 只影响 UI 表现。
- 不是业务持久状态。

#### 放进 `src/ui/views/<page>/state.rs`

满足任一即可：

- 主要只服务某个页面。
- 由页面本身驱动刷新和交互。
- 离开页面后不需要继续作为全局 UI 状态存在。

#### 应移出 `src/ui/`

满足任一就应优先下沉到非 UI 层：

- 解析。
- 排序。
- 下载。
- 网络请求。
- 图片解码。
- 资源缓存。
- 磁盘持久化。
- 播放控制。

### 5. 命名规范

- 应用入口层使用 `App*` 命名，例如 `AppBootstrap`。
- 主窗口壳层使用 `MainWindow*` 命名，例如 `MainWindowView`。
- 页面模块使用页面语义命名，例如 `HomePageView`、`DownloadPageView`。
- 通用组件避免带页面语义，例如 `InputState`、`ToastState`。
- 文件名优先表达职责，不使用模糊名字如 `shell`、`helper`、`misc`。

### 6. 当前硬规则

- `components/` 只保留真正通用组件，不保留页面语义组件。
- 页面状态跟页面走，不再把页面状态堆回统一的 `page.rs`。
- `views/*` 只负责“怎么显示”和“页面内 UI 编排”，不负责缓存、解码、网络、持久化。
- `main_window/*` 只负责窗口壳层编排，不负责真实业务实现。
- `runtime/*` 不是新的 `misc/`；如果模块开始承担持久化、缓存、解码、下载生命周期，应继续外移。
- 已确定越界的页面模块只允许继续下沉，不允许继续扩张。
- 主窗口不再承担动画调度中心职责，不要新增全局 animation driver。
- toast、dropdown、breadcrumb、topbar 这类动画必须保持局部调度，不要回流到主窗口统一探测。
- 对高频刷新列表，优先使用“页面级快照表 + 纯渲染函数”，不要为每一行再建一个 `Entity`。
- 任务页这类持续更新列表应保持扁平，不要把每个任务条目做成独立实体后再回挂到页面树上。
- 任务页这类高频列表的 `notify` 只能由用户可见的显示签名驱动，不要把 `sequence`、时间戳、内部递增计数当成变更条件。
- 任务页做 UI 隔离验证时，可以临时关闭真实任务数据流并注入静态假任务；这只用于定位问题，不是最终运行模式。

### 7. 交互数据零拷贝原则

- UI 和后台之间优先传递共享句柄，不优先传递 owned 大对象。
- 广播、订阅、页面缓存、弹窗状态优先使用 `Arc<T>` / `SharedString` / 轻量枚举，而不是整份 `String` / `Vec` / `HashMap` 反复克隆。
- `Render` 入口只做展示编排，不在其中重新组装大型中间数据。
- 需要高频刷新的数据，优先拆成静态部分和动态部分，动态部分只更新最小字段。
- 任何新增 `clone()` 都应先确认是否只是为了跨线程/跨实体持有生命周期；如果只是持有生命周期，优先改成共享句柄。
- 绝对零拷贝不应理解为“所有地方完全不 clone”，而是把热路径上的深拷贝收敛到一次构造、后续只传引用或共享句柄。

## 目录结构总览

### 根目录文件

| 文件 | 作用 |
| --- | --- |
| `src/ui/mod.rs` | UI 总模块导出入口，统一暴露 `ui` 子模块。 |
| `src/ui/main_window.rs` | 主窗口根视图模块，负责主窗口页面编排和主窗口内部协作。 |
| `src/ui/navigation.rs` | 路由定义与导航入口。 |
| `src/ui/runtime.rs` | UI 运行时支撑模块导出入口。 |
| `src/ui/state.rs` | UI 全局状态模块导出入口。 |
| `src/ui/overlays.rs` | 覆盖层模块导出入口。 |
| `src/ui/debug.rs` | 调试模块导出入口。 |
| `src/ui/README.md` | `src/ui` 目录结构说明与规范文档。 |

### `src/ui/components/`

这些文件应该保持“通用、可复用、无页面绑定”。

| 文件 | 作用 |
| --- | --- |
| `src/ui/components/mod.rs` | 通用组件模块导出入口。 |
| `src/ui/components/button.rs` | 通用按钮样式或按钮渲染封装。 |
| `src/ui/components/color_picker.rs` | 通用颜色选择与颜色值处理。 |
| `src/ui/components/dropdown.rs` | 通用下拉选择组件。 |
| `src/ui/components/html_renderer.rs` | HTML 内容到 EGPUI 元素树的渲染。 |
| `src/ui/components/icon.rs` | 图标元素或图标渲染辅助。 |
| `src/ui/components/input.rs` | 输入框状态、事件、渲染与交互逻辑。 |
| `src/ui/components/markdown_renderer.rs` | Markdown 文档渲染。 |
| `src/ui/components/modal.rs` | 通用模态层容器。 |
| `src/ui/components/virtual_list.rs` | 虚拟列表窗口切片与占位间距计算。 |
| `src/ui/components/scroll.rs` | 滚动区域封装。 |
| `src/ui/components/toast.rs` | 全局 Toast 系统与 Toast 状态。 |
| `src/ui/components/toggle_switch.rs` | 通用开关组件。 |

### `src/ui/debug/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/debug/state.rs` | 调试窗口状态，以及主/调试窗口运行时采样缓冲。 |
| `src/ui/debug/view.rs` | 调试窗口视图，负责自适应布局、性能面板、日志控制台和开发操作。 |
| `src/ui/debug/window.rs` | 调试工具接入、调试窗口配置，以及元素样式/历史回选辅助接口。 |

### `src/ui/runtime/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/runtime/root_view.rs` | 窗口根视图容器，负责把任意视图挂到窗口根节点。 |

补充约束：

- `root_view.rs` 属于 UI 根挂载，可保留在 `runtime/`。
- UI 偏好持久化不再留在 `runtime/`，当前已下沉到 `src/core/ui_prefs.rs`。
- 未接入的全局资源缓存代码不应继续以 `runtime` 名义保留；若未来需要资源缓存，应以明确落点和真实消费方重新引入。

### `src/ui/state/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/state/agreement.rs` | 用户协议弹层状态与接受状态。 |
| `src/ui/state/debug.rs` | 主 UI 侧调试状态导出入口，复用 `debug/state.rs`。 |
| `src/ui/state/launch_prereq.rs` | 启动前依赖检查、安装操作与覆盖层展示状态。 |
| `src/ui/state/navigation.rs` | 导航动画、激活项、标签显隐等导航状态。 |
| `src/ui/state/quit.rs` | 应用退出过渡动画状态。 |
| `src/ui/state/theme.rs` | 当前主题模式、强调色、主题动画状态。 |
| `src/ui/state/update.rs` | 更新检查、更新弹层、下载进度相关 UI 状态。 |

### `src/ui/main_window/`

这些文件都属于主窗口内部实现，不应上提到 `src/app.rs`。

| 文件 | 作用 |
| --- | --- |
| `src/ui/main_window/background.rs` | 主窗口背景渲染。 |
| `src/ui/main_window/background_support.rs` | 背景源选择、背景快照、背景辅助函数。 |
| `src/ui/main_window/chrome.rs` | 主窗口顶部栏、窗口按钮、导航胶囊等窗口框架 UI。 |
| `src/ui/main_window/chrome_view.rs` | 顶部栏视图实体，负责把全局状态转成顶部栏渲染状态。 |
| `src/ui/main_window/controls.rs` | 主窗口里各页面输入控件和订阅关系的接线。 |
| `src/ui/main_window/music_player.rs` | 主窗口音乐播放器 UI 与交互渲染。 |
| `src/ui/main_window/page_loading.rs` | 页面数据懒加载与首次加载流程。 |
| `src/ui/main_window/page_registry.rs` | 页面实体注册、创建、释放和缓存。 |
| `src/ui/main_window/route_effects.rs` | 路由切换副作用处理。 |
| `src/ui/main_window/support.rs` | 主窗口内部小型辅助函数。 |
| `src/ui/main_window/update_flow.rs` | 更新检查结果、更新下载状态到 UI 的流转。 |

补充约束：

- `main_window/*` 只做窗口壳层编排、页面切换编排、顶栏/背景/覆盖层挂载。
- 不要在这里继续堆真实下载实现、图片处理实现、更新下载实现、音乐播放实现。
- `support.rs` 只能保留小型辅助，不能继续长成业务收容器。

### `src/ui/overlays/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/overlays/launch_prereq.rs` | 启动前依赖检查与安装操作覆盖层 UI。 |
| `src/ui/overlays/update.rs` | 更新弹层 UI。 |
| `src/ui/overlays/user_agreement.rs` | 用户协议弹层 UI。 |

### `src/ui/theme/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/theme/mod.rs` | 主题模块导出入口和主题工具函数。 |
| `src/ui/theme/colors.rs` | 主题颜色 token、浅色/深色调色板和插值逻辑。 |

### `src/ui/views/`

`views` 目录只放页面，不放应用装配和通用组件。

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/mod.rs` | 页面模块导出入口。 |
| `src/ui/views/home.rs` | 首页页面模块导出入口。 |
| `src/ui/views/download.rs` | 下载页页面模块导出入口。 |
| `src/ui/views/manage.rs` | 版本管理页。 |
| `src/ui/views/settings.rs` | 设置页总入口。 |
| `src/ui/views/tasks.rs` | 任务页。 |
| `src/ui/views/tools.rs` | 工具页总入口。 |

补充约束：

- 页面目录下允许保留“仅服务该页面的 UI 组装代码”。
- 页面目录下若出现 `cache / decode / loader / prefetch / parse / sort` 为主的模块，应优先判定为越界。
- 页面根文件和页面子目录共同组成一个页面模块时，根文件负责导出和页面入口，子目录负责页面内部实现。

### `src/ui/views/home/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/home/page.rs` | 首页真实视图实现。 |

补充说明：

- 首页版本解析与排序已经下沉到 `src/core/version/launch_versions.rs`。
- 原 `src/ui/views/home/curseforge_images.rs` 无调用链，已删除，避免继续把预取/解码逻辑留在页面层。

### `src/ui/views/download/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/download/common.rs` | 下载页通用 UI 片段和辅助函数。 |
| `src/ui/views/download/state.rs` | 下载页及 CurseForge 子页的页面状态。 |
| `src/ui/views/download/toolbar.rs` | 下载页顶部工具栏、筛选和分页控制。 |
| `src/ui/views/download/game.rs` | 游戏包下载页面。 |
| `src/ui/views/download/mods.rs` | 资源/模组下载页面。 |
| `src/ui/views/download/curseforge.rs` | CurseForge 资源页渲染入口与页面内 UI 编排。 |

### `src/ui/views/manage/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/manage/state.rs` | 管理页页面状态与版本列表状态。 |

### `src/ui/views/download/curseforge/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/download/curseforge/content.rs` | CurseForge 内容区渲染。 |
| `src/ui/views/download/curseforge/image_prefetch.rs` | CurseForge 图片预取策略、预取批次选择与后台抓取调度。 |
| `src/ui/views/download/curseforge/image_state.rs` | CurseForge 图片可见集计算、图片缓存裁剪与页面图片状态清理。 |
| `src/ui/views/download/curseforge/image_pipeline.rs` | CurseForge 图片结果队列、RenderImage 刷新与缓存淘汰编排。 |
| `src/ui/views/download/curseforge/modals.rs` | CurseForge 专属弹层、安装对话框和 UI 状态写回。 |
| `src/ui/views/download/curseforge/results_state.rs` | CurseForge 查询条件变化后的结果失效与刷新调度。 |
| `src/ui/views/download/curseforge/share_actions.rs` | CurseForge 分享文本解析、剪贴板导入与复制动作。 |

补充说明：

- CurseForge 搜索缓存、元数据缓存、图片磁盘缓存与图片缩放压缩已下沉到 `src/core/curseforge/cache.rs`。
- CurseForge 资源详情查询和文件列表查询已下沉到 `src/core/curseforge/queries.rs`。
- `src/ui/views/download/curseforge.rs` 已不再直接承载图片缓存清理、结果失效调度和分享文本解析，这些职责已拆到明确子模块。
- `modals.rs` 已不再直接发起网络查询或做接口 DTO 映射，只负责 UI 状态切换与结果写回。
- 原 `images.rs` 已拆成 `image_prefetch.rs` 与 `image_pipeline.rs`，避免继续以单文件承载预取策略、结果队列和缓存淘汰。
- CurseForge 图片预取状态使用 Tokio `AbortHandle` 跟踪真实后台抓取任务；图片结果先进入后台共享队列，再由单一 UI 结果泵批量回写，避免每张图各自唤醒主线程。

### `src/ui/views/settings/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/settings/state.rs` | 设置页页面状态与设置页专属枚举。 |
| `src/ui/views/settings/common.rs` | 设置页通用样式和辅助函数。 |
| `src/ui/views/settings/content.rs` | 设置页内容切换和分区分发。 |
| `src/ui/views/settings/tabs.rs` | 设置页标签栏或分类栏。 |
| `src/ui/views/settings/rows.rs` | 设置项行组件构建器。 |
| `src/ui/views/settings/about.rs` | 关于页入口。 |
| `src/ui/views/settings/customization.rs` | 个性化设置入口。 |
| `src/ui/views/settings/game.rs` | 游戏相关设置页。 |
| `src/ui/views/settings/launcher.rs` | 启动器相关设置页入口。 |

### `src/ui/views/settings/about/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/settings/about/sponsors.rs` | 赞助者弹层 UI、分页状态切换和加载结果展示。 |
| `src/ui/views/settings/about/update_flow.rs` | 关于页中的更新相关区块。 |

补充说明：

- 赞助者数据获取、排序和头像缓存已经下沉到 `src/core/sponsors.rs`。

### `src/ui/views/settings/customization/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/settings/customization/background.rs` | 背景设置区块。 |
| `src/ui/views/settings/customization/theme_color.rs` | 主题色设置区块。 |

### `src/ui/views/settings/launcher/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/settings/launcher/connectivity.rs` | 连通性检测和网络连通设置区块。 |
| `src/ui/views/settings/launcher/download.rs` | 下载源、代理、线程数等设置区块。 |

### `src/ui/views/tools/`

| 文件 | 作用 |
| --- | --- |
| `src/ui/views/tools/state.rs` | 工具页页面状态与联机页状态。 |
| `src/ui/views/tools/common.rs` | 工具页通用辅助函数。 |
| `src/ui/views/tools/sidebar.rs` | 工具页侧边栏。 |
| `src/ui/views/tools/online.rs` | 联机工具页入口。 |
| `src/ui/views/tools/online_controls.rs` | 联机工具控件接线。 |
| `src/ui/views/tools/online_peers.rs` | 联机成员列表。 |
| `src/ui/views/tools/online_room.rs` | 联机房间详情区。 |
| `src/ui/views/tools/online_widgets.rs` | 联机工具复合控件。 |

## 后续整理建议

- `src/ui/views/download/curseforge/image_prefetch.rs` 仍然承担明显的预取批次选择和后台抓取调度，后续应继续评估是否下沉到更稳定的资源加载层。
- `src/ui/views/download/curseforge/image_pipeline.rs` 仍然承担结果队列、RenderImage 刷新和缓存淘汰编排，后续如果继续增长，应继续拆解为更细职责模块。
- `src/ui/main_window/support.rs` 应持续保持为小型辅助，不应重新长成业务模块。

## 本轮已处理

- `src/ui/views/home/launch_versions.rs` 已移出 UI，落点为 `src/core/version/launch_versions.rs`。
- `src/ui/views/home/curseforge_images.rs` 因无调用链已删除，后续若恢复相关能力，应落在非 UI 层。
- `src/ui/views/download/curseforge/cache.rs` 已移出 UI，落点为 `src/core/curseforge/cache.rs`。
- `src/ui/views/download/curseforge/images.rs` 已拆分为 `image_prefetch.rs` 和 `image_pipeline.rs`，并继续调用 `src/core/curseforge/cache.rs` 处理图片磁盘缓存和缩放压缩。
- `src/ui/views/download/curseforge.rs` 已拆出 `image_state.rs`、`results_state.rs`、`share_actions.rs`，页面根文件只保留渲染入口和页面内编排。
- `src/ui/views/download/curseforge/modals.rs` 已移除资源详情查询、文件列表查询和接口 DTO 映射，改为调用 `src/core/curseforge/queries.rs`。
- `src/ui/views/download/curseforge/image_prefetch.rs` 当前使用 Tokio `AbortHandle` 管理真实图片抓取任务；抓取结果通过共享结果队列汇总，再由 `image_pipeline.rs` 的单一 UI 结果泵批量应用，避免翻页时为每张图片单独向主线程投递消息。
- `src/ui/runtime/prefs.rs` 已移出 UI，落点为 `src/core/ui_prefs.rs`。
- `src/ui/runtime/resource_manager.rs` 因无实际消费方且职责不清已删除，应用启动链不再初始化该全局缓存。
- `src/ui/views/settings/about/sponsors.rs` 已移除网络请求、排序和头像缓存实现，改为调用 `src/core/sponsors.rs`。

## 当前已知越界风险

- `src/ui/views/download/curseforge/image_prefetch.rs`
  现状：仍然承担图片预取策略和后台抓取调度，仍偏重。
- `src/ui/views/download/curseforge/image_pipeline.rs`
  现状：仍然承担结果队列、RenderImage 刷新和缓存淘汰编排，仍偏重。

## 网络请求规范

### 核心原则

**所有网络请求必须使用后台线程执行，严禁在 UI 主线程中发起任何网络调用。**

### 实现模式

#### 1. 标准后台请求模式

```rust
// 正确示例：使用 tokio::spawn 在后台执行请求
let (tx, rx) = tokio::sync::oneshot::channel();
tokio::spawn(async move {
    let result = some_network_request().await;
    let _ = tx.send(result);
});

// 在 UI 线程中接收结果
cx.spawn(async move |_this, cx| {
    let result = rx
        .await
        .map_err(|_| "network task dropped".to_string());
    
    match result {
        Ok(Ok(data)) => {
            // 更新 UI 状态
            cx.update_global(|state: &mut SomeState, _cx| {
                state.data = data;
            });
        }
        Ok(Err(e)) | Err(e) => {
            // 处理错误
            cx.update_global(|state: &mut SomeState, _cx| {
                state.error = Some(SharedString::from(e.to_string()));
            });
        }
    }
    
    Ok::<(), anyhow::Error>(())
})
.detach();
```

#### 2. 并发控制模式

```rust
// 全局信号量限制并发连接数
static FETCH_SEMAPHORE: once_cell::sync::Lazy<Arc<tokio::sync::Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(tokio::sync::Semaphore::new(6)));

// 在请求中获取许可
let semaphore = FETCH_SEMAPHORE.clone();
let _permit = semaphore.acquire_owned().await?;
// 执行请求...
```

#### 3. 超时控制模式

```rust
// 创建带超时的客户端
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(5))
    .connect_timeout(Duration::from_secs(3))
    .build()?;
```

#### 4. 任务取消模式

```rust
// 使用 AbortHandle 管理任务生命周期
let task = tokio::spawn(async move { ... });
let abort_handle = task.abort_handle();

// 存储 handle 以便后续取消
state.abort_handles.insert(id, abort_handle);

// 取消时
if let Some(handle) = state.abort_handles.remove(&id) {
    handle.abort();
}
```

#### 5. 后台解码模式

```rust
// 使用 spawn_blocking 在后台解码大文件
let decode_result = tokio::task::spawn_blocking(move || {
    let decoded_image = image::load_from_memory(&bytes)?;
    Ok(decoded_image)
})
.await;
```

### 禁止行为

❌ **禁止在 UI 线程中直接 await 网络请求**
```rust
// 错误示例：阻塞 UI 线程
async fn load_data(&self, cx: &mut Context<Self>) {
    let response = client.get(url).send().await?; // 错误！
}
```

❌ **禁止在 render() 方法中发起网络请求**
```rust
// 错误示例：render 中发起请求
fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
    let data = fetch_data().await; // 错误！
}
```

❌ **禁止静默丢弃错误**
```rust
// 错误示例：丢弃错误
let _ = some_network_call().await; // 错误！

// 正确示例：传播或处理错误
some_network_call().await?;
```

### 检查清单

在提交包含网络请求的代码前，请确认：

- [ ] 所有网络请求使用 `tokio::spawn` 或 `cx.background_spawn`
- [ ] 结果通过 `cx.update_global` 或 `entity.update` 在 UI 线程更新
- [ ] 有适当的超时控制
- [ ] 错误被正确处理，不静默丢弃
- [ ] 长时间任务支持取消（使用 `AbortHandle`）
- [ ] 并发请求有适当的限制（使用信号量）
- [ ] 大文件解码使用 `spawn_blocking`

### 相关文件

- `src/ui/views/download/curseforge/results_state.rs` - 搜索结果请求示例
- `src/ui/views/download/curseforge/image_prefetch.rs` - 图片预取和并发控制示例
- `src/ui/views/download/curseforge/modals.rs` - 详情页请求示例
- `src/ui/main_window.rs` - 元数据加载示例
- `src/ui/views/download/curseforge/image_pipeline.rs` - 后台解码示例

# GPUI 统一构帧与合成链路：持续执行 TODO

> 状态：进行中。此文件是独立维护 GPUI/Nova-GFX 渲染重构的唯一进度入口；不是 BMCBL 业务 UI TODO。
> 看板历史建档基线：2026-10-15 `678d02079f895186b5dc33d88a08592d753fdc87`（**不是当前 HEAD**）；2026-10-14 DX11 与 2026-10-15 DX12 日志均为历史基线，不是最新提交的验收证据。原始文档基线 `405a7133eec7e093ad93681ed725de6a69a89cfb`。
> **所有后续对话/代理在编辑前必须重新读取 master HEAD、本文和相关源码，不得使用本文件的历史 HEAD 直接写入。**
> 本文是唯一执行状态源；[GPUI P0 历史依赖审计](GPUI_P0_RENDER_DEPENDENCY_AUDIT.md)仅作已发生事实与当时源码快照参考，其旧 RenderTree/RenderCommit 方案不再是目标。

## 最新执行看板（唯一实时任务表，2026-10-15）

**本节是当前决策依据**；下面 `P0-A / P0-B / P1–P3` 的长勾选记录与逐提交小节是**历史实现账本**，不再用它们单独认定完成。新增任务或改变状态先改此表，并在原有历史章节记对应提交/验证证据，不另建 TODO 文件。

**状态语义**：`已提交·待验证`＝仓库中存在实现/测试源码，但**无可复核的当前 HEAD Rust 构建、测试及目标后端证据**；`部分实现`＝某条路径已写，但全任务边界尚未闭合；`待处理`＝尚未实现任务要求。只可依据实际结果将状态推进到「编译通过 → 对应测试通过 → 指定后端实机通过 → 完成」。尚无记录不等于用户从未本地测试，标记为**未提供证据**。不使用没有实测来源的 GPU 百分比、帧时间、显存节省数据。

| ID | 优先级 / 现状 | 下一可执行切片（完成时必须有的产物） | 前置与验收出口 |
| --- | --- | --- | --- |
| **P0-01** | P0 · **编译部分通过：本机 workspace check 已过；fmt/完整单测仍阻塞** | **构建与单测阻塞清零**：检查 GPUI/Nova 当前 master 的 Rust fmt/check、`AutoStatic`、`gaussian_capture`、`retained_filters`、`chunk_upload`、设置 Trace/TOML 兼容性；出现 E0308/E0616/visibility/borrow 错误按实际源码修复并提交，**不以 CI 代替本地测试** | 无前置；记录命令、后端 Feature、错误/通过输出、测试数与对应 commit；未提供证据前其余 P0 全是“待验证” |
| **P0-02** | P0 · **DX12 已确认开启；关闭/重启待验收** | **Trace 实际启停验收**：设置 → 启动器 → `GPUI / Nova 详细 Trace` 独立开关；持久化→完整重启→开启/关闭分别确认 `frame_metrics`、`gpui::diagnostics=trace`、`latest.log`；旧配置缺少键时默认 false；验证日志滚动限制 | P0-01；`src/config/*`、`src/ui/views/settings/*`、`src/app.rs`、`src/startup.rs`、`src/utils/logger.rs`；不触发 Debug 子窗口、不以环境变量绕过 |
| **P0-03** | P0 · **DX12 UI Trace 已采到；事件因果链未闭环** | **同一次实验的时间线闭环**：将 UI Dropdown 打开/收起、Re-target、页面切换、Frame Request 的原因、`static_prefix_captured`、`auto_static_relocation`、`GPUI AutoStatic submitted`、Draw/Present 按 `window_id` / scene revision / 事件顺序关联；确认 `src/ui/components/dropdown.rs` 与 `dropdown_overlay.rs` 现存 `tracing::trace!` **不会仅凭 `gpui::diagnostics=trace` 自动进入日志**，补安全的 UI 侧定向 Trace 或关联字段，不全局开启所有库 trace | P0-01/02；一份可复现完整 Trace + 帧事件对照，保留低开销默认关闭语义 |
| **P0-04** | P0 · **AutoStatic 四后端合成像素/静态复用通过；真实 Dropdown Capture 待验证** | **AutoStatic 静态前缀真缓存命中**：公共 Dropdown 连续展开/回收与首页版本下拉 A/B；记录 `static_prefix_captured=true`、成功 Present 后 `source_index=Some`、稳定动画帧 `refreshed_layers=0`、`source_draw_steps=0`，且 `native_view/content_version` 无故不变更；失效/Resize 首帧允许必要重绘 | P0-01～03；逐帧结果及截图，不能把 “Capture candidate” 误报为真实 GPU 缓存命中；涉及 `renderer/{retained_upload,present,filters}.rs` |
| **P0-05** | P0 · **新增同规格 Gaussian 跨索引物理复用源码；原生测试待执行，容量超限仍重建** | **Filter Target 高水位复用**：Gaussian/Root/Element + AutoStatic 混合，预热 3→4→6 后连续 6→3→4→6；`active_isolated/allocated_isolated`、`layout reused/recreated`、物理 View、ResourceSet、Filter 输出内容版本一起核验；新 Slot、资源变化必须重绘而非采样旧像素 | P0-01～04；容量以内不应每次重新 create/destroy 全链；容量以外可重建；必须校验异步 Fence / GPU in-flight / 多窗口 / Blur on/off |
| **P0-06** | P0 · **Root 下线/重上与 FrameSlot 四后端原生测试通过；独立 Window Owner/Fence 待迁移** | **Root Color 和 Filter Capture 真实资源所有权分离**：根颜色不再使用 `retained_root_blur`/零半径 Blur 充当永久语义所有者；把 RootColor 的尺寸、内容版本、物理 View、Fence 退休和 Filter DAG 依赖独立定义，迁移旧消费者并删除冗余旧所有者 | P0-04/05 的实机正确性基线；跨 Popup 不无故失效、Resize/透明/Clip/Mask/Texture 别名不出现旧帧；不要再增加第二套 Root 缓存 |
| **P0-07** | P0 · **四后端共用 Scene Capture 损伤扩散源码已补；导航/首页动画与模糊 GPU 成本仍待四后端原生验收** | **Dropdown GPU40% vs 首页20% 端到端归因**：记录 UI notify / Render、Taffy layout/Prepaint/Paint、CPU Encode/Upload、Resident Copy、Filter/Composite Pass 和 GPU Timestamp；用同一后端和物理尺寸、刷新率、模糊设置做 A/B，区分 CPU Owner busy 与任务管理器 GPU 使用率 | P0-02～05；历史 DX12 日志 24 次 layout recreated、`composite_passes=12`、`blur_source_pixels=1,489,240` 仅作旧版基线；必须确认**最新 master** 的新指标，不预设是模糊或 Taffy 的单一根因 |
| **P0-08** | P0 · **公共动画绑定/replay/精确损伤修复已实现；定向测试通过，最终调用方与跨平台实机待验** | **UI 粒度失效和帧请求源**：对独立 Settings 内容 Entity、Overlay、CachedView/DispatchTree、NavChrome、Toast、任务与隐藏页更新验证“不可见页停止构帧、纯动画不用 UI 每帧重排、静态画面无无效 Present”；补 Frame Request reason 与进入 UI/Composite/WSI 的时序 | P0-03/07；Frame Request ≠ Draw ≠ Present；记录 240Hz 静止/动画以及隐藏/失活/最小化的 requests、draws、p95 Prepaint，前台过渡不卡顿 |
| **P0-09** | P0 · **DX12 子窗口关闭修复后 UI 复验 2 次通过；Resize/其余后端待验收** | **Resize 与窗口生命周期四后端回归**：WM_SIZE 消息栈不重型构帧；尺寸代次 Client→Layout→Drawable→成功 Present 对齐；专项 DX11 拖拽不出帧、DX12 子窗口关闭 exit 2173、Vulkan 拉伸/裁剪、OpenGL 拖拽卡顿，以及 DPI、多窗口、DComp/透明、最小化恢复 | P0-01/03；附真实像素/视频和日志、未提交帧 Fence 和窗口析构状态；不能用强制整窗重绘/每帧 wait_idle 伪修复 |
| **P1-01** | P1 · **待处理** | **单一所有权 / 字节账本**：逐一列出 `Frame/SceneSlot/Scene/PresentationPacket/FrameUpload/DrawSteps` 的生产者、有效期、实际拥有的状态与 copy bytes；列所有消费者和可删除/必须保留职责 | 先通过 P0-01 并有 P0-07 profile；提供源码符号检索与现有功能清单，明确 Element/输入/IME、3D Scene、Shader/Native backend 不在误删范围 |
| **P1-02** | P1 · **部分实现** | **唯一持久 Paint Artifact + 局部失效**：用持久 Chunk/Order/Filter/Property 依赖代替逐帧全量 Scene replay 与 Scene Finish；完成 CachedView 嵌套、中断/重定向动画、重排 Painter Order、纯 Composite 属性更新时不重建内容 | P1-01；相同输入下画面像素等价、静态 Buffer upload=0、变化只上传受影响范围；一条局部纵切跑通后再扩展 |
| **P1-03** | P1 · **待处理** | **唯一 GPU 消费协议**：替换 `PresentationPacket.scene` 全量快照与 `SceneSlot` 主链路，GPU Owner 的 Create/Update/Remove/Animation/Damage/Surface epoch 有顺序的增量提交；队列 latest-wins 不丢删除/资源操作 | P1-02 + P0-09；断帧、动画中断、跨窗口、Resize、队列拥塞与 GPU Fence 下正确；不额外建立 RenderTree/RenderCommit 等价影子结构 |
| **P2-01** | P2 · **待处理** | **显存预算与资源退休**：Filter/Root/Path/Atlas/Buffer 的 alloc/reuse/retire/budget/fragmentation/idle-trim 指标；统一资源所有者、Fence 退休，避免已预热 Spare Target 无限驻留 | P0-05/06、P1-03；高频弹层结束、关闭子窗口及 Resize 后 residency/peak/retire 可回落；DX11/DX12/VK/GL 资源生命周期等价 |
| **P2-02** | P2 · **部分实现** | **原生 GPU 时间与 Render Graph 同步**：查询各后端 GPU pass timestamp、Copy/Upload、Filter、Composite、Present 的可比较时长；图资源版号和 Alias RD/WR/WAW barrier、Load/Clear/Scissor 与多 FrameSlot 保护 | P0-07、P2-01；不能用 Owner CPU busy 替代 GPU 时间；跨页面、跨帧物理 alias、首帧、Resize 后输出像素正确 |
| **P2-03** | P2 · **待处理** | **跨后端/跨平台统一语义**：同一 Composition/Damage/Blend/Alpha/Filter/Texture ABI 接 DX11、DX12、Vulkan、OpenGL，Metal 与 Linux consumer 明确可编译迁移范围 | P1-03、P2-02；相同测试场景画面/动画/输入一致，记录不支持项，禁止只修某个 Backend |
| **P3-01** | P3 · **待处理** | **破坏性删除与消费者迁移**：删除被替换的 `Scene` 全量容器/Replay/Finish、`Frame.scene`、`SceneSlot`、`pack_scene`、全场景 `FrameUpload` 等旧主通路；将 `render_pipeline` 的真 Shader/SVG/RendererOptions 分归正确模块并修全部消费者 | P1-01～03、P2-03；`rg` 符号搜索、编译/单测、示例/Bench/文档说明；不能删 `gpui-3d::Scene`、IME/Hitbox/Atlas/Native Surface |
| **P3-02** | P3 · **待处理** | **完整回归与基准验收**：四后端静止 / Dropdown 高频 / Blur on-off / 多窗口 / Resize / 输入法 / 动态背景 / 字体图像 / 地图 / 关闭恢复场景的像素、CPU p50/p95/p99、GPU pass、RSS/VRAM、upload bytes / frame 全矩阵 | P0～P3-01；对相同硬件、刷新率/驱动/渲染参数做前后对照；仅有回归与不退化证据后才把统一合成器标记完成 |

### 2026-10-16 Scene 职责迁移（基线 `39caea45`）

- 已迁移：Paint / Animation / Backdrop Damage、Filter Source 纯判定进入 `paint/`；全部仓库消费者直接调用新算法，删除 Scene 旧方法。零半径 identity Capture 的局部子元素变化保留旧、新像素损伤，不扩大到整个 Capture。
- 已迁移：Chunk identity、候选、规划、稀疏 Replay 结构索引及 clear/finish/trim 生命周期进入 `window/retained/`。Frame 与 Presentation Snapshot 成对持有 Paint 和 Retained Owner；Nova `retained_upload` / `chunk_upload` 仍是 GPU 驻留的唯一所有者，没有新增 GPU Cache。
- 本地验证：workspace no-default-features check、bmcbl lib locked check、GPUI 四后端 features test-target check 通过；435 个定向测试通过。四后端各执行 6 项原生像素回归，共 24 项通过，覆盖 popup 反向、静态背景复用、buffer in-flight、字体图像、resize 与 chunk splice。
- 格式基线：workspace `cargo fmt --all -- --check` 仍因修改前已存在的无关格式差异失败；新 `paint/`、`window/retained/` 模块的 rustfmt check 与 `git diff --check` 通过。未用 CI 代替本地验证。
- 未完成：P0-06 Root Color 独立 GPU 生命周期与 Render Graph、P1-03 SceneSlot / 快照主通路替换、四后端实际应用交互性能矩阵。上述像素测试不等于 GPU 时间或任务管理器占用改善；尚无可报告的性能收益。
- UI 调查：Tab pill 与公共 Dropdown 中间帧使用 presentation；响应式导航标签和首页启动下拉仍有真实 Layout / caller-sampled 路径。后续保持现有弹性、错峰、裁剪与阴影轨迹，不能用 API 替换推定视觉等价。

### 2026-10-15 后续执行：动画与 Resize（基线 `14289e14`）

**2026-10-16 首页列表回退（当前 HEAD `600023a7`）**：用户反馈最新首页版本列表动画异常卡顿，撤回 `6d15651c` 引入的逐行 `clipped_translation_opacity` sampled Capture，恢复原来的 `top(10 * (1 - item_factor)) + opacity(item_factor)`。错峰、弹性、容器高度/阴影、虚拟列表与选择逻辑保持原实现；箭头稳定 identity 和公共 GPUI 修复保留。仅修改首页及本文，不恢复已删除的旧 API，不新增兼容层。不将行 Capture 推测为已证实根因，也不将源码恢复视为运行时卡顿已修复。按用户要求不重复构建/测试，仅检查定向差异及格式；运行效果待当前产物重新构建后确认。下文“首页所有行迁移”是已撤回的历史切片，不再代表当前状态。

**最新执行（用户授权全部相关动画修复，随后要求停止逐项反复测试，并明确全平台、无兼容层）：**

- `element/animation.rs` 删除帧局部 `with_sampled_animation`、`SampledAnimationElement` 和配套 state，不保留 alias、转发或双入口；唯一 caller-sampled API 使用稳定 identity。平移/透明度端点变化不再更换 ID 或强制重画后代；Capture 语义变化仍会重新绑定以保证正确性。静止非 identity 的终值继续生效，但不调度下一帧；包装层每次执行写入样本、内部静态内容允许复用。
- `scene/display_list.rs` 修复 retained replay 只复制 primitive 而遗漏 caller-sampled 值的问题。仅携带被 replay 范围实际引用的样本，去重且保留本帧已经写入的新值；不添加第二个 Scene/Root Owner，不复制无关目标样本。新增 retarget/settle 集成测试曾稳定复现旧行为的 `primitive 有绑定但 samples=[]`，修复后通过。
- `animation/engine.rs` 的 scene-bound track 不再追加保守 declaration bounds（Translation 可为整窗），由现有 Scene 前后真实样本计算 swept damage；无 scene binding 的 UI track 保留原有 dirty bounds。正/负过冲用实际样本并集覆盖，不截断曲线或用限帧掩盖成本。
- 跨平台 `PlatformWindow::owns_scene_animations` 删除依赖 `has_active_presentation_animations` 的默认实现，成为必须显式实现的所有权契约。Windows/Wayland/X11 的 Nova 独立呈现端始终拥有队列中、deferred 与等待 frame-ready 的 scene 动画；macOS 原生 Metal 明确仍由 UI 采样；TestWindow 同样显式声明。不是只给 DX12 特判，也没有令尚无独立呈现采样实现的 Metal 错误跳过 UI 时间推进。`animation_clock.md` 同步更新契约。
- 首页箭头及所有虚拟列表可见行改用稳定 sampled 绑定。保留逐行 delay、ease-out-back、位移10px、透明度与容器原始高度/顶部/阴影弹性；行不再每帧改 text layout origin，使用 clipped subtree translation 保留图片局部 clip。行与箭头是被动 sample，由容器单一 layout target 驱动，不各自再请求下一帧。新增行 Capture 需要实机核对静态源复用及 GPU pass 成本，不能先声称 GPU 降低。
- 认证弹层移除 `state.animating` 的额外 owning-view layout 帧请求及冗余聚合字段；由局部稳定 wrapper 驱动。Toast、认证内容/行、公共组件共享上述框架修复。导览三条 feature 的延迟260ms过渡改为 `.delay + FillMode::Both + translation_opacity`，删除原来的两段 CPU margin-left 回调，延迟30/80/130ms和10px轨迹不变。
- 全应用检索覆盖 `with_animation`、`with_animations`、visual/group/stable sampled、布局帧请求；未机械改写已在 presentation lane 的 Tabs、公共 Dropdown、Modal、加载/进度指示器等。首页容器真实高度弹性、任务退出时的高度/padding/换行变化、欢迎卡 margin/自适应尺寸、主题与任务颜色过渡仍有 CPU 工作；这些不能通过删除颜色/错峰/布局效果或假装已有 GPU Color 支持来“清零”。**全部动画独立 presentation 迁移尚未完成，不能标为全部性能问题已解决。**
- 已执行验证：no-default GPUI 测试产物 `gpui-1e4c381270e0dfcf.exe`，`animation::` **104 passed / 1 ignored**；ownership **3 passed**；referenced-sample replay **1 passed**；Translation sampled damage（含正/负过冲）**1 passed**。`cargo check -p bmcbl --lib --locked` 在首批公共修复后退出0。之后新增的首页行、导览 feature 与全平台 required ownership 声明只集中做最终编译，不逐项重复测试。
- 扩大 `animation` 测试过滤器触发既有调度测试失败和 `0xc0000409`；旧产物 `gpui-1489a80fdee4689d.exe` 也复现 `active_image_animation_immediate_requests_are_coalesced` / `animation_engine_frame_requests_are_coalesced_and_merge_drivers` 的 `left=1/right=2`，旧 click-notify 测试同样异常退出。完整测试仍阻塞。`cargo test -p gpui --all-features --no-run --locked` 因既有 `benchmark.rs:66/70` 缺失 layout owner 参数 E0061 未通过，不为通过本轮动画验证擅改该模块。用户要求不继续扩大测试后，停止追加测试/benchmark。
- 未完成：修改后 UI 快速反向、图像/文字像素等价、macOS/Linux 编译与 native 验证、四后端 GPU busy/Filter source/pass 对照及 Resize 黑块验收。未停止或替换用户正在运行的旧程序，不能把旧进程日志当本轮修复效果。此前“暂停首页迁移”由本次明确修复授权更新；本节是当前状态，其下审计记录保留历史事实。
- 最终批量检查（2026-10-16）：完成首页所有可见行与箭头、导览 feature 和 required ownership 声明后，`cargo check -p bmcbl --lib --locked` **退出0，0 errors / 506 warnings（既有警告保留）**；受影响动画实现文件定向 rustfmt check 与 `git diff --check` 通过。按用户要求未再逐项重跑测试；上述104+3+1+1测试证据属于前一公共修复阶段，不能冒充最后平台声明修改后的整套复验。没有新增兼容 re-export、旧方法包装、运行时开关、第二套 Scene 或 Root 缓存。

用户最新实测：设置页公共下拉目前正常、DX12 子窗口关闭正常；首页版本下拉由轻量对照提升为优化对象，标题栏 Tab 与其它应用动画需审查。窗口 Resize 跟随迟缓仍未解决。不能把关闭窗口修复或设置页改善推导为 Resize 已修复。

用户补充硬约束：**必须保持现有视觉效果再优化**。首页原有弹性展开/回收、逐行错峰、位移、透明度、启动栏锚点与快速反向连续性均为验收内容；不得用删除效果或替换为不同 reveal 观感换取较低负载。尝试的单底边 reveal 会改变观感，已撤回该首页源码试改，未作为优化交付。优化必须给出原轨迹与新实现的逐时刻几何/透明度对照，再做实际画面验证。标题栏 presentation 迁移也需继续核对过冲边界等视觉细节，功能和编译通过不等于视觉等价已验收。

用户进一步明确执行顺序：**先审查当前动画及 GPUI 公共链路是否存在重复 Capture、扩大整窗损伤、无效刷新 Filter 源，再确定视觉等价修复**；审查涵盖 GPUI 其它动画，不局限某个 Dropdown。暂停首页动效迁移与参数调整，不用缩减效果、禁用动画或限帧替代归因。当前截图 PID53812 显示首页开合 CPU6.3% / GPU15.6%，与历史公共 Dropdown GPU40%～50% 分开记录。该进程完整前段日志已解压到 `%TEMP%/bmcbl-53812-early.log`（49881行，0～4m58s）；后段 `%TEMP%/bmcbl-dropdown-current-53812.log` 混有 Resize，不直接作为固定尺寸 Dropdown A/B。运行产物 SHA256 `A2916B346BE24DF2A3077F2FAB0C8CC0D05377C29E9CA15891A870141F530EDE`。首页试改构建因运行程序文件锁失败，未替换该产物；首页源码已经恢复，不运行未验收的 reveal 方案。

本轮启动时 HEAD `14289e14a7935048deff2e085cdc0a50d3978f52`，工作区干净；此前修复已由用户提交。边界为可见动画执行 lane、相关失效/调度与 Windows Resize；不扩展到无关业务。仅使用本文件维护结果。

公共动画链路审查（同一基线；源码审查不等于逐组件实机验收）：

| 路径 | 已核对的行为 | 判断与后续验收 |
| --- | --- | --- |
| 普通 opacity / translation 与 packed visual group | `element/animation.rs` 的直接 primitive binding 不必为每个 child Capture；clipped translation、transform/rotation、clip reveal 则有 subtree Capture 语义 | 不能把出现 Capture 直接判为重复；替换 API 必须保持共同 pivot、clip、文字和阴影几何 |
| 嵌套动画 | `window/element_context.rs::with_scene_animation_impl` 在已有 parent animation 时将 parent 提升为 composite；当前 primitive 只能携带一个 animation id，直接移除提升会丢失父动画 | 确认存在额外离屏层的可达路径；尚未证明相同内容被无必要重复 Capture。优先关联嵌套层 id、source version、draw steps 后决定合并 |
| caller-sampled 动画 | `with_sampled_animation` 每帧新 identity，框架明确要求后代重画；`with_stable_sampled_animation` 稳定 identity，但仍由调用方采样并定向 invalidation | 首页 chevron 使用前者，Toast/Auth 使用后者；前者是确定的局部复用障碍，后者不能标成独立 GPU timeline。都不能单凭 API 推导整窗刷新 |
| 首页、标题栏及其它应用动画 | 首页容器与逐行动画仍有 layout target；标题栏 pill 已有本轮候选迁移。Tabs/Modal/公共 Dropdown 使用 retained 路径；Toast/Auth、欢迎卡片、任务完成/退出与 theme interpolation 仍有 CPU 采样或真实布局工作 | 这些是审查清单，不代表所有调用点已实机覆盖。任务 Enter 与 Complete/Exit 不能混为同一路径；3D viewport 持续帧须与 UI 动画分开 |
| Filter source 失效 | `renderer/filters.rs` 先比较身份、几何、内容及 sample，再检查 `gpu_sample_damage` 是否命中可见源；`sampled_child_pixels_changed` 会要求父源刷新 | 动画子像素变化导致父源更新可能是必要工作；不能跳过版本校验或强制缓存命中。是否有过宽源范围/重复层仍需同帧层级证据 |
| partial damage 与全绘制回退 | `renderer/present.rs:572–594` 中 native partial present 与 retained Root partial 是不同条件；两者均不可用时，即使 GPUI dirty 为 partial 也记录 full-redraw fallback | 已确认统计含义不同；后续必须同时记录 dirty、retained Root、native present 能力，不能把 fallback 计数当整窗损伤计数 |
| Translation 初始 viewport dirty bounds | prepaint 对 Translation 使用 viewport，paint 仅对 spring 收紧；但 `engine.rs::insert_driver_index` 排除已有 scene binding 的 UI visual track，平台拥有 scene animation 时使用 `tick_driver_with_compositor` | 仅凭保守 bounds 不能认定当前 DX12 动画每帧整窗损伤；需继续验证平台 ownership 与 fallback 可达性，暂不改损伤算法 |

当前 PID53812 日志的独立复核：`%TEMP%/bmcbl-53812-early.log` 的 **10–15s** 指标为 1192 draws / 1192 presents、prepaint 1544µs、backend draw 1919µs、706 次 renderer full-redraw fallback；1190 条活跃 damage gate 全部 `live_resize=false` 且无 full dirty。代表 scene162 为 5 GPU passes、11 filter source draw steps、218848 source pixels、27528 upload bytes、`retained_root=false` / `present_damage=None`。**15–20s** 指标为 782 draws / 734 presents、prepaint 1288µs、backend draw 13961µs、444 次 fallback；代表 scene5981 的 `full=true`、`forced=false`、`spatial_area=581256`，2 passes、19 filter source steps、582228 source pixels、47840 upload bytes；该段808条活跃 gate 也均非 live resize。这证实存在两类成本：局部 damage 的全绘制回退，以及实际 full damage；**没有 UI 开合事件/目标 id 或 GPU busy 百分比，不能将两段归因为 Dropdown，也不能重算历史40%～50%**。Trace 已开启，数值不能当低日志开销性能基准。

下一可执行切片是 P0-03 的事件因果闭环：保留公共 Dropdown、首页、Tab、Toast/Modal 的不同目标身份，关联 Capture/source-version/损伤理由与 Draw/Present；之后只修复证据支持的公共路径。暂不缩减动效，不依据 Capture 数量单独宣称根因，不扩大 Root Owner 迁移。

Translation ownership 补核：Wayland/X11 的 `owns_scene_animations` 恒为 true；Windows 使用 `pending_scene` 或 renderer 活跃呈现场景判定。`RenderOwner::has_active_presentation_animations` 在等待 frame-ready 时可能为 false，若此时没有 pending scene 且又触发 UI tick，才可能落入上述 viewport dirty 路径；目前没有该事件重叠的运行时证据。此项为**条件风险**，不能标为 DX12 持续整窗损伤的已确认根因。本轮公共链路审查未改 GPUI 源码、未重新构建或操作 UI；`git diff --check` 通过。

- 有效性纠正：Debug 覆盖主窗口时的边缘拖拽没有证实命中正确 native 边框，用户已指出该问题；这些操作剔除出复现证据。后续分窗测试，按 window_id 和实际尺寸变化判定有效，禁止将工具操作间隔当程序卡顿。
- Zed 参考：官方 [issue 10703](https://github.com/zed-industries/zed/issues/10703) / [PR 10758](https://github.com/zed-industries/zed/pull/10758) 的 modal-loop runnable pump 本仓已有对应机制（WM_MODAL_UI_TASKS / VSync / watchdog）。不把缺少任务泵当根因，也不照搬 Zed 同步 DX11 resize；继续关联 Client→Viewport→Scene→GPU→Present。
- 当前有效主窗口日志已保存 `%TEMP%/bmcbl-resize-main-14289.log`。其中 00:01:56～57 有连续 native/viewport/scene/present 记录；1536×1024 是 live-resize 容量，必须与 target_px/布局尺寸区分。黑块持续时间和感知延迟尚未验收。
- 标题栏审计确认 pill 使用两条 CPU spring + 每帧 left/width/layout target，而框架已有 `AnimationGroup::horizontal_edges`。本轮迁移到该 presentation API，保持原 spring 参数、稳定圆角/高度和快速反向 current-value retarget；NavState 删除另一份 CPU pill spring，只保留路由目标。响应式标签宽度是真实布局变化，仍使用局部布局动画。修改待定向测试与 UI 实测，不先宣称性能改善。
- 首页独立下拉的容器 height/top 与行 text top/opacity 仍逐帧重排；通用 Dropdown 已采用 retained reveal。首页应保持启动栏锚点与中断行为，优先迁移稳定行布局、clip/reveal；本轮尚未改动首页，不将首页和公共组件混为同一实现。

标题栏切片验证：`cargo test -p bmcbl --lib ui::state::navigation::tests --locked` **4 passed / 0 failed**，包含最新路由目标确认、快速重定向、标签断点滞回与中途反向；`cargo build -p bmcbl --locked` 退出 0（2m46s）。复用未改动的 GPUI 测试产物执行 `horizontal_edges_preserve_height_radius_and_crossing_order`，**1 passed**，验证高度/圆角/边缘交叉几何。真实 UI 测试先关闭 Debug，仅操作主窗口：首页→设置的采样段 **165 次成功 Present / chrome-nav Render 5 次**，最终 pill 对齐设置项；随后反向回首页可正常导航。原始段 `%TEMP%/bmcbl-nav-ui-settings.log`。这些数据证明连续 presentation 不再依赖逐帧 Nav Render，不是旧版同条件 GPU 利用率 A/B，也没有验证所有快速中断轨迹或多后端。

调用方检查：`cargo test -p bmcbl --all-features --no-run --locked` 首次因运行中的验证程序锁定 `BMCBL.exe` 退出 1；正常退出后增量重试退出 0，lib/bin/integration tests 编译通过。`cargo bench -p bmcbl --all-features --no-run --profile dev --locked` 退出 0，plugin_runtime/resource_parsing 等 bench 调用方编译通过；复用 dev profile 只做 API 编译检查，未执行性能 benchmark。定向 rustfmt check 与 `git diff --check` 退出 0。完整测试仍未宣称通过，前轮记录的其它失败不因本切片关闭。UI 验证产物 SHA256 `5512EFE3DA14E2E344369E34BF15291EB9DFCD2AF81937E8B3874431C015C1FB`；all-features 编译随后重建了可执行产物，不能混淆两者。

Resize 有效段统计（主窗口，00:01:56.272～57.666）：117 次成功 Present、116 个 Scene revision；native→首次对应 Present 116 对，p50/p95/max **6/9/46ms**；Scene→Present **3/4/43ms**。Present 间隔最大 210ms 的区间，约 203ms 没有新 native/Scene，随后输入到 Present 为 7ms，不能算作 GPU 卡了 210ms。三次真实 surface resize 耗时 **21.803/21.366/21.860ms**，submission_wait 均为 0，Swapchain 占 **15.052/16.930/15.392ms**。结束阶段有两轮同尺寸提交：57.551 queued 的 live 帧已在 57.553 Present；57.633 是新的 final settle queued，57.655 resize 完成、57.665 Present。final queued→resize 完成为 22ms，不能把上一轮 queued 混入而推导 104ms/82ms 未知队列等待。没有证据把普通连续 Resize 归为全面并发帧迟缓；黑块感知验收仍待独立窗口复现。

动画分族 inventory（源码搜索 + 定点核对，**不是逐调用点全量通过**）：

| 动画族 | 当前 lane / 已确认事实 | 剩余审查重点 |
| --- | --- | --- |
| 标题栏 pill | 本轮迁到双边缘 presentation，删除 CPU 副本 | 快速反转、标签断点变化与其它后端 |
| 首页版本菜单 | caller-sampled height/top 与行文字位移 | 固定最终行布局、retained reveal，保留锚点/stagger/中断 |
| 公共 Dropdown / Tabs / Toggle | 声明式 presentation | 多实例 ID、开合反向与最终移除 |
| Modal / 路由页面 | 主要采用 retained composite/presentation | 多层 modal ID、关闭中重开与退出清理 |
| Toast / Auth | 稳定几何 + caller-sampled retained property；不等于完全独立 GPU 动画 | CPU 采样/帧请求、堆叠与账号变化时 retarget |
| Loading / Linux runtime 进度条 | 多数声明式 presentation；Linux 条纹已声明 relative_translation，callback 只设常量 left，不判为逐帧布局 bug | 重复动画隐藏/停止时停帧；逐调用点核验 |
| Onboarding 卡片 | welcome-card-enter callback 动态 mt/scale/opacity | 纯视觉入口转 retained，保持文本几何 |
| 任务卡片 | Enter 为 presentation；Complete/Warn callback 改颜色/透明度，Exit 还改变 top/max_h/padding | 区分纯视觉脉冲与真实移除布局，避免文字重复排版 |
| 主题颜色 | frame clock 正确，多个视图仍请求 Layout 帧 | 颜色动画覆盖范围与 retained 色彩能力，不能机械取消主题渐变 |
| Map Viewer / Skin Pack 等独立窗口 | 3D motion/自动平移/行走有显式帧请求 | 失焦、隐藏、停止输入的停帧；不是 UI 2D Scene 删除对象 |
| 插件 DSL / 下载/管理/在线工具页面族 | 已做 API 搜索并抽查；多处声明式视觉动画 | 尚未逐一核对所有 ID、属性、时钟和中断，不能标记全仓完成 |

下一切片按用户最新要求先完成 GPUI 通用动画 Capture/损伤/Filter 刷新归因矩阵，再据证据选择视觉等价修复；独立窗口 Resize 感知与其它原任务保留。不得基于无新输入的 Present 空档修改 cadence，不用新增缓存掩盖 Scene 计算债。

### 2026-10-15 本机执行记录（HEAD `e47f7ff0d419df4e4715a231bb1f84ad084f5c27` + 未提交修复）

执行环境：Windows 11 build 26100，Rust/Cargo 1.98.1，工作区开始时干净，分支 `master`。以下严格区分原 HEAD 结果与未提交修复后的结果；不创建提交，不以旧程序或历史日志代替本轮验收。GPU 为 AMD Radeon 780M / RX 7600M XT，驱动 `32.0.31007.1017`；原生测试未固定 adapter，不能据此作性能 A/B。

| 阶段 | 实际命令与结果 | 对进度的影响 |
| --- | --- | --- |
| 原 HEAD 格式 | `cargo fmt --all -- --check`：退出 1，多 crate/应用已有格式差异 | 不执行全仓格式化；fmt 闸门未通过 |
| 原 HEAD 编译 | `cargo check --workspace --no-default-features --locked`：退出 0，RTK 汇总 0 errors / 507 warnings（27 crates） | 已取得该 feature 组合的编译证据，不代表全 feature/全后端通过 |
| 修复后 GPUI 编译 | `cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --locked`：退出 0 | 补齐框架规定的无默认 feature 编译检查；四后端测试 target 也已成功构建 |
| GPUI 测试编译 | `cargo test --manifest-path crates/gpui/Cargo.toml --lib --locked`（默认 DX12）：原代码 16 个编译错误；修复测试变量、ElementId、fixture 参数及私有状态测试归属后可编译 | 私有状态测试移动到所属模块，没有扩大生产 API 可见性 |
| GPUI 全测初次执行 | 1202 个用例，进程异常退出 `0xc0000409`；串行定位到 CachedView 范围预期 2、实际 3，失败清理又触发 panic | fixture 已修复为只统计两个叶实体；定向复验通过，完整单测仍被下行输入测试阻塞 |
| GPUI 全测再次执行 | CachedView 范围测试已通过；串行全测又在 `input::key_dispatch::key_dispatch_tests::test_input_handler_pending` 异常终止。单独复现 `key_dispatch_tests.rs:266`：文本实际 `"k"`、预期 `""`，随后 leak detection 清理 panic | **P0-01 仍阻塞**。不扩展为输入分发重构；独立 Owner/Fence 迁移与 UI 性能验收继续保持未开始 |
| Nova 定向复验 | Gaussian Capture 2/2、retained filters 2/2、chunk upload 13/13、Root color 2/2、Root 重绑定 1/1、buffer capacity 4/4，均退出 0 | 逻辑 Root 下线/重上、原生句柄重绑定和在途 slot 拒绝已有单测证据；不是独立 Fence 退休证明 |
| Nova 模块复验 | 最终四后端 feature 组合运行 `platform::nova:: --test-threads=1`：**314 passed / 0 failed / 27 ignored**，退出 0；另 CachedView 范围用例 1 passed | 修复测试 fixture 与 AutoStatic 顺序边界；不代表被忽略的全部硬件用例或 GPUI 完整单测通过 |
| 四后端 Root/Popup 原生像素 | `popup_reversal_preserves_static_color --ignored --nocapture --test-threads=1`：**4 passed**，退出 0；DX11/DX12/Vulkan/OpenGL，含 backdrop on/off、Sibling/Nested/NestedTranslation，以及 Root→AutoStatic 独立 Popup→Root 的真实逻辑下线/重上 | AutoStatic + 独立 Popup 与直接绘制像素一致；下线时 Root 内容版本清空，重上后新版本存在且局部绘制与完整重绘像素相同；仍不是独立 Owner/Fence 退休验收 |
| 四后端 FrameSlot 原生像素 | 同一测试二进制运行 `static_buffer_versions_preserve_in_flight_pixels --ignored --nocapture --test-threads=1`：**4 passed**，退出 0 | 现有 slot/version 在途像素测试通过；不推导 Resize、窗口销毁或新退休机制正确 |
| 四后端 AutoStatic 原生像素 | 同一测试二进制运行 `automatic_background_preserves_pixels_without_raster --ignored --nocapture --test-threads=1`：**4 passed**，退出 0 | 检查静态背景复用、源变化刷新及完整绘制像素对照；不是实际 Dropdown 性能采样 |
| 应用 Trace/TOML | `cargo test -p bmcbl --lib gpui_trace --locked`：初次 7 个测试编译错误（宏歧义 6 处、已删除 fixture 字段 1 处）；测试侧最小修复后 **2 passed / 0 failed / 923 filtered out**，退出 0 | 覆盖默认 false、旧 TOML 缺键、持久化及日志过滤独立性；不是 Trace 实际启停/重启验收 |

本轮边界：**P0-01 → 由失败直接暴露的 P0-04 正确性 → P0-06 已有生命周期测试补验**。AutoStatic 准入代码按职责移入 `renderer/retained_upload/auto_static.rs`，按唯一 `layer_id` 解析 `Scene::blurs` 中真实父合成 order，不放宽安全准入，不引入另一份 Root 缓存。扩充的原生场景还确认了动画注册与合成标记不匹配（真实 blur 1 / 标记 2）：现改为在插入 AutoStatic 合成标记前注册真实 Element 动画，保留原打包索引。**独立 Window GPU Owner / GPU Fence 退休迁移尚未开始；公共 Dropdown 与首页版本下拉同条件采样尚未开始。**先清除实际失败再推进看板依赖，不能把单测或 512×512 原生测试当作真实 UI 性能证据。

四后端构建/Root 运行命令：`cargo test --manifest-path crates/gpui/Cargo.toml --lib --locked --features nova-gfx-dx11,nova-gfx-vulkan,nova-gfx-opengl popup_reversal_preserves_static_color -- --ignored --nocapture --test-threads=1`。默认 feature 保留 DX12。其余定向用例复用产物 `target/debug/deps/gpui-f826ba1e5cf3386a.exe`，避免重复构建；所有结果均对应本节 HEAD 加本轮未提交修改。

差异检查 `git diff --check` 通过；新增 AutoStatic 模块、移动的私有测试模块及修改的两个原生 fixture 文件通过定向 `rustfmt --check`，全仓 fmt 的既有失败仍保留。受影响渲染 helper 的时钟审计仅发现 `retained_upload.rs` 中三处原有 CPU 性能计时，没有新增当前帧视觉采样时钟或 UI 每帧通知。未运行全 workspace/all-features 测试、bench、Clippy、真实 UI A/B、Resize/窗口关闭矩阵；原生 readback 会同步等待，不能单凭像素对照推导独立退休竞态安全。

本机原始输出存于 `%TEMP%/bmcbl-p0-*.log`（临时文件，非仓库验收附件）。**剩余阻塞：全仓 fmt 已有差异、GPUI 输入测试失败；P0-01 不能关闭，不能开始大规模 Owner/Fence 迁移。** P0-03/07 仍缺同条件真实 UI Trace/GPU 对照；本轮不声明 GPU40%～50% 已降低，不将 GPU Owner busy 当作 GPU 时间。

### 2026-10-15 UI 实测与后续基线（18:50 起，未验收）

- 输入阻塞已定向修复：Windows pending key binding 不应提交 `KeyDownEvent::text`；普通 listener 的 `stop_propagation` 仍保留原输入语义。`test_input_handler_pending` 单独运行 1 passed、退出 0。完整 GPUI 再运行仍有 text/cache/window 调度失败，不能关闭 P0-01。窗口用例逐个进程执行为 180 passed / 31 failed；不得通过放宽 coalescing 掩盖失败。
- 真实 UI 基线程序：`target/debug/BMCBL.exe`，构建时间 18:42:35，SHA256 `E4B4C5699A32398F5A3D8870256657A8826AE22A70BFF757C1DE42CDF9357274`。它不是最新未提交源码的验证产物。配置 DX12 / AMD Radeon 780M / `gpui_trace=true`；日志确认 backend 和 adapter，刷新周期约 4167µs。默认 Trace filter 已包含 `bmcbl::ui::render_trace=trace`，不能继续把旧的“target 未启用”推测当阻塞。
- **DX12 子窗口关闭崩溃已复现一次**：18:50:36 启动 PID 52508，点击 Debug 子窗口 X，主窗口短暂出现覆盖残影，随后进程退出。Event 1000（18:51:16）异常 `0x0000087d`、模块 `KERNELBASE.dll`。日志确认 child `4294967298` 关闭、remaining=1/quit=false，GPU owner 2 shutdown begin 后没有 completed。尚无栈；不能断言 fence 漏等。复现日志 `%TEMP%/bmcbl-dx12-child-close.log`。后续只增加 teardown 阶段日志定位，不先改销毁顺序或加入全局等待。
- **设置页公共下拉**：972×600，快速点击 launcher-display-mode trigger 10 次。关闭末尾 `dirty_full=true`、`dirty_area=583200`、`filter_source_draw_steps=18`、`blur_source_pixels=1057500`，随后仍刷新整窗源 583200 像素。983×600 重复后也出现整窗 589800 像素。证据支持存在关闭末尾整窗重绘；不等于重复 Capture 的根因已经定位。
- **首页对照初采**：相同进程、DX12/adapter/Trace、983×600，版本下拉 trigger 10 次。5s 桶观察到 draws=431 / retained_presents=400；设置页最近桶 draws=16 / retained_presents=260。桶不严格与操作起止对齐，不能直接做性能比值或认定首页更差。**用户确认首页版本下拉性能无异常；它仅作轻量对照。主问题是复杂设置页中大量周围元素是否被下拉失效牵连，不能偏移为修复首页。**后续补齐事件时间、相同活跃操作时长和复杂度影响。
- **Resize 初采**：系统尺寸调整实际产生 `live_resize=true`。1536×600 重建耗时 20533µs，其中 submission_wait=0、swapchain=16300µs、resources=4233µs；随后 983×600 为 21451/0/16971/4479µs。最大化到 1920×1032 为 40013/0/30723/9289µs。这些样本主要耗在 swapchain 重建，不能据此断言并发帧排队是黑块根因。电脑控制只取得操作后截图，尚未量化拖拽中的黑块持续时间。
- OS GPU engine 初采 CSV：`%TEMP%/bmcbl-settings-gpu.csv`，进程引擎峰值 11%，包含两个 LUID；短操作与低频采样未严格对齐，**没有复现或否定 GPU40%～50%**，不与 GPU owner busy 混用。Trace 分段保存在 `%TEMP%/bmcbl-dx12-settings-resize.log` 和 `%TEMP%/bmcbl-dx12-home-dropdown.log`。

本节是对上节“尚未开始 UI 采样”的后续更新：目前仅初采，P0-03/07 性能验收仍未通过。Scene 全量计算债、独立 Window Owner/Fence 迁移仍按看板推进，不用新缓存代替迁移。

18:56 当前源码诊断构建：`cargo build -p bmcbl --locked` 退出 0（1m03s）。产物 SHA256 `65D159FB4063836DFF3B96B145072FDC5182FE576E26831B3D1451EB872389DA`，18:56:49。再次点击 Debug 子窗口 X 后进程退出；新增阶段日志确认 submissions drained、extensions/path/filters/atlas/frames/depth 已经过，最后停在 `phase=swapchain`，因此将调查范围缩至 DX12 swapchain 销毁内部，而不是应用误退出策略。日志 `%TEMP%/bmcbl-dx12-child-close-current.log`；尚未证明具体 native 操作或修复。

后续 P0-09 修复与验证：细化日志确认 queue drain 返回 `Ok(())`，frame pacing reset 已完成，异常发生于 Swapchain COM 字段释放。代码核对发现 `submit_without_wait()` 在 `Present/Present1` **之前** signal fence；`has_pending_gpu_work()` 只对比该 submission fence，会漏掉后续 Present 队列工作。DX12 `destroy_swapchain()` 改为销毁窗口时始终追加一次 queue signal/wait，复用既有退休函数；普通帧和 Resize 路径不增加等待。[微软 Swap Chains 文档](https://learn.microsoft.com/en-us/windows/win32/direct3d12/swap-chains)明确 Present 在创建 Swapchain 时提供的 3D queue 上执行。

修复构建 `cargo build -p bmcbl --locked` 退出 0（42.18s），产物 SHA256 `4A5512F7656AF62086916C076EA70FEEF9838785668FF9F0238850B8F64F6ECF`。真实 UI 关闭 Debug 子窗口、F12 重开再关闭，**连续 2 次均有 renderer shutdown completed**（owner2/3），主进程 PID54836 存活且主窗口可交互；swapchain close 15871/15971µs，renderer teardown 27225/26398µs。随后主窗口正常退出。对照修复前 3 次复现崩溃；仍需其它子窗口类型、快速关闭/Resize 和后端矩阵验收，不能宣布整个 P0-09 完成。原始日志 `%TEMP%/bmcbl-dx12-child-close-swapchain.log` 与 `bmcbl-dx12-child-close-barrier-repeat.log`。

最新默认 DX12 GPUI 测试 target 构建退出 0；pending key、animation tick merge、background pointer recent-input、inert-hitbox recent-input 四项定向用例 **4 passed / 0 failed**。修复误将无交互背景点击延长 recent-input presentation 的路径，未放宽 coalescing。完整测试旧失败仍保留，未反复运行全测。

### 2026-10-15 本轮源码审计与偏移防护（父 HEAD `259fa1747c12ea24cc1f6745352992b8c3d85adb`）

**结论：重构目标未偏移，之前的状态描述却过度容易被误读为“Root 所有权/Render Graph 已完成”。** 此表覆盖历史小节的状态口径；历史章节记录当时源码，旧符号和过期行为不应作为本次或后续的实现依据。

| 核对对象 | 当前可证实事实 | 保持的任务边界 |
| --- | --- | --- |
| P0-01 编译闸门 | 本对话环境无 cargo/rustc/rustfmt，未得到当前 HEAD 的本地 fmt/check/test 成功输出 | **必须优先恢复编译与单测；此前“已提交”不等于编译通过** |
| P0-06 Native Root | `RootColorTarget` 负责 Texture/View 及 Pass/Composite ResourceSet 原生分配/释放，但仍嵌套在 `BackdropBlurTargets` 中，Root 零半径逻辑 Variant 仍借用 Texture/View | Window GPU Owner 独立、Fence 退休与 Root 逻辑 Capture 删除 **未完成** |
| P0-05/06 Root 关闭 | 无活动 Root 时，旧 Native Root 物理纹理不能被无 ResourceSet 的 Filter Variant 占用。本轮源码调整 `reuse_layout` 的选择以及 `rebind_zero_radius_layout` 的 spare 停放、再启用 | correctness 子切片，**新增测试源码但未运行**；容量不足可安全拒绝并重建 |
| Render Graph 名词 | `resource_set_for_config` 和 `direct_composite_target` 已读 Root Native IDs；仓库仍保留 `FrameUpload.retained_root_blur`、`SceneSlot`、`pack_scene`、`PresentationPacket.scene`。本处“Render Graph”指未来统一资源 DAG，不等于已经迁移了所有 DrawStep/Pass 消费者 | P0-06 仅部分消费者迁移；P1/P3 未完成，禁止提前删除现有绘制 ABI |
| Fence / Resize | Filter Target 拓扑变更有原有 submission preflight / wait，Resize、Window Destroy 仍从 Filter Chain 释放资源 | 没有独立 Window Root Fence 退休；需 GPU in-flight/多窗口/四后端实测，禁止把同步等待描述为独立异步退休 |
| 性能基线 | 公共 Dropdown 的任务管理器 GPU 40%～50% 与 GPU Owner busy≈20% 是不同统计。没有当前 HEAD 同操作、同后端、同尺寸、同模糊设置的 A/B | P0-03/04/07 归因未闭环；不得声明占用已降低 |

**不偏移执行顺序**：P0-01 编译正确性 → P0-02/03 Trace 因果链 → P0-04/05 真 Capture/物理复用 → P0-06/09 Native/Fence/Resize 正确性 → P0-07/08 端到端性能 → P1～P3 唯一持久 Paint/消除旧 Scene。为了避免实际 GPU 错误，允许提交有限 P0 正确性补丁，但**不得凭静态审查继续扩大未经编译的大规模所有权迁移**。BMCBL 业务 UI 缺陷不改变本 GPU 任务范围。

### 关键依赖 / 严格执行顺序

1. **先闭环 P0-01 + P0-02**：如果新编译器错误或 Trace 未生效，**先修正确性和可观测性**，不得继续加缓存、猜 GPU40% 的原因。保留用户并发提交的 UI 改动。
2. **再做 P0-03～05 的同一次采样**：先验证「是否获得静态前缀 Capture」，再检查「是否发生物理纹理重排」，最后才看「真正 GPU 离屏命令与 Pass 是否省去」。静态背景缺席则记录「不满足准入」，不是命中。
3. **若 P0-04/05 正确但 GPU 仍高，推进 P0-07/08；若仍重复 Recreate，优先查 P0-05 或 P0-06 的物理 Target 依赖；若 Resize 或子窗口仍崩溃，P0-09 优先于性能微优化。**
4. **有证据才推进架构**：P1-01→P1-02→P1-03→P2 资源/Barrier→P3 真删除。禁止先删除底层 Scene 后临时复制另一棵树保功能；中间迁移保持渲染正常但**最终无旧兼容入口**。

### 最小本地验证命令与实验协议（待执行）

```powershell
# Windows 代码与测试（在仓库根目录；按实际 Feature 增补 Backend 参数）
cargo fmt --all -- --check
cargo check --workspace --no-default-features
cargo test --manifest-path crates/gpui/Cargo.toml --lib auto_static
cargo test --manifest-path crates/gpui/Cargo.toml --lib gaussian_capture
cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_filters
cargo test --manifest-path crates/gpui/Cargo.toml --lib chunk_upload
cargo test gpui_trace
```

- 开启 `设置 → 启动器 → GPUI / Nova 详细 Trace` 并**完整重启**；默认关闭。文件 `BMCBL/logs/latest.log`；分别记录应用版本/HEAD、系统 GPU 使用率、驱动、窗口物理大小、DPI、240Hz/实际 Present Mode、后台模糊开关、采样起止时间与操作次数。Trace 会影响 CPU/log IO，测性能必须额外复测 Trace 关闭的版本。
- 固定一组 5–10 秒静止基线，再分别执行：公共 Dropdown 快速反复开/关、展开中打断与反向、两个 Dropdown 互斥切换、首页版本下拉对照、静态页面 / Blur 开关、页面切换与 Resize、多窗口打开关闭。每组单独记录，不能把多窗口或不同尺寸的样本混在同一 FPS/GPU 百分比中。
- **Capture 判定**：同一 `window_id` 对齐 `GPUI color retention result static_prefix_candidate/static_prefix_captured`、`GPUI retained target layout reused/recreated` 和 `GPUI AutoStatic submitted`；成功 Present 后稳定帧 `source_dirty=false`、`refreshed_layers=0`、`source_draw_steps=0`，而 `native_view/content_version` 与前次相同。`auto_static_relocation=None` 在无拓扑变化时正常；源内容/尺寸真变时版本刷新**必须**发生。
- **Filter 高水位判定**：首次超出物理容量的 3→4→6 可以分配；容量预热后的 6→3→4→6 不应周期性全链重建。比对 `active_isolated`、`allocated_isolated`、`composite_passes`、`blur_source_pixels`、`blur_target_pixels` 与目标 bytes。`target_bytes`/Owner busy 不是实际显存峰值或 GPU Shader 时间。
- **窗口与性能判定**：静态 idle 不持续无效提交；DX11/DX12/VK/GL 下 Popup 的可见内容、层次、Alpha、Clip、Blur、文字、焦点均一致；Windows 240Hz 拖拽、DComp/透明、小窗关闭、最小化恢复无黑边、拉伸或崩溃；最终记录 CPU Layout/Prepaint/Draw/Present p50/p95/p99 与可用的 native GPU 时间。对于尚无 GPU timestamp 的后端明确标记「GPU 原生耗时未知」，不造数。

### 当前缺失的验收证据（不得据此宣布已完成）

| 证据 | 当前仓库可核实状态 | 补齐后更新哪些任务 |
| --- | --- | --- |
| Windows Cargo fmt/check/tests 的完整成功输出 | 当前 HEAD workspace check 已过；fmt 失败，GPUI/应用测试编译及执行已暴露真实失败；修复复验见上方本机执行记录 | P0-01/P0-02/P0-04/P0-05 |
| 当前 HEAD 开启/关闭 Trace 的日志与设置重启截图 | 开关与过滤器源码已提交；**没有新的运行日志** | P0-02/P0-03 |
| 最新公共 Dropdown vs 首页版本下拉可对齐的逐帧 Trace + 系统 GPU 指标 | **只有旧版 2026-10-15 DX12 日志**（24 次 Target recreated；单帧 12 Composite Pass；GPU Owner busy≈20.6%–21.3%，与任务管理器 GPU40% 不可互换） | P0-03～08 |
| 四后端真机像素/Resize/子窗口关闭 + Fence/VRAM 回收 | Root 下线/重上、FrameSlot 在途版本、AutoStatic 像素用例各四后端通过；Resize/窗口销毁/独立 Fence 退休及完整矩阵仍待验 | P0-05/06/09、P2、P3 |
| 最终单权威 Paint/Compositor 与旧 Scene 消除 | 有 retained chunk/partial upload 改动；`Scene/SceneSlot/PresentationPacket.scene` 等仍在 | P1～P3 |


## 不可更改的工程目标

1. **统一 GPUI 的 UI 构帧、增量绘制、GPU 合成、Surface Present 语义**，降低端到端延迟、内存复制和显存/纹理反复分配。框架必须独立于 BMCBL，BMCBL 只是一个真实负载。
2. **采用破坏性替换，无旧链路兼容层**：不建立第二棵 Render Tree，不形成 RenderTree → Scene → RenderCommit → RenderPipeline 链式全量中间表示。保留已有 GPUI Element/布局/输入身份，最终移除旧全窗口 Scene 主链路；新增结构只能承担不可替代的职责，并应替换而非叠加旧所有者。必要 Property/Filter DAG 需用依赖与性能证据说明。
3. **失效分类**：UI/Layout、Paint/Content、Composite/Property、Surface/Presentation。纯合成变化不得误触发 UI Prepaint；不变内容不得重复编码与上传。
4. **GPU Owner 保留最新有效可呈现内容**；UI 任务、GPU 提交、Surface Resize、Present 各自有清晰版本和资源生命周期。禁止 Win32 WM_SIZE 路径承担重型同步构帧，也禁止以增加帧请求频率掩盖黑边。
5. **Root Color Cache 不应永久伪装成零半径 Blur**；Filter Capture/Target 需要稳定资源身份，避免弹窗变化令纹理全局失效。改造应包括预算、资源回收、Fence 生命周期。
6. **四后端等价性**：DX11 / DX12 / Vulkan / OpenGL；保留受支持的 Metal 路径，不以某一后端的专有假设污染统一合成层。
7. 不采用全窗口强制重绘、每帧 GPU idle、无上限纹理缓存或禁用滤镜作为所谓修复。

## 已确认的基线与原因边界

2026-10-14 用户的 DX11 Debug 日志（非四后端性能结论）：

- 5 秒：806 requests / 474 Draw / 446 Present。存在频繁请求与合成；持续动画时不应将请求数直接等同浪费。
- 一次 91.6202 ms 构帧：**Prepaint 86.753 ms**、Paint 3.829 ms、Scene Finish **0.028 ms**。不能断言 Scene Finish 是长尾主因；首先查 layout/text/CachedView/依赖失效。
- Retained Target Layout 多次在 2～6 个 active variants 间重建。日志报告的目标容量变化从约 4.67 MB 至约 27.99 MB；**不等于实际分配额或 GPU Stall，需资源事件/时间戳确认**。
- 最新一帧 scene_primitives=46, scene_batches=12, encoded_primitives=109, encoded_batches=31, gpu upload=6144B, composite_passes=2, retained_copy_pixels=583200。局部逻辑 Scene 很小，但编码/合成布局可能额外扩张；继续查独立 Color/Filter 语义。
- `crates/gpui/src/platform/nova/frame_upload/encode.rs::append_retained_root` 使用零半径 Blur/Render Target 表示根颜色驻留；`renderer.rs::ensure_backdrop_blur_targets` 在布局不兼容时会重建并可能等待已提交 GPU 工作。
- `crates/gpui/src/platform/windows/window.rs::dispatch_native_resize_frame` 仍在原生 WM_SIZE 栈内执行 `dispatch_pending_update_with_vsync(None)`，须测量 UI 长尾和新旧 HWND / Layout / Surface / Present 尺寸代次。
- DXGI opaque HWND 当前 Scaling=None；这可能导致 Resize 期间旧画面未覆盖扩大的客户区，**须按窗口类型进行实机 A/B，尚非最终根因**。


## 破坏性重构决定：消除多重表示与技术债（2026-10-14 增补）

### 真实模块盘点：先判定“存在”再决定删除

基于远程 master 的当前源码目录和入口核对：

| 名称 | 当前事实 | 最终决定 | 严禁误删 |
| --- | --- | --- | --- |
| render_tree / RenderTree | crates/gpui/src 下**不存在独立 render_tree 模块** | 不新增完整树；不把 Scene 更名为 RenderTree | 已有 Element、Taffy、Dispatch Tree |
| render_commit / RenderCommit | **不存在独立 render_commit 模块** | 不新增与既有提交协议并行的全量持久对象；统一帧变更通知 | 必要的线程安全、版本化消息 |
| scene.rs + scene/ | 确实存在 Scene、paint_operations、Replay、Finish、Batch、动画边界、Blur 等 | **删除旧整窗口 Scene 作为每帧输入/输出边界的全部职责与模块**；必要几何图元定义归入各自所有者 | 真实 Quad、Shadow、Text、Path、Blur、Surface 功能 |
| render_pipeline.rs + render_pipeline/ | 不是另一套正在运行的渲染管线；实际为 renderer_backend.rs、shader.rs、svg_renderer.rs | 最终**拆散/删除含糊的 render_pipeline 目录及聚合 re-export**：RendererBackend/Options/Policy 归入平台渲染配置；WGSL Shader 类型归入 Shader/Resource 所有者；SVG Renderer 归入图像/资产渲染所属模块；更新全部调用方 | 不得因目录改名就丢掉公开功能、设备选择、WGSL、SVG |
| Frame / SceneSlot / CachedView | UI Frame 包含输入、布局与 Scene；SceneSlot 通过 Arc 共享 immutable Scene | 保留 UI 输入状态与必要 scratch；删除独立 SceneSlot、Scene Index、基于 Scene Range 的 Paint Replay | Dispatch、Hitbox、IME、Focus、TabStops、Listeners |
| PresentationSnapshot/Packet、RenderOwner | 目前按完整 Scene Snapshot 进行 pending/active/latest-wins 和 GPU owner 排队 | **只保留一种 GPU 消费的权威绘制状态和版本化更新事件**；合并重叠的 Frame / Presentation 数据所有权 | Native Size / Layout / GPU Fence / Presented 语义 |
| Nova FrameUpload / retained_upload / draw_steps | 目前含重复 pack/encode/signature/cache 与 GPU FrameSlot staging | 以 GPU resident 编码和变化范围复用替换多层全场景中间副本；DrawStep 仍保留必要的 GPU 命令编排，但不因一次局部变化重建全部记录 | 有用的 GPU Buffer、Atlas、Mask、Barrier、Frame Resource |
| gpui-3d::Scene | crates/gpui-3d/src/scene.rs 下独立 3D 领域类型 | **不属于本次删除** | 3D 模型、地图与皮肤预览 |

注意：删除 render_pipeline/ 是**文件与职责归属清理**，不应混同消除 GPU 真实编码/Render Graph/Draw/Present 功能。中间阶段可以有函数，但不能各自拥有第二套全窗口状态与生命周期。

### 最终目标：一条唯一权威的渲染合成链路

~~~text
GPUI View / Element / Layout / Input （唯一 UI 状态所有者）
  ↓ 依赖失效（Layout / Content / Composite / Surface）
局部计算 + 已有 Element/CachedView 的有效性校验
  ↓ 仅变化片段
持久化 Paint Artifact （有序 Chunk / Property / Filter 依赖；不含额外全量树）
  ↓ 同一条版本化变更通知（不再包裹完整 Scene）
GPU Owner 持有的唯一权威 Compositor 状态
  ├─ 稳定 GPU Draw Range / Buffer / Atlas
  ├─ Color Layer 与 Filter Target（分别管理身份、内容、回收）
  ├─ Animation / Clip / Transform / Damage
  └─ Surface Geometry / Fence / Frame Resources
  ↓ 根据现有资源版本生成必要 GPU 命令
Nova-GFX DX11 / DX12 / Vulkan / OpenGL / Metal
  ↓ Native Swapchain Present（版本验证与窗口覆盖）
~~~

这里的 Paint Artifact 是**同一份持久绘制内容**，不是另一个全量 Scene、RenderTree 或逐帧重新生成的全量 FrameUpload。GPU Owner 根据它维护已驻留的后端资源；允许为 GPU 并行性保留多 FrameSlot、Staging Ring、已提交旧纹理直到 Fence，但这些是不同生命周期的硬件资源，**不能复制第二套权威逻辑绘制状态**。

### 统一所有权：每份数据只能有一个权威修改者

| 数据 | 权威拥有者 | 允许复制/保留 | 禁止 |
| --- | --- | --- | --- |
| Element、布局依赖、输入命中 | GPUI UI Thread | 不可变已提交布局/命中版本 | GPU 线程操作 View/Context/RefCell；重建第二棵等价 UI 树 |
| Paint Artifact / Chunk 身份与顺序 | 单一持久绘制内容管理器（可由 UI 编排、GPU 镜像索引） | 引用、版本号、局部不可变内容 | Scene + RenderTree + RenderCommit 全量三份并行维护 |
| 局部变化与 Damage | 单一版本化更新流 | 未呈现 Damage 的合并结果 | 直接丢弃 Create/Remove/Resource 操作造成状态不完整 |
| GPU Buffer/Texture/Atlas/Layer | GPU Owner | Frame-in-flight 和 Fence 退休队列 | RenderGraph、FrameUpload、FilterPool 各自独立拥有同一物理资源 |
| Presentation 状态、Native Surface | 平台 Surface Controller + GPU Owner | last-valid image / epoch 快照 | 把 UI Layout 完成、GPU Submit、Present 和屏幕显示混为一谈 |
| Filter DAG、Property/Animation | 对应 GPU Compositor 数据所有者 | 小型依赖索引和变更值 | 无变化重建完整 DAG 或让根颜色缓存依附 Blur 插槽 |

**最大化减少传输层**：不改变内容的帧，UI → GPU 传输的静态图元字节数应为零；变更仅传局部内容或属性值，不复制全部子树、不进行全局 Scene equality/Hash/Batch 重建。只有资源依赖、硬件缓冲复用、裁剪/滤镜必须时才产生新的编码和上传。

### 强制删除/替换清单（每个条目必须有代码证明）

- [ ] 删除 crates/gpui/src/scene.rs 与 scene/ 的旧全窗口容器/操作记录/排序/Replay/Finish；必要基础 Primitive/Bounds/Geometry 定义**迁移并更新全部调用方**，不保留只转调旧 Scene 的适配器。
- [ ] 删除 crates/gpui/src/window/frame.rs 中 SceneSlot、Frame.scene、PaintIndex.scene_index、retained_scene_index，以及相关无效范围修补；输入状态/布局生命周期保留在 UI。
- [ ] 改写 window/draw.rs、draw_reuse.rs、retained_identity.rs、element/div/、element/cached_view.rs：Retained 子树可以复用真实有序绘制块，不再在下一帧复制整个旧区间，且匿名 ID、父自身与子树交错的 painter order 保持正确。
- [ ] 改写 window/paint.rs、window/composite_layer.rs：图元、Clip、Blur、Opacity、Surface、RendererExtension 的变化直接作用于持久绘制内容；不产生嵌套 Arc Scene。
- [ ] 消除 platform/frame.rs::PresentationPacket.scene 与 window/state.rs::PresentationState 的全量 Scene 快照双份协议，统一状态变更/版本/合成属性，并保持 UI 与 GPU 的线程隔离。
- [ ] 改写 platform/render_owner/queue.rs、worker.rs：增量数据基于已应用版本；队列压缩不能遗失 Create/Update/Remove 与资源、Resize、Shutdown 的依赖，不需要每个提交另存一个完整绘制快照。
- [ ] 删除 NovaRenderer::pack_scene 与 scene_revision 主导的全部缓存键；重用实际未变化的 GPU Buffer/DrawStep/PathMask/Atlas 与渲染命令，同时不要让一个变更导致全局 Encode/Upload。
- [ ] 移除 FrameUpload 的**全场景 CPU staging 容器职责**，保留必要的小块编码器、紧凑动画参数、FrameSlot 上传 Ring 与一次性 Backend Batch；不能再新增等价的 RenderPacket 中转。
- [ ] 拆开零半径 Root Color Cache 与真实 BackdropBlur Target：分别拥有尺寸/内容/资源代次、纹理池与 Fence 退休；Popup 频繁展开不得无故抖动整条 Target Chain。
- [ ] 删除/拆散 crates/gpui/src/render_pipeline.rs 和 render_pipeline/ 的错误聚合职责，按上述真实所有者搬迁三个现存模块并全量更新编译/测试/文档引用。这里是**整理功能归属**，不是删掉 Shader、SVG 和 RendererOptions。
- [ ] 同步更新 platform/nova、windows、linux/x11、linux/wayland、mac/metal、Blade、StatusItem、gpui examples、benchmarks、测试；不保留 GPUI 的旧 Scene 后端入口。
- [ ] 删除原 Scene 专属日志、指标与迁移期死代码，修复过时文档和多套互相冲突的架构图；只保留本文作为下一次对话状态源，历史审计文档明确标记 snapshot。

清理实现时：以**源码符号引用**验证，不能只执行文本关键字删除。Rust 中合法存在的 3D Scene、设备后端 Shader、渲染命令和 Render Graph 不因为命中关键字就删除。

### 必须解决的七个端到端链路问题

1. **元素计算 → 局部绘制**：谁生成 Dirty？谁向祖先传播？为什么未变化 View/Render 被重构？Taffy / Text / CachedView / Prepaint 是否仅处理必要节点？优先消除 86ms 等长尾，而不是仅删除几十微秒的 Scene Finish。
2. **绘制内容 → GPU 数据**：一次按钮/Dropdown 更新带来多少 RenderRange 变动？Quad、Glyph、Image、Path 是否多次序列化/Hash/Copy？连续帧的传输字节必须可追踪，不变资源应直接驻留。
3. **合成属性 → GPU 动画**：纯 Transform/Opacity/Color 不应重新 Layout 与 Paint；动画结束停止唤醒。过滤属性、文本排版和实际命中保持正确，不能以 GPU 动画造成交互错位。
4. **颜色缓存 → Filter DAG**：Root Color、Background Blur、Element Blur、Clip/Mask 的内容/分配生命期独立。拓扑数量变化尽量复用目标容量，但不复用错误的旧像素；不得无上限堆积纹理。
5. **队列延迟 → Frame Pacing**：UI Update / compositor tick / resource ready / Native Surface 分别调度、合并重复请求；报告从 Dirty 开始到**真正成功 Present** 的延迟及 p50/p95/p99，不拿 GPU owner CPU Busy 当实际 GPU Shader 时间。
6. **Win32 Native Resize → GPU 覆盖**：WM_SIZE 与重型 UI 构帧解耦；记录 HWND client、Layout、GPU Drawable、成功 Present 的尺寸/epoch；新 UI 未就绪时让旧有效画面以适合各窗口类型的方式覆盖全客户区。不能把失配 Scene 当作新尺寸内容。包括 DX11 不出帧、DX12 子窗口崩溃、Vulkan 拉伸、GL 卡顿的专项回归。
7. **资源提交 → 回收**：所有后端保护 in-flight Buffer、Texture、Descriptor、FBO、Swapchain；资源分配/复用/退休由单一所有权控制，有显存预算、内存碎片统计和关闭窗口后的回收证据。严禁通过每帧 wait_idle 掩盖生命周期。

### 反技术债验收合同（未通过不得标记“完成”）

| 验收项 | 必须提供的证据 |
| --- | --- |
| 删除旧结构 | ripgrep 搜索及新 GPUI 主干源码证明：无 Frame.scene / SceneSlot / Scene::replay / Scene::finish / pack_scene / PresentationPacket.scene / Scene revision cache；无二次全窗口容器伪装 |
| 单一数据权威 | 每份画面内容的所有权、变更源、GPU resident handle、资源退休路径可追溯；不允许全量 Frame → 全量 Scene → 全量 Upload → 全量 DrawStep 多份副本 |
| 局部事件效率 | Dropdown Hover/展开/回收、单卡片动画、单字输入、列表滚动的 Layout Nodes / Paint Chunks / Encoded Bytes / GPU Upload / GPU Composite Pixels 有量化记录 |
| 串行延迟 | UI invalidation → Layout → Paint → GPU data ready → Submit → Present 的阶段直方图与 p95/p99；不允许输入延迟改善仅靠延迟 UI 事件 |
| GPU 时间与实际占用 | DX11/DX12/Vulkan/OpenGL 的 GPU timestamp/query 或可比的原生测量；禁止把 CPU 调用时长等同 GPU 花费 |
| 纹理生命周期 | 重复 Popup 与 Blur 变化：物理分配、池命中、失效、Fence 退休、显存峰值/回落都可证明；没有 target_layout_count 持续抖动导致全链重建 |
| 静态场景 | 静态、无动画窗口没有不必要的 Draw/Present/资源分配；纯合成动画无 UI 全量构帧 |
| Resize | Native/UI/Surface/Presented 代次在静止后收敛且像素实际覆盖；Opaque、透明、DComp、多窗口、DPI、最大化、缩小、移动与 Resize 分别验证 |
| 跨后端 | DX11 / DX12 / Vulkan / OpenGL 共用相同的局部绘制/滤镜/Alpha/资源语义；不能通过关局部裁剪或强制全窗口绘制取得“通过” |
| 内存与维护 | Allocations/frame、RSS、GPU residency、peak、pending retire、idle trim、Frame/Resource owner 数量；删除重复所有权与未使用的 Feature Flag / adapter / obsolete doc |
| 工程质量 | 实际 cargo fmt/check/test 命令和结果、没有未经验证的性能提升宣传、Windows GPU 实机与非 Windows 状态分开标记 |

**注意**：允许特殊后端或滤镜为正确性在最终 Swapchain 执行整目标合成，但不等于允许 CPU 再构造、上传全部不变原语。完整 Present、GPU 栅格化、CPU Encode 与 UI Prepaint 是不同成本，需分别统计。

### 执行排序：端到端纵切，允许最终删除时一次性破坏内部 API

- S0（当前）：依赖与成本审计、完整基线、确定职责所有者；本文和历史审计一致，禁止再扩展新 RenderTree 原型。
- S1：在至少一个原生后端实现单一持久绘制内容、局部绘制与 GPU resident 纵向通路；不改完所有后端不允许宣布完成。独立 GPUI 最小测试必须验证静态、Dropdown、Blur 与 Input。
- S2：在共享 Nova 层统一 Filter/Root Color/Target 资源与 Present 调度；并行独立解决 Win32 Resize 同步构帧与代次覆盖，不能等待 S4 才修黑边。
- S3：将 DX11/DX12/Vulkan/GL 接到**同一个**通用合成语义和提交接口；对于 Metal、Blade、Linux consumers 同步迁移或明确可编译地退出支持范围，不保留老 Scene 后门。
- S4：彻底清理 Scene / render_pipeline 旧模块与所有引用、重复临时结构、兼容适配器、过时文档；保持 GPU Barrier 和线程所有权安全。
- S5：全场景 Correctness + GPU 时间/内存/显存/帧延迟回归，以旧基线对比；所有测试不执行则明确未完成。

最终完成 = **旧全窗口 Scene 真实删除 + 不存在另一个等价全量 RenderTree/FrameUpload + UI 局部计算可量化 + GPU 资源跨帧复用可验证 + Windows Resize 正确 + 四后端等价 + 不留兼容/遗留文件技术债**。不得仅以编译通过、减少一处分配或新增 TODO 证明整项工作完成。

## 分阶段历史实现账本（勾选仅代表提交过代码/测试源码；当前状态以顶部执行看板为准）

### P0-A 共享 GPU Filter / Color Target 生命周期

- [x] **本轮代码（2026-10-14，单一路径压缩）**：删除重复的 `reuse_stable_prefix_layout` 入口，合并进 `reuse_layout`；实现按稳定 Capture 槽身份一对一重排（包括非前缀顺序、缩减活动数），直接搬移 GPU target descriptor/resource-set handle 而不重分配 GPU 纹理；不兼容时保留原有根颜色特殊映射 / zero-radius rebind 路径。新拓扑复用后统一强制刷新滤镜内容与 DrawStep，杜绝错误采样旧像素。
- [x] 新增模拟 GPU TextureView 身份重排、失败不修改旧状态的单元测试；**进一步改为原地 Vec::swap 重排**，防止复用物理 GPU 纹理时却反复分配 CPU Target Descriptor Vec。测试检查 Vec backing pointer 在变更前后不变；单元测试尚未运行。
- [ ] **本轮测试未执行**：GitHub 远程编辑环境不提供 Rust/Cargo；需本地运行 `cargo fmt --all -- --check` 和 `cargo test --manifest-path crates/gpui/Cargo.toml --lib gaussian_capture`，再做 Windows 4 后端高频弹窗 Native GPU 回归。不得将代码已提交视为性能证据。

- [x] 审计 Zero-radius Capture、Root Retained Color、目标布局复用路径和最新日志。
- [x] 增加**稳定前缀布局**复用：在现有 GPU 纹理规格/目标身份均不变且只改变活动 Capture 数时，保留已有分配，不再走完整目标链重建；覆盖 Gaussian Source 情况。变更后保守刷新 Filter Cache 和 DrawStep。
- [x] 为上述稳定前缀路径增加无实际 GPU 依赖的单元测试，确认 TextureView 身份保留及不兼容时拒绝复用。
- [x] 为无法复用而需要 Recreate 的情况补充拓扑、Root Source、Variant 计数和 in-flight 数量诊断。
- [ ] Windows 真机确认新增路径命中率、纹理重建次数与 GPU/CPU 耗时（DX11/DX12/Vulkan/OpenGL）。
- [ ] 分离 Root Color Residency 与 Gaussian/Backdrop Filter Target；取消零半径 Blur 伪合成的结构性耦合。
- [ ] 过滤布局以稳定 Capture ID/Generation 表示，避免扁平索引扰动带来的全链路失效；处理跨帧 Fence 与资源身份。
- [ ] GPU 纹理预算、空闲 Target 复用与高频弹窗的压力/泄漏测试。

### P0-B 帧请求、UI 计算与 Windows Resize

- [x] **2026-10-15 Prepaint 选择性重放正确性修复（代码层）**：将 `reuse_prepaint_fragment` 的 DispatchTree parent-map 验证/节点转移置于 Hitbox、Tooltip、ElementState、TextLayout 变更之前，避免因无效父路径返回 false 时已消费旧 Tooltip 或残留输入命中状态。依赖 `DispatchTree::reuse_fragment` 已有的全范围预验证保证其失败时不搬运 listener；没有新增双缓冲或回退协议。
- [x] **2026-10-15 元数据搬运临时数组收敛（代码层）**：`replay_retained_element_metadata_fragment` 移除 `SmallVec<[bool;64]>`，由唯一 `included_prefix` 的相邻差分判定纳入集合，同时提供重排后的 offset；添加 256 种 8 节点位图的计数与区间完整性单测（未运行）。这是一次临时数据结构删除，不等于旧 Scene/Replay 已删除。
- [ ] **待执行**：本地 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib compact_inclusion_prefix` 和 CachedView 嵌套 Prepaint/Focus/Tooltip/IME 验证，尤其是失效后 fallback 的无副作用性。当前无 Rust 工具链，尚未实机确认延迟收益。
- [x] **代码修改（2026-10-14，Win32 消息栈解耦）**：删除 `dispatch_native_resize_frame` 及从 `WM_SIZE`/`WM_WINDOWPOSCHANGED`/`WM_EXITSIZEMOVE` 直达 `dispatch_pending_update_with_vsync(None)` 的同步构帧分支；原生事件只对齐 HWND Client 尺寸/epoch、排入最新 Resize/UI 请求，交给现有 modal VSync/foreground-task/fallback timer 消费。不新增 Frame/Scene/RenderCommit 状态或兼容入口。
- [x] **代码诊断（未运行）**：在 `dispatch_pending_resize` 的消费路径记录达到 16ms 的 resize callback 耗时与尺寸代次；该耗时是 CPU callback 墙钟时间，**不是 GPU 时间，也不能代替 GPUI Prepaint 分项计时**。
- [ ] **仍须验证**：Rust fmt/check、WM_SIZE→modal VSync 消费与 Windows 四后端实机 Resize，重点检查扩大窗口时旧图覆盖、DX11 不出帧、DComp/透明子窗口、失活/无 VSync fallback；此项不能被当作黑边已解决。
- [ ] 详细计数与标注每次 Frame Request 的 UI/Paint/Composite/Surface 原因，消除同一时钟重复唤醒。
- [ ] 剖析 Prepaint 长尾：Taffy Bounds 缓存失效、Text Layout、CachedView Selective Splice、Overlay/Dropdown 的具体触发者。
- [ ] 将 Window Resize Native Message 快路径与可能耗时的 Layout/Prepaint 解耦；确认 Win32 modal event loop 仍可及时调度 UI。
- [ ] 建立 Native/UI/Surface/Presented 代次与最终像素覆盖诊断；区分窗口 Move 与 Resize。
- [ ] 复现并定位 DX12 子窗口关闭异常、Vulkan Resize 拉伸、DX11 不出帧和 OpenGL 交互卡顿。

### P1 统一帧内容表示与合成提交

- [x] **2026-10-15 自动背景 Source Scene Replay/Finish 全部删除（仅代码实现，未经 Rust 编译和 GPU 实机验证）**：原来的 `auto_capture_static_background` 不再创建 Background `Scene` 或 `Arc<Scene>`。使用现有唯一的 `FrameUpload` 直接从已编码、排除 Clip 退化的 Quad/Shadow 有序 Batch 中计算源内容指纹（类型、实例数、打包字节和顺序），原位插入已有 Blur Marker；新源只保存 Bounds/ContentMask/Hash/Bytes，而非第二套 Paint 所有权。
- [x] **统一 Filter Source 输入类型（仅代码实现）**：`FilterSource::Element(PaintBlur)` 保留嵌套 Element Blur 的原有 Scene/动画语义，`FilterSource::AutoStatic` 仅保存 GPU 静态源内容身份；同一个 `FilterRegistry` 依据成功 Present 的离屏目标刷新单调 `TextureContentVersion`，新旧 Source 类型变化不可复用旧目标。自动静态源内容指纹改变时强制整层刷新，避免脏矩形为空或不足导致旧纹理被误保留，原有 Filter 创建/退休机制不变。
- [x] **单测源码（未运行）**：静态内容变动失效、前景动画变动不失效、Filter 内容版本只在成功刷新时推进、AutoStatic 与真实 Element 类型隔离、Quad/Shadow Batch 切分与空 Clip 退化回退。
- [ ] **未完成验收**：本地 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib auto_static`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_filters`，Windows DX11/DX12/Vulkan/GL 的 Dropdown/Blur/动画/Resize/切换页面及目标 GPU Timing。无 Rust 工具链，不能声称测试通过或性能改善；Scene/SceneSlot/全局 Replay/FrameUpload 主链路仍未删除。
- [x] **2026-10-15 自动背景捕获删除整份 Foreground Scene（代码层，未编译）**：`auto_capture_static_background` 不再 `Scene::replay(prefix_end..)` 构建动态前景、副本 `Scene::finish` 后再次编码；只构建一份静态背景 Source Scene 用于原有 Filter 内容版本/失效证明。`pack_scene` 直接对唯一原始 Scene 编码一次，再由 `FrameUpload::capture_encoded_static_prefix` 按原始 DrawOrder、排除零面积 Clip 后的 Quad/Shadow GPU 实例索引，原地切分最多一个混合 Batch，插入 BeginBlur/EndBlur/CompositeBlur，重建现有 FilterConfig。动态数据的 Source Batch 无额外完整 Scene 或二次编码。不能安全切分时不修改编码结果，回退已有直接绘制/Root Color 策略，而不是保留原 Foreground Scene 兼容路径。
- [x] **2026-10-15 测试源码（尚未运行）**：静态100 Quad+动态1 Quad（两段 Batch）、静态100 Shadow+动态1 Shadow（Shadow 单 Batch 内切分）、裁剪掉动态后的拒绝/无副作用、旧/新 Source Content 改变正确失效、Blur marker/source 范围。必须测试 Layer 嵌套、不同 Solid 混合、滚动、DPI/Resize、动画和 Filter Target Fence。
- [ ] **仍未完成**：最终删除静态 Background 的 `Scene::replay/finish` 与 `Arc<Scene>` 内容身份依赖。应让持久 Paint Chunk/Source ContentVersion 取代 Source Scene 的深度对比，在 FrameSlot 和 GPU Filter Owner 同一代次下管理。避免新建 RenderTree/RenderCommit 或 FrameUpload 第二份全场景容器。
- [x] **2026-10-15 直接编码 GPU Root Color 合成记录（代码已提交，单测待执行）**：`append_retained_root` 删除只为序列化零半径 Root Blur 而分配的 `Arc<Scene::default()>` 与临时 `PaintBlur`。生产编码共用 `write_paint_blur_fields`，实 Element Blur 包装器转调相同字段 ABI，保持原 136 字节缓冲布局、合成顺序与四后端 Shader 语义。该改动只移除**空 Scene 的错误构造**，不应声称真正静态背景的两次 Scene Replay 已删除。
- [ ] 针对 Root Color 合成 ABI 的 Rust 单测/性能计数、四后端静态/Blur/弹窗实机仍待执行。
- [x] **2026-10-15 Retained Quad 当前布局校验与描述符分配去重（仅静态代码）**：`QuadResidentLayout::from_upload` 从原先的 `Vec::clone() → Arc::from(Vec)` 双重分配，改为从 `&[RetainedResidentSpan]` 直接构造唯一不可变 Arc。局部上传前同时校验 old/current Chunk byte range 的非空、排序、无重叠和当前 buffer 上界；当前布局损坏即回退 Full，避免错误跳过上传。允许旧布局长于缩小后的当前 buffer，并对这些边界添加单测（未运行）。
- [ ] **本地验证**：`cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib chunk_upload`、Windows Native 多 FrameSlot/Resize；此改动不意味着底层 Scene/FrameUpload 的重复权威状态已被清理。

- [x] **2026-10-15 Scene Finish 候选隔离证明去重复扫描（源码已改、测试未运行）**：`Scene::prepare_retained_quad_chunks` 在候选数≥8 且 PaintOperation 数≥128 时，惰性构建一次仅含 `DrawOrder` 的临时排序索引，用二分统计候选范围内的所有 Primitive 操作数，替换多候选的重复全量 `paint_operations` 顺序扫描。候选操作已经全部证明为 Quad 且其最小/最大 DrawOrder 正是范围端点，所以全局计数等于候选 Quad 数与原有“范围外没有同 Order 操作”条件等价；重复 Order 与嵌套 Blur 子操作仍参与计数。小场景原扫描保留且不建立索引。新增 32 种长度下所有连续区间的等价性单测源码。
- [ ] **仍须实测**：比较多个 CachedView/Dropdown 同时重放时 `scene_finish_us` p50/p95/p99、内存分配与短片段阈值，不得直接归因于 Prepaint 86ms；CI 不替代 cargo 本地执行。
- [x] **代码提交（2026-10-15，Retained Chunk 区间化编码）**：`FrameUpload::encode_retained_quads` 不再为每一个 Quad Batch 全量扫描所有 Retained Chunk；直接使用 `Scene::finish` 已建立的**按 Quad 索引排序、互不重叠**的有序片段，经两次 `partition_point` 截取相交部分。保留 Chunk 相邻范围、Solid/Non-solid、Cache 命中、Painter order 的原有检查。新增与旧全量扫描结果对照的覆盖测试（仅编写，尚未执行）。
- [x] **Scene Finish 低成本剪枝（未实测）**：对候选 Retained Chunk 先检查已排序的 Quad Buffer 范围完整性，再执行全 `paint_operations` 顺序隔离证明，避免无法表示为单个 GPU 范围的候选触发昂贵的全表遍历；不修改正确性判定，不新增 Scene 索引/RenderTree。
- [ ] **仍未完成**：持久 Chunk 唯一所有权、Paint/Prepaint 粒度失效与 Scene/FrameUpload 删除；需运行 `cargo fmt --all -- --check`、GPUI retained scene 与 nova static_dynamic_batch_tests，以及 Windows GPU 实机。性能收益尚无基准数据。
- [x] **代码修改（2026-10-14，Retained Quad 描述符去重）**：复用完全相同的有序 `QuadResidentLayout` Arc backing（按 Chunk ID、范围、Generation、内容 hash 完整比较），减少新 Scene revision 引起的重复 CPU 描述符分配；Quad 局部上传范围规划以已排序的 GPU byte offset 做线性匹配，不再每次构造临时 FxHashSet；乱序旧布局保守全量上传。**这只消除描述符复制/临时哈希表，不代表 Scene/FrameUpload 已删除。**
- [x] **单测源码（未运行）**：添加复用 Arc、内容变更后拒绝复用与乱序回退验证；既有兄弟 Chunk 局部上传覆盖保持。
- [ ] **待验证**：`cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib chunk_upload`、GPU Buffer 多帧槽/多窗口内容等价；本轮环境无 Cargo/Rust。
- [ ] 明确目前 Frame → SceneSlot → PresentationPacket → pack_scene → FrameUpload → DrawSteps 的职责重复点及每帧字节数，并按上方“强制删除/替换清单”逐项确认**删除落点和唯一所有者**。
- [ ] 设计**唯一一份权威的持久绘制记录**：稳定 Chunk/绘制顺序/属性依赖，避免复制整 Scene 或再建一棵冗余 Render Tree。
- [ ] 用增量变更通知 GPU Owner，旧静态 Chunk 不重复编码/上传；正确处理旧变更丢弃、资源创建/释放顺序和 damage 累积。
- [ ] 删除被替代的 Scene Replay/Scene Finish/Pack 主链路及其旧缓存规则，而不是只改名。
- [ ] Element 命中、IME/Focus、CachedView、动画、Blur/Clip 在新旧视觉代次之间保持正确性。

### P2 Nova Compositor / Resource Lifetime

- [ ] 长期驻留的可复用 GPU 编码、DrawStep、Buffer Range 与 Texture 资源；预算和碎片回收。
- [ ] GPU 原生 Pass 时间戳和关键上传/资源创建/销毁/Fence 数据。
- [ ] 独立合成属性动画、按需 Present，静态窗口不轮询；留意 240Hz 与后台限帧。
- [ ] 四后端统一的 Barrier/Load/Clear/Scissor/Texture Alias/Resize 首帧正确性测试。

### P3 删除旧数据结构并验证

- [ ] 清理旧 SceneSlot / Frame.scene / PresentationPacket.scene / pack_scene / Scene::replay / Scene::finish / 全量 FrameUpload / 旧统计（**不保留双路径**）；拆散名称误导的 render_pipeline，**不删除其真实 Shader/SVG/BackendOptions 能力**。
- [ ] 独立 GPUI 性能实验：Dropdown 高频展开、Blur 切换、静态页面、Text/Image、动画、窗口 Resize/多窗口。
- [ ] BMCBL 实机回归，尤其页面切换、弹窗、地图、透明/模糊与输入法。
- [ ] 对比 CPU 帧长尾、GPU Present 频率、显存、上传量、Filter Target Recreate、资源回收；不可编造提升百分比。

## 2026-10-14 阶段性边界与验证状态（历史快照，不代表最新 master）

- **代码层面（累计）**：稳定 Filter Target 复用、Win32 WM_SIZE 同步构帧入口删除、Retained Quad 布局共享/局部上传、按排序 Chunk 区间编码和 Scene Chunk 完整性前置检查。均是已有单链路的局部纵切，尚未删除 Scene/FrameUpload，也不能宣称统一合成器已完成。
- **本地编译/测试**：当前运行环境未提供 Rust/Cargo、Windows GPU，也不能通过容器连接 GitHub 克隆仓库；已做源码静态审查与 2695 例 JavaScript 区间算法等价检查。Rust 单元测试已加入源码，**未运行**；没有 Cargo fmt/check/test 或 Windows 四后端实机结果。
- **不得宣称**：Windows 黑边已修复、四后端 GPU 占用已改善、Scene 已删除、Prepaint 长尾已解决。

## 2026-10-14 继续重构：原生 Resize 的同步构帧去重

- **变更文件**：`crates/gpui/src/platform/windows/window.rs` 与本文。基线 master `0e45ca39d4cb79dbb184e09a4c1becdc7058272f`；本次提交以 GitHub commit 记录为准。
- **具体删除**：旧 `dispatch_native_resize_frame`（包括一次 `request_frame` 后紧接同步 `dispatch_pending_update_with_vsync(None)` 的二次入口），去掉 WM_SIZE/WM_WINDOWPOSCHANGED 同步调用与 EXITSIZEMOVE 同步调用。原有 modal VSync、foreground-task 和 watchdog 仍负责实际投递更新。
- **保留约束**：`pending_resize` 最新值、尺寸 epoch、Scene/Surface 匹配、in-flight GPU 资源与 UI 输入状态均不被重写；`queue_resize` 自身已发出 UI request，WM_SIZE 不应再重复强制消费。
- **风险与验证**：当前容器没有 Rust/Cargo 和 Windows 显卡设备，不能运行格式检查、编译或 GPU 原生验证；必须本地复现 WM_SIZE 消息嵌套、DWM 暂停时的 watchdog、退出 modal 的最后一帧和四后端 pixel coverage。未解决 Root Color 与 Blur 资源耦合、SceneSlot/Replay/FrameUpload 冗余以及 Prepaint 86ms 长尾。
- **后续优先级**：以 Windows 原生日志确认新的 Resize 消费源和 p95，再按 P1 逐段替换 Scene → FrameUpload 的权威数据重复，禁止新增 RenderTree/RenderCommit。

## 2026-10-14 继续重构：Quad 局部上传索引去重

- **变更文件**：`crates/gpui/src/platform/nova/renderer/chunk_upload.rs`、`renderer/retained_upload.rs` 与本文，接续 Windows Resize 解耦提交 `74088d480c062a8207eeb5ea374f7fa0d72c1619`。
- **唯一所有者不变**：GPU Owner 的 FrameSlot 仍保存每个 slot 的驻留布局身份，当前 Quad Span 作为不可变 Arc 与上传槽共享；不建立第二份权威 Paint 内容，也不变更资源 Fence。
- **可观测事实**：相同 Span 描述符无需重复分配新 Arc；局部上传规划不再为 previous spans 单独分配 HashSet。尚未取得 CPU p95/Allocations/frame 或 GPU 上传量实测。
- **尚未完成**：消除 Scene/Replay/pack_scene、Root Color 与 Blur 分离、四后端正确性及本地编译。上述条目仍是 P0/P1/P3 主线，不得作为完成勾选。

## 2026-10-15：Retained Chunk 编码遍历与隔离校验纵切

- 源码审查基线：`331ae0a4b41873f03514958aebc1f9d19e1bd1c0`；提交期间 master 持续接收其他 UI 提交；每次重新读取 HEAD 与本次目标文件 Blob/Tree，确认无 GPUI 冲突后采用 expected_sha 的非强制快进提交，不覆盖并行变更。
- 生产代码：`crates/gpui/src/platform/nova/frame_upload/encode/retained.rs` 与 `crates/gpui/src/scene/display_list.rs`，另同步本文。
- **计算复杂度变化**：当同一 Scene 有 B 个 Quad Batch、C 个保留 Chunk 时，原编码遍历按 Batch 扫 C 个槽位；新路径在 Scene 已排序、互不重叠的不变量下每 Batch 二分相交区间（`O(log C + overlap)`），没有新增第二份索引。不能用估算复杂度代替实机加速结论。
- **保守正确性**：`Scene::prepare_retained_quad_chunks` 的 `paint_operations` 外部顺序扫描仍保留，因为嵌套 Blur 捕获会有未展平到当前 Scene primitive vector 的子绘制操作；优化仅提前进行不影响最终判定的 Quad 完整性检查。
- **尚未验证**：无 Rust 工具链／Windows GPU，本次只是静态补丁和测试源码；下一次重点核查 Compiler/Rustfmt、Scene Blur、Retained Chunk 复用、Dropdown CPU Encode 与四后端 Present。
- **尚未清除技术债**：Scene/SceneSlot/Replay、GPU root zero-radius filter 与 FrameUpload 的全场景 CPU staging 均存在；本条不得算 P1 完成。

## 2026-10-15 续改：Prepaint 事务性重放与元数据合并

- 基线 HEAD：`446defcff320895034e7d500dc32666d29b8c7f5`。代码文件 `crates/gpui/src/window/draw_reuse.rs`，唯一进度文档为本文。
- 验收语义：`DispatchTree::reuse_fragment` 首先验证全片段父路径映射，失败直接返回 None；`Window::reuse_prepaint_fragment` 仅在其成功后搬运辅助 Prepaint 状态，避免旧 Tooltip 提前被 take() 造成 UI 输入状态损失。
- 内存路径：片段元数据重放从两份 SmallVec 标记/前缀表减少为一份 prefix；仅改变选择/偏移的表示，原先关于完整子树、paint/prepaint/child Scene 代次和保守回退的判断保持不变。
- 测试分层：静态审查及代码内用例并不等于通过 cargo、Windows DX11/DX12/Vulkan/GL 原生验证；Prepaint p95、GPU 使用率、Scene/FrameUpload 清理和 Filter Root Color 解耦仍未完成。
- 下一阶段：核对 CachedView fragment 复用的边界与退化日志，并逐步迁移唯一 Paint Chunk 所有权，避免新增 RenderTree、RenderCommit 和重复 staging。

## 2026-10-15：Scene Finish Retained 候选隔离优化

- 基线：`3f21b67fc72e3027ef1b62c6bea099518b2cb83d`；修改 `crates/gpui/src/scene/display_list.rs` 与本文，上一轮 Prepaint 事务性重放提交 `3f21b67f` 已在 master。
- 确认保留全 `PaintOperation`（包括嵌套 Blur）Order 语义，不能只索引扁平 `Scene.quads`；在大候选场景才惰性构建临时 DrawOrder 数组，排序后以二分计数，避免 C 次完整扫描。该数组不是持久绘制状态或第二棵 RenderTree。
- 正确性验证：新增排序计数与原范围外检查的纯逻辑对照单测；真实窗口 Blur/Dropdown、运行时间、显存和四后端原生测试仍未执行。
- 重要未完成项：Prepaint 真正耗时贡献、Paint Chunk 单一所有权、Scene/FrameUpload 删除、Root Color 与 Filter 拆分、DX11/DX12/Vulkan/GL 回归均未完成。

## 2026-10-15：Nova GPU Retained Quad 上传安全边界

- 代码基线：`898eeae62ac6810ec817d86523b54c664dc57bb2`。文件 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs`，同步唯一进度文档。
- 改进：避免从 Vec 再拷贝到 Arc 的中间暂存所有权，局部上传前验证当前与旧布局均为合法有序片段；不新增任何 RenderTree/RenderCommit、GPU Resource Owner 或兼容层。
- 风险隔离：过时的大尺寸 old FrameSlot 仍可能在尺寸变化后保存有效布局，因此不按新 byte_len 校验旧布局尾部；current 必须满足新的 GPU Buffer 上界，损坏时回退全量上传。
- 尚未验证：无 Cargo/rustc/四后端显卡，已添加单测但尚未执行；未知收益不得写为已实测。
- 下一步：继续减少 FrameUpload 对完整 Scene 的打包/签名扫描，验证 CachedView 局部失效对应实际 GPU bytes，并进一步拆分 root color / Filter DAG 资源所有权。

## 2026-10-15：直接 Root Color GPU 记录，移除空 Scene

- 基线：`5d349bc50661389cd053632433ae42e1495e2ce3`；代码文件 `crates/gpui/src/platform/nova/upload_packing.rs` 与 `frame_upload/encode.rs`，另同步唯一 TODO。
- 生产变更：`write_paint_blur` 保留真实 Element Blur 的原有调用契约，委托共享 `write_paint_blur_fields` 写同样的 136-byte packed ABI；`append_retained_root` 直接传递零半径、尺寸和 mask，删除 `Arc::new(Scene::default())`。新字段编码不触及 Filter DAG、GPU Barrier、资源退休或 Shader。
- **必须继续删除**：`auto_capture_static_background` 仍然会为静态前缀和动态前景分别 `Scene::replay` / `finish`；处理它必须先证明自动捕获的 Prepared Batch 边界、Drawable extent、Blur 配置及跨帧 source-content version 不变量，不能随意丢失 GPU retained source。
- 本轮 GitHub 编辑没有 cargo/rustc/Windows GPU 测试环境，性能和四后端等价未确认。

## 2026-10-15：自动背景捕获原地编码与真实资源范围

- 本次连续改动承接 `c68363a37db169814f2798033adb7b5a628c1088`（Root Color 空 Scene 删除），基于其后 HEAD 追加 `renderer/retained_upload.rs`、`frame_upload/encode.rs` 与本文的纵向修改。
- 新链路：`原始 Scene.prepare_batches → FrameUpload::encode(原 Scene) → capture_encoded_static_prefix(在原有 GPU 实例批次内切开) → Filter Source/DrawStep/Present`。不再创建中间 Foreground Scene；不再对动态前景再做 `replay/finish`。
- 旧前景 Scene 会变更上传 Primitive instance 排序和 Painter-order；因此严格要求录制次序的 DrawOrder 分界：前缀操作 Order 小于动态第一个 Order，后缀不小于；编码后以**裁剪后的有效实例计数**确认已分组 Batch，且仅对 Quad/Shadow 合法批次切开。任一条件不成立原地回退，而非部分提交 Batch。保留单一 Source Scene 用于 `FilterRegistry::source_unchanged` 的原内容版本证明，不能简化成只看 revision。
- GPU Target：不改变 Shader 136 字节 blur ABI、原有元素模糊目标分配及 Fence 退休；已复用 `FrameUpload::refresh_backdrop_blur_configs/rebuild_backdrop_blur_passes`。DX11/DX12/Vulkan/GL Native Buffer、重合阴影、Clip、零面积、Live Resize 均**没有**实测。
- 下一步：Source ContentVersion 脱离背景 Scene，并对 auto source Texture Residency 接入持久 Paint Chunk 原有逻辑，最终从代码删除所有自动背景 Scene Replay；在此之前 P1/P3 保持未完成。

## 2026-10-15：自动静态背景内容身份从 Scene 移至 GPU 编码/Filter Owner

- 起点 HEAD `80e17ecdfe52f672a5c7d7e24f39285a397ce07b`，生产代码：`crates/gpui/src/platform/nova/frame_upload/frame.rs`、`frame_upload/encode.rs`、`renderer/retained_upload.rs`、`renderer/filters.rs`、`renderer/draw_steps.rs`；修正 `renderer/present.rs` 单测中旧 Source 类型。
- **本次真正删除**：`auto_capture_static_background` 的 `Scene::default` / `Scene::replay(0..prefix_end)` / `Scene::finish` / `Arc::new(background)`；自动背景不会再复制任何子 Scene。Encoder 的源指纹按实际渲染的有效 Quad/Shadow Batch 次序读取打包字节，不能仅凭 Scene revision/ElementID 跳过内容变更。
- **无新的所有权层**：唯一 `FrameUpload` 仍持有本帧临时批次，唯一 `FilterRegistry` 仍持有在成功 GPU 提交后才推进的内容版本；`FilterSource` 替换原有 `Vec<(u32,PaintBlur)>` 语义类型而非建立并存缓存；AutoStatic 不持有 Scene。真正的 Element Blur 保持旧的深度匹配与动画区域计算。
- **正确性边界**：AutoStatic 仅接受原来已证明的 96+ 个纯 Quad/Shadow 静态前缀；Layer 必须平衡，不能有 Retained Chunk/Atlas/Shader Effects/混合跨顺序。已编码 Batch 不合法仍原子回退；指纹差异或未验证目标强制重绘完整源，不依赖窗口 DirtyRect 的覆盖范围。Blur Target 创建/更新/提交时序不变。
- **下一步起点**：验证新 Source Type 的所有消费者、单元测试和 GPU 原生渲染；补齐单一 Paint Chunk 的真实版本/所有权并继续消除 SceneSlot/Scene Replay 和双重 CPU FrameUpload，在验收前 P1/P3 未完成。

## 2026-10-15：FilterRegistry 退役源快照与有序索引

- 起点 HEAD：`90ea49cc54e3fd4cd40d5664b4a3814d57ad08b5`；修改 `crates/gpui/src/platform/nova/renderer/filters.rs`，同步本唯一进度文档。
- **代码实现，未编译验证**：成功 Present 后，仅移除本帧已不存在的 FilterSource 驻留快照；仍在帧内但未执行离屏刷新者保持原 TextureContentVersion，不会凭普通 Present 伪造源更新。消失的 Element FilterSource 不再长期强持有旧子 `Arc<Scene>`。
- 保留唯一 GPU FilterRegistry 所有者，源列表采用已存在的 `Vec` 按 index 有序插入，查找和活动源裁剪使用二分；不创建 RenderTree/RenderCommit/第二份缓存。依赖 `FrameUpload::encode_scene` 按 packed backdrop index 单调分配 source 的事实，加入 debug 断言及测试覆盖反序刷新、未刷新版本延续、旧 Scene Arc 释放。
- **尚未验证**：无本地 Rust/Cargo 及四后端 GPU，本改动未执行 rustfmt/check/test、Windows Render Graph/Fence 回归；不能宣称显存或 GPU 占用已下降。缓存复用仅代表 CPU 侧元数据/Arc 生命周期，不代表实际 GPU Target 已退休；目标资源仍由原有 Fence/TargetPool 路径管理。
- **下一步**：验证 FilterSource 迁移后的原生四后端编译与动态页面过滤器数量回落，再推进 CachedView 持久 Paint Chunk 唯一所有权、SceneSlot/Replay/pack_scene 的有依赖删除；仍须验证 Dropdown、Blur、DX11 Resize、DX12 子窗口、Vulkan/OpenGL。

## 2026-10-15：Element Filter 动画快照同代共享

- 接续 master `c6398104e513932b91cec36590d919fd9e9e34b3`。在 `crates/gpui/src/platform/nova/renderer/filters.rs` 中将各 Element Filter 离屏刷新时的动画快照，从逐滤镜独立 `Vec<SceneAnimationValue>` 复制改为同一次成功 Present 的共享不可变 `Arc<[SceneAnimationValue]>`；无动画与 AutoStatic 不分配此快照。不同刷新代次独立持有，未重绘的 Element 继续保存原始采样值，不受其他目标的刷新影响。
- 正确性：只优化 CPU 样本所有权和复制次数，保持 TextureContentVersion、Source Damage、GPU Shader 采样协议、Filter Target/Fence 和真实动画轨迹语义。新增同代 `Arc::ptr_eq`、跳过目标后的旧采样保留、清除失效版本等源码单测。
- **未通过验证**：容器仍无 Rust/Cargo 与四后端原生 GPU；没有执行 cargo fmt/check/test 或实机测试。性能效果只可称潜在减少分配，不声称现有瓶颈已经实测解决。下一步需要继续将 FrameUpload 与 retained Paint Chunk 归为单一所有权，按既定删除清单清理 SceneSlot/Replay。

## 2026-10-15：Retained Quad 持久编码缓存迁入 GPU Owner，统一嵌套 Chunk 存活范围

- 起始 master：`363195999e129e64fcd34176e6b5e794b725748c`。本次修改 `crates/gpui/src/platform/nova/frame_upload/{frame.rs,encode.rs,encode/retained.rs,capacity.rs,quad_stream.rs}`、`platform/nova/renderer/{retained_upload.rs,memory.rs}`、`platform/nova/renderer.rs`、`platform/nova/renderer/shared_buffers/native_tests/chunks.rs`、`platform/nova.rs`、`platform/nova/tests.rs`；本文是唯一进度文档。
- **真实所有权迁移（代码实现，未编译）**：`RetainedQuadChunkCache { retained_quad_chunks, active_retained_chunk_ids_scratch }` 是 GPU Owner 下 `RetainedUpload.quad_chunks` 的独占持久成员；这两个跨帧容器从 `FrameUpload` 删除。本帧编码器只借用 `&mut RetainedQuadChunkCache`，`FrameUpload.quads` 只持有相同不可变 Arc 源的分段引用、增量 dirty gaps 与本帧 `resident_quad_spans`。不引入 RenderTree、RenderCommit、另一份 packed byte 缓存或 Scene 兼容层。
- **嵌套失效修复**：原 `FrameUpload::prune_retained_quads(scene)` 只采集根 Scene Chunk，递归 Element Blur Scene 编出的保留 Chunk 即使已进缓存也会被误删。现在编码入口先 `cache.begin_frame()`，每个实际准入的 Quad Chunk 在递归编码中标记存活，根编码结束时对唯一 GPU 缓存进行一次 `prune_retained_quads()`。完成纹理提交前不修改 GPU Fence/Target/FrameSlot；已借入当前帧的 Arc 在缓存 Trim 后仍有独立生命周期。
- **内存/压力路径同步**：`retained_quad_memory` 与 `retained_byte_capacity` 接受 GPU 缓存所有者并按 Arc 地址去重，`NovaRenderer::trim_retained_capacity` 对临时 FrameUpload 和持久 Quad 缓存分别执行裁剪；两套 bench 明确持有自身测试用缓存。内存统计仍不能替代驱动 VRAM 实测。
- **测试源码（未运行）**：更新全部已定位的 `FrameUpload::encode`、内存统计、bench、Native GPU 测试消费者；新增释放/重新构造 FrameUpload 后仍能命中驻留 payload，以及嵌套 Element Blur 下的 Retained Chunk 不被根帧错误裁剪的对照测试。需要 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_quad`，Windows DX11/DX12/Vulkan/OpenGL 及 Live Resize/Fence 实机验证，当前环境未运行。
- **不能标记已完成**：SceneSlot、`Scene::replay`、`Scene::finish`、`NovaRenderer::pack_scene`、全场景 FrameUpload staging、完整持久 Paint Chunk、GPU property compositor lane 仍存在。下一次必须先编译/验证此所有权迁移，随后推进直接以 CachedView Chunk 变化集驱动 GPU resident encode/DrawStep，移除更多 Scene Replay 与全帧打包链路，不添加新包装中转。

## 2026-10-15：修复迁移后测试并用 Chunk Epoch 清理存活集合

- 基线 HEAD：`005d3f7551f016aaa3eb8273e9e4d8879be87eea`。源码追踪确认上轮 `crates/gpui/src/platform/nova/tests.rs` 的独立 FrameUpload 生命周期测试漏声明 `chunk_cache`，另有两处旧 `upload.retained_quad_chunks` 访问；本次一并修复。前一版**尚未通过 Rust 测试**，不能将其描述为已编译。
- 以 GPU Owner 的 `RetainedQuadChunkCache.frame_epoch` 和各 `PackedRetainedQuadChunk.last_used_epoch` 替代每次完整编码复制所有 `RetainedChunkId/GlobalElementId` 的临时活跃 HashSet。只有实际完成打包/复用的 Chunk 写入本轮 epoch；包括递归 Element Blur 的 CachedView；根层完成后对唯一缓存按 epoch 原地 retain。u64 epoch 溢出时清空旧缓存，避免错误命中。此变更不产生第二份持久 Paint 状态，不能推导出 GPU VRAM 降低。
- 修改范围：`frame_upload/frame.rs`、`frame_upload/encode/retained.rs`、`frame_upload/capacity.rs`、`platform/nova/tests.rs`、本 TODO。同步删除 HashSet scratch 的统计、Trim、测试假设，增加过期 Chunk 释放时仍由外部 `Arc` 持有旧字节的测试。
- 当前仅静态检查，**未运行 cargo fmt/check/test，也无四后端实机验证**。下一步必须优先本地编译上轮及本轮 GPU Owner 迁移，然后推进真正 CachedView → Paint Chunk 唯一记录，清除 SceneSlot/Replay/pack_scene；不要仅增加旁路缓存。

## 2026-10-15：大型 Scene 的 CachedView 结构隔离校验去重复前缀扫描

- 起点 `1e7ddc3261688a6a3a704556d1645fe85f0fa13c`，代码变更 `crates/gpui/src/scene/display_list.rs`；保持本文件为唯一进度记录。
- **代码实现（未编译）**：`Scene::range_is_independently_replayable` 原本每次验证都从全场景 `paint_operations[0..range.start]` 逐项计算 Blur/Layer 深度，再扫描目标范围；多个 CachedView、Selective Splice、普通 Retained Element 重复判断同一已完成 Scene 时，可能产生重复 O(N) 前缀扫描。对已完成且 PaintOperation >=256 的 Scene，第一次查询惰性建立仅含 Blur/Layer Begin/End 的稀疏结构边界索引，后续以 `partition_point` 求边界前缀状态，复杂度按结构屏障数为 O(log B) 每次查询。静态 Quad/文本操作不在索引中，不构建新的绘制树或双份权威内容；小场景及正在构建/回放的 Scene 继续按原代码扫描。
- **保守正确性**：空区间、越界、区间起点处于未闭合 Blur/Layer、前缀出现不匹配关闭、区间内部不匹配关闭，均保留原语义。索引由唯一 `paint_operations` 派生，`Scene::clear`、`finish_with_previous` 取消旧计算结果，未完成 Scene `revision=0` 不读取旧索引。新增正常嵌套与错误结构下逐区间对照旧扫描单测。稀疏索引仅属于过渡期 Scene 的加速元数据，长期目标仍是单一持久 Paint Chunk/依赖索引；不将 Scene 标记完成。
- **验证**：已静态检查算法和调用关系；无 Rust/Cargo、Windows 原生 GPU，未执行 rustfmt/check/test，不声明 Prepaint p95 改善。须重点测多个嵌套 CachedView 的命中率/Prepaint p50/p95/p99、内存增量和普通小页面冷启动。后续仍须移除 SceneSlot/Replay 与重复 FrameUpload staging，并测试 Blur、Dropdown、DX11/DX12/Vulkan/GL。

- **2026-10-15 新增测试静态类型修正**：上条结构索引提交 `c4c9346f` 的测试错误假设 `PaintOperation: Clone`；实际 `PaintOperation` 未实现 Clone，已改为按需构造普通 Quad 操作的闭包，不修改生产逻辑。仍未执行 Rust 编译/单测；本修正不等于验证通过。

## 2026-10-15：FrameUpload 编码接口迁移后的测试作用域回归修复

- 起点 HEAD `953db6be081650e38aaaded60e44d4511b7f6d88`。对上轮迁移后的全部 `platform/nova/tests.rs` 和 `platform/nova/frame_upload/encode.rs` 单测函数做静态变量作用域审查，确认另外 6 处 `&mut chunk_cache` 调用缺少对应声明（分别 4 处、2 处），现已在各自测试作用域创建 `RetainedQuadChunkCache::default()`，不修改生产函数、GPU ABI 或资源生命周期。
- 静态逐测试函数检索中已无未声明 `chunk_cache`；此结论不等同 Rust 编译/测试通过。仍须本地运行 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib retained_quad` 及普通 GPUI 单测，禁止用 CI 替代。

## 2026-10-15：补齐 FrameUpload 子模块剩余调用点

- 起点 HEAD `118d1cadd0a68f163d2f2177f5514be07d7b9ef1`。通过 GitHub 代码搜索审计 `upload.encode(`，发现上轮 GPU Owner 缓存迁移尚遗漏 `platform/nova/renderer/retained_upload/sharing.rs`、`platform/nova/tests/sprite_gpu.rs`、`platform/nova/tests/sprite_gpu/relocation.rs` 三处测试/硬件验证子模块，仍按旧版 `FrameUpload::encode(scene,...)` 调用。现均改为独立 `RetainedQuadChunkCache` 可变借用并保持原场景数据和 GPU 资源流程不变。
- 这属于**编译接口完整性修复**，不是新增功能，尚无 Rust 编译或四后端实机验证。迁移是否真正通过以本地 `cargo check` 与 `cargo test` 为准，不能以静态审查替代。

## 2026-10-15：CachedView Scene Replay 候选范围二分，移除每片段全量扫描

- 基线 master `faa7f4725cb7ef5c0c895757874d7fc2181542ef`；修改 `crates/gpui/src/scene/display_list.rs` 与本唯一进度文档。保留原始 `Scene::paint_operations` 和 `retained_chunk_candidates`，**不创建第二份 RenderTree/Chunk ID 索引**。
- **代码实现，未 Rust 编译**：`Scene::finish_with_previous` 原地按候选 `scene_range.start` 排序（相同起点优先较大的结束位置），令完成帧候选区间可二分。`Scene::replay` 对已完成上一帧通过两个 `partition_point` 收窄起点落在重放片段内的候选，再保留既有 `scene_range.end <= range.end` 的完整包含证明。这样多个独立兄弟 CachedView 局部重放不再为每个片段线性遍历全部候选；复杂度由每片段 O(C) 降至 O(log C + K)，K 为该片段起点内候选（包括与片段终点重叠但需要过滤的候选）。多层嵌套父范围、断开的 Replay 片段不被错误继承。
- **正确性边界**：构建中的 Scene（`revision == 0`）候选可能仍是 append 顺序，继续使用原全扫描；`record_retained_chunk` 写入新的候选时将 `revision` 标回 0，不允许在再次 `finish` 之前消费过时的有序证明。候选排序在 `prepare_retained_quad_chunks` 前完成；Prepared Chunk 的保守 painter-order/独占验证、GPU Arc/Fence、离屏滤镜不变。
- **测试源码**：16 个兄弟 Cache Chunk、跨越两次 Replay 的父 Chunk、排序与跨片段保留、未完成 Scene 回退以及后置候选更新后的 revision 失效；另用独立随机区间脚本对照原始逐项扫描，36,000 组结果一致。**没有执行 Rust 编译、rustfmt、单测及 DX11/DX12/Vulkan/OpenGL 实机测试**，不宣称 Prepaint 或 GPU 占用已实测下降。
- **后续删除清单**：`SceneSlot`、`Scene::replay`、`pack_scene`、全量 `FrameUpload` staging、全类型持久 Paint Chunk 尚未删除。本改动优化现存 Replay 热路径，而非以额外长期层阻碍后续破坏性迁移。下轮先运行 Cargo 验证，并从 CachedView 的纯 Quad/Shadow 可持久段扩展向单一 GPU resident 变更集。

## 2026-10-15：移除仅局部平衡的弱 Blur Replay 校验

- 基线 master `f252280cdb26b9f426f6a29765620ae7dcf6db8e`；删除 `crates/gpui/src/scene/display_list.rs` 的 `range_has_balanced_element_blurs`，同步清理 `crates/gpui/src/scene/tests.rs` 中仅测试该弱接口的断言。源码搜索确认无生产消费者。
- 该旧方法只检查候选片段内部 `StartBlur/EndBlur` 配对，不能识别片段起点位于外层 `StartBlur` 或 `StartLayer` 内的情况，返回 true 不代表可单独 Replay。生产链路原已使用更严格的 `range_is_independently_replayable`；现在消除可能被未来调用者误用的重复入口，保留嵌套捕获的拒绝回归测试。无新兼容层。
- **状态**：代码实现及静态依赖检查，未 Rust 编译/测试、未四后端原生验证；不以删除弱接口声称完整 Scene Replay 已迁移。

## 2026-10-15：修复 CachedView Debug 重放退化，验证 GPU Cache 实际长度

- 起点 master `72c97104f8f8a5a8f164be82487583521b9cad96`。修改 `crates/gpui/src/scene/display_list.rs`、`platform/nova/frame_upload/encode/retained.rs`、`platform/nova/tests.rs` 与本文。
- **已代码实现、尚未编译**：上轮 Replay 候选二分逻辑在 Debug 模式下仍在每一次重放时执行 `debug_assert!(candidates.windows(2).all(...))`，导致每个 CachedView 片段隐含 O(C) 全表扫描，抵消所声称的热路径优化。将这个排序不变量检查移至 `Scene::finish_with_previous` 完成排序后，只有每帧一次 Debug 校验；生产逻辑继续使用上一帧的有序候选二分，没有新增结构、缓存或场景快照。
- **GPU resident 防错边界**：同一 `RetainedChunkId` 语义身份不代表可忽略 Packed Quad 数量。缓存复用时额外校验 `quad_count == 本次 chunk.quad_range.len()` 且 `bytes.len() == quad_count * PACKED_QUAD_BYTES`；不一致回退当前帧重新编码，避免旧片段用错误实例数覆盖新布局。新增在保留相同 ID/generation 但真实 Quad 数从 32 增至 64 的异常回放对照测试，要求完全重编码并更新唯一缓存；不会伪造 chunk hit。
- **验证与后续**：尚无本机 Cargo/rustc、Windows 原生 GPU，未运行 `cargo fmt --all`、GPUI 单测及四后端验证；Debug O(C) 消除为静态代码事实，真实 Prepaint p95/GPU 占用未测。下一步继续检查 Scene Replay → Packed Chunk 身份、FrameUpload 全量实例流与独立 GPU Compositor Lane，待编译验证通过后逐步移除 SceneSlot/Replay，而不是继续增设兼容层。

## 2026-10-15：Quad 可见性边界按实际 Packed Batch 统一计算

- 起点 HEAD `e9fcc8aa4ebfd0fb074a6093526245681865a0fe`；修改 `crates/gpui/src/platform/nova/frame_upload/encode.rs` 和唯一 TODO。目标是减少 Scene→FrameUpload 重复 CPU 遍历，不引入新 Scene/RenderTree/额外 FrameUpload。
- **实现**：原 `encode_scene` 对每一个 Quad Prepared Run 先调用 `static_quad_run_visual_bounds` 遍历原始 Quad，再执行 packed encoding；仅当任一动画令预判失效时，才再次调用 `static_packed_quad_batch_bounds` 按真实上传 Batch 重扫。改为所有 Quad Run 编码完成后统一按已打包的 `UploadedBatch::{Quads,SolidQuads}` 及有效可见 Quad（跳过零面积 clip）计算一次每批可见边界。源 Run 中有 GPU-indexed 动画的 Batch 仍返回无边界并禁止错误裁剪，未含动画的兄弟 Batch 保留独立裁剪，嵌套 Element Blur 子 Batch 的 metadata 保持原有增量处理；Sprite/Underline 仍用各自先验计算，不调整 Shader ABI。
- **价值和风险**：静态场景不再对每个 Packed Chunk 套用整段 Run 的宽泛联合边界，提高按批次进行 Scissor/DirtyRegion 裁剪的机会；混合动画场景不再进行一次必然失败的整段 Quad bounds 前置遍历。仍会在编码后遍历有效 Quad 一次生成原有 metadata，不能宣称避免整个 FrameUpload、GPU copy 或已测量 GPU 占用下降。
- **测试源码（未运行）**：添加 32 Quad 保留块 + 1 Quad 空隙 + 32 Quad 保留块的真实 encode 对照，验证 3 个 Packed Batch 的独立非重叠边界；现有静态、动画、退化 clip、安全回退测试保持。当前环境无 `cargo`/`rustc`/`rustfmt` 和 Windows GPU，尚未编译、测试、截图或验证 DX11/DX12/Vulkan/OpenGL，仍需以本地 `cargo fmt`、`cargo check`、`cargo test` 和真实 Profiler 验收。
- **未删除**：`SceneSlot`、`Scene::replay`、`NovaRenderer::pack_scene`、全场景 FrameUpload staging；下一步分析 Quad batch bounds 是否应直接由 Scene 已验证的 retained Paint Chunk 维护，以便进一步删除同一场景遍历，而非新增身份缓存。

## 2026-10-15：Monochrome 字形边界裁剪收敛为单次遍历

- 起点 master `b7cd8eaa40920b32d4af974065769e7d817b0160`；修改 `crates/gpui/src/platform/nova/frame_upload/encode.rs` 与本 TODO。完成上轮 Quad Packed Batch 边界统一后的第二项直接热路径收敛，未创建新 RenderTree、状态缓存或 Upload Stage。
- **代码实现，尚未编译**：原 MonochromeSprite 预先 `sprites.iter().all` 检查是否有非单位 TransformationMatrix，再 `sprites.iter().map` 逐字形聚合裁剪边界；普通文本页面每一个无变换 glyph batch 都会重复扫描一遍。现在把 `has_unbounded_transform` 作为已有 `static_raster_run_visual_bounds` 的第四个输入，与动画安全检查合并，同一遍历完成矩阵证明和可见 bounds union。Quad、Polychrome、Underline 使用 `false`，保留原判定；对真实变换过的 Mono 字形继续返回 None，拒绝不安全的包围盒裁剪。
- **验证**：新增通用静态 sprite bounds 的非单位变换拒绝用例；现有动画 rejection、Quad Packed Batch 测试继续覆盖。静态检查全部 `static_raster_run_visual_bounds` 调用点并核对调用参数数量，未运行 `cargo fmt/check/test`、未在 Windows DX11/DX12/Vulkan/OpenGL 测试、未量化高频 Dropdown 的 CPU/GPU 效果。下一步应测试文本密集页面的 Encode p95，并检查预期的保守裁剪与实际 Render Graph Scissor 是否一致。
- **未完成**：彻底取消 Scene Replay、SceneSlot、FrameUpload 全量 staging、持久 Paint Chunk 类型统一和 Presentation Lane，不能因为局部减少一次字形遍历便宣布阶段结束。

## 2026-10-15：Quad 可见边界与 Packed Chunk 编码合并（移除生产二次扫描）

- 起点 master `bb2f705aa6d9d6519f32f2b2ea97d246ab9b3a9c`。纵向改动 `crates/gpui/src/platform/nova/frame_upload/encode.rs`、`frame_upload/encode/retained.rs`、`frame_upload/frame.rs`、`platform/nova/tests.rs` 及本文件；没有添加 Scene、RenderTree、RenderCommit、额外 FrameUpload 或第二个 Quad Payload Owner。
- **生产实现（未编译）**：原 `encode_scene` 即使 Retained Chunk packed bytes 直接从 GPU Owner 的 `Arc<Vec<u8>>` 复用，也调用 `static_packed_quad_batch_bounds` 重新遍历此帧全部源 Quads，导致复用场景仍付出大段 CPU bounds 扫描。现在 `encode_quad_range` 在写入可见 Quad 的同一循环累计当前 Batch 的保守可见边界；静态/动画分组切换时一并提交对应 `batch_visual_bounds`。GPU Owner 的**同一条** `PackedRetainedQuadChunk` 缓存记录增加 `visual_bounds`，在首次/脏内容打包时顺路计算，与 packed bytes、QuadCount、hash 同步失效；重放命中直接提交此前已证明的边界，不扫描旧 Quad 数据。动画、非有限几何返回 None 禁止错误裁剪；零面积 clip 继续被忽略，Count/ABI 未变。
- **嵌套所有权/批次顺序**：`encode_scene` 不再调用原生产二次扫描（旧算法仅供 `#[cfg(test)]` 测试对照）。在递归 Element Blur 的 `BeginBlur` 与子 Quad 编码之间先将 `batch_visual_bounds` 补齐到当前全局 `batches.len()`；同时覆盖同一 Prepared Batch 连续多个 sibling Blur 的 End/Composite 占位，避免边界与后续 GPU DrawStep 批次错位。其他 Sprite、Underline、Backdrop 和 RenderExtension 继续保留保守判断及原有 Fence/Target 协议。
- **测试源码（未运行）**：保留两个 32-Quad Chunk 与中间 gap 的独立边界用例；新增 12+1 动画+12 Quad 的分组边界测试；Nova 测试核对 cached/replayed Quad 的边界相同、单个与两个 sibling Element Blur 的 Begin/Quad/End/Composite 全局 metadata 下标对齐。新增字段由测试手动构造的 `PackedRetainedQuadChunk` 同步初始化。
- **验证限制**：当前环境无 Cargo/rustc/rustfmt 或 Windows GPU。仅已审计调用点和代码依赖；未运行 `cargo fmt/check/test`、DX11/DX12/Vulkan/GL native draw/resize，也未证明 Prepaint p95/CPU/GPU/VRAM 下降。须先本地编译并优先跑 `retained_quad`、`batch_visual_bounds`、`element_blur` 对照后再进一步破坏性改动。仅多出每条 CPU Chunk 缓存记录一个保守 Bounds 元数据，不修改 GPU Shader ABI。
- **剩余**：SceneSlot、Scene Replay、统一全类型 Paint Chunk、全部 FrameUpload Staging、独立 GPU Compositor Lane 未完成。下一步让 DirtyRegion/DrawStep 使用统一的 Chunk 变更代次并进一步减少不必要的 per-frame DrawStep/Upload 重建，不得引入平行 Scene/RenderTree 中转。

## 2026-10-15：Quad 打包与动画分段决策合并到单次遍历

- 起点 master `d584eb4f525e8e4fbf8445131004fb7724cf5d62`。修改 `crates/gpui/src/platform/nova/frame_upload/encode/retained.rs` 与本唯一 TODO；不创建并行场景、DrawStep、批次缓存或新增 GPU Owner。此前已实现 Retained Chunk Packed Bytes/Bounds 共同所有权，继续减少未保留 Quad Gap 的 CPU 工作。
- **【代码实现，未编译】**：删除编码前的 `should_split_static_dynamic_quads(quads)` 全量预扫描。现在 `encode_quad_range` 打包有效 Quad 时顺带统计动画状态切换、最长静态连续段与保守边界；最多在前 8 次切换处暂时追加批次。编码结束后，仅当动画切换 >0 且 <=8、最长静态段 >=8 时保留拆分；否则将本次局部新增批次和边界**原地**截断为一个连续 GPU Batch，无额外 Vec 或重复源 Quad 扫描。超过阈值即停止继续产生候选 Batch，防止动画高频交替造成大量临时提交。
- **【正确性收敛】**：`append_quad_batch` 只记录 `UploadedBatch` 与索引对齐的 `batch_visual_bounds`，`encode_quad_range` 按实际发出的 Quad 总数更新 `summary.quad_count` **一次**，合并候选后不会二次计数；`encode_retained_quad_chunk` 同样直接按编码实例数更新 summary。缓存命中原路径不变，仍使用唯一 GPU Owner 的 `PackedRetainedQuadChunk`。合并静态/动画混合 Batch 设 Bounds=None，避免危险的局部裁剪；不可见零面积 Quad 不计入切换与 GPU 实例。
- **【测试源码，未运行】**：原有 10+1+10 和高频交替两个 Batch 测试保持，增加短静态段自动合并与计数断言；新增 400 轮包含静态、动画、零面积裁剪的随机 Quad Pattern，对照旧阈值语义、Batch 数量、first/count、bounds 是否可裁剪以及 `summary.quad_count`。本次还使用独立脚本对 100,000 条随机 trace 比对旧/新拆分判定一致性；这不是 Rust 编译或运行测试。
- **【待验证】**：当前环境无 cargo/rustc/rustfmt，不能执行 `cargo fmt --all -- --check`、`cargo check`、`cargo test -p gpui --lib` 与四后端实机测试；应重点关注极高动画频率时批次合并、超出 MAX_QUADS 的截断帧、Canvas 自绘数据，及不匹配 Scene/Batch Bounds 的异常。只有本机 profiler 才能确认 CPU Encode p95 与 GPU DrawCall 效果。
- **【仍未完成】**：SceneSlot、Scene Replay、根 Scene/Filter 拆分、全量 FrameUpload Staging、跨类型 Paint Chunk、独立 GPU Presentation/Compositor Lane。下一纵向切片应减少 Scene→FrameUpload 的其他源数据重复复制并验证 GPU resident 修改代次，而不是再添加新的并行表示层。

## 2026-10-15：Quad Resident 上传脏区间内联规划，删除常规逐帧 Vec 分配

- 基线远程 master `28473c9519d9f1afb6371ebf1ddc08967c77369d`。修改 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs`、`renderer/retained_upload.rs`、`renderer/retained_upload/sharing.rs` 和本唯一 TODO。遵守 GPU Owner 中 `QuadResidentLayout`/跨帧 slot residency 为单一依据，不引入第二套上传状态或额外 Scene/Chunk 所有者。
- **【代码实现，未编译】**：`QuadUploadPlan::Ranges(Vec<Range<usize>>)` 改为 `Ranges(SmallVec<[Range<usize>; 4]>)`。多 Chunk 局部修改中常见的 1～4 段脏字节区间可留在计划本身的内联存储，不再为此单独分配堆 Vec；超过四段仍自动 spill，完整保留顺序、半开区间、字节数和原生 API 接收的散射写入语义。
- 完整上传、空计划、合法性验证、原旧 Slot Range、Chunk ID/generation/hash、资源替换后强制 Full、物理 GPU Buffer 和 `BufferUploadBatch` 都不变。`dirty_complement` 的唯一区间列表仍按全局字节顺序产生，不会与动画局部更新重新排序；不假设 Backend 具备跨 Buffer 的无序执行能力。
- **【测试源码，未运行】**：将所有已知 `QuadUploadPlan::Ranges(vec![...])` 测试改为 `smallvec!`；新增三个脏区间全部内联、七段碎片自动转堆且正确计算 72 字节上传量、完整保持物理顺序的回归测试。注意本变更在最坏大量碎片情况下依然会分配堆内存，并非消除全部 BufferUploadBatch 阶段 CPU 开销。
- **【验证】**：仅对 GitHub 源码全局调用点进行了静态审查，**无 Rust Cargo/ rustc / Windows native GPU**，未运行 fmt/check/test/四后端。真实堆分配下降、DX11/12/Vulkan/GL 上传性能和 p95 延迟仍须本地 Profile 测量；不要以此推断 GPU/VRAM 降低。
- 后续重点：验证 Resident Upload Plan 与 DrawStep/DirtyRegion 的版本一致性，清理冗余 `SceneSlot`、`Scene::replay`、全量 `FrameUpload`、并完成独立 GPU Compositor/Present lane（尚未完成）。

## 2026-10-15：共享 Arc 的 GPU Quad Resident Layout 单遍脏区间验证

- 起点 `34a2091b5de771208bf5ee316265086ad75c0243`；变更 `crates/gpui/src/platform/nova/renderer/chunk_upload.rs` 与唯一 TODO。未引入第二套 GPU Chunk/Scene/FrameUpload 所有者。
- **【代码实现；未 Rust 编译】**：`QuadResidentLayout::from_upload_reusing` 已在完全相同的 Chunk ID/generation/hash/位置序列时直接共享 `Arc<[RetainedResidentSpan]>`；原 `upload_plan` 对这类相同对象仍各遍历旧布局、新布局并执行逐项归并。新增 `Arc::ptr_eq` 精确快路：利用现有 `dirty_complement` 在**单次遍历中同时验证范围顺序、是否越界、是否为空**以及生成脏字节间隙，不再扫两份等价元数据。Arc 指针不相同则保留原严格比较逻辑，不使用指针相等作为跳过范围合法性校验的理由。
- **【消除空工作】**：若当前被确认共享的保留 Span 覆盖全部 GPU Quad 字节，虽然静态签名可能由于动画拓扑发生变化，但本轮物理 Quad Bytes 没有缺口，`dirty_complement` 返回 `QuadUploadPlan::None` 而非 `Ranges(SmallVec::new())`；两者原生写入字节均为 0，后者仍要保留无用的区间状态。零 span、异常 Range、被替换 Slot、新旧内容不同仍走 Full 或既有安全局部上传。
- **【测试源码，未执行】**：同 Arc 与深拷贝相等 Span 的上传间隙必须一致；满驻留时返回 None；同 Arc 元数据倒序、重叠、越界、空范围均强制 Full。使用独立 JS 模型生成 60,000 组随机区间（含不合法区间和非同 Arc 情况），新旧真实上传字节区域对照一致；这不是运行 Rust 单测。
- **【风险与限制】**：无 rustc/cargo/rustfmt 和 Windows 原生 GPU，未验证 rustfmt、编译、GPUI lib tests、DX11/DX12/Vulkan/OpenGL。该提交只减少 CPU 上传规划扫描和空计划表达，不改变底层 GPU Buffer、Fence、Upload ABI 或跨 Slot 生命周期；实际帧耗时/GPU 占用未测。下一步继续减少 FrameUpload 静态流重复 hash/上传和复用 DrawStep 的有效性检查，持续删除 SceneSlot/Replay 技术债而非添加旁路缓存。

## 2026-10-15：彻底跳过零写入帧的 GPU Buffer 提交

- 基线远程 master `d2154ca382d73c9a222037f5aef2200e12e44788`；修改 `crates/gpui/src/platform/nova/renderer/buffer_upload.rs` 和本唯一 TODO。纵向范围仅涵盖静态/动画 FrameUpload → Borrowed BufferUploadBatch → Native ResourceDevice；无新增 FrameUpload、RenderTree、GPU 缓存或兼容适配层。
- **【代码实现、未 Rust 编译】**：`upload_frame_buffers` 原先即使全部静态流干净、`QuadUploadPlan::None`、没有 CPU animated primitives 或 animated backdrop-blur pass，仍执行全静态流/Quad/逐类动画的规划遍历。现在通过纯函数 `requires_buffer_upload_planning` 跳过无写入帧的规划步骤，保留原计时入口。
- **【原生 GPU 快路】**：`upload_buffer_batch` 曾在 `BufferUploadBatch::writes().len() == 0` 时仍调用 `ResourceDevice::write_buffer_batch(device, [])`。新版在此情况下直接采用 `gfx_core::BufferUploadStats::default()`，不进入任一后端 Native Driver 写 Buffer 路径，仍提交 `BufferUploadMetrics` 全 0 和现有计时样本。gfx-core 各后端零批次语义已按代码确认是零调用/零上传；非空情况下完全保留原顺序、Barrier/Fence/Slot 与错误返回。
- **【测试源码，未执行】**：增加零写入帧判定、静态 dirty Full、Quad Full/局部 Ranges 均须规划，以及空 Batch 统计契约的单测；代码审查确认 GPU-indexed animation tables 的更新并不经此 Native BufferUpload 路径，保留其独立原有更新流程。没有模拟 BackendResources 或运行测试。
- **【风险与验收】**：当前环境未提供 rustc/cargo/rustfmt，也没有 Windows GPU；尚未执行任何原生后端/单测和性能记录。此改动只会减少无写入帧 CPU 规划与无效 Native API 进入次数，并不能减少实际的 RenderPass/Draw/Present 或证明 GPU 20%+ 占用问题解决。后续应给静止页面、纯 GPU 动画与小 Dropdown 创建独立 profiling 基准，验证每帧写数/绘制数/Present 数，并持续推进 SceneSlot/Replay → 唯一 Paint Chunk 和独立 Compositor Lane。

## 2026-10-15：按唯一 Scene 编码代次失效 DrawStep 与 Path Mask 保留描述符

- 起点 master `11acc281b828b0227be9ac0e7461a5f76f84e0c6`。代码修改 `crates/gpui/src/platform/nova/frame_upload/{frame.rs,encode.rs}`、`renderer/{draw_steps.rs,draw_step_scratch.rs}` 和唯一 TODO；原生 DX11/DX12/VK/GL 的 Buffer ABI、Fence、Target/Present 提交顺序均不改。不新增第二套完整 Scene 或跨帧 Chunk 缓存。
- **【确认的源码风险】**：`NovaRenderer::pack_scene` 的 `UploadKey` 除 Scene revision 还考虑 Atlas placement generation、drawable size、blur quality、输出 Alpha，且动画结束时可强制重编码。之前 `DrawStepCacheKey` 仅在 RendererExtension 非空时带 `dynamic_frame_id`，否则相同 scene revision/size/Atlas Texture generation 可能重用旧 `FrameUpload.batches` 对应的 RenderStep；即使完整重编码后批次拓扑、root filter capture、资源绑定已经变化。PathMask 描述符缓存同样只由 scene revision 与 raster resource set 决定，无法显式区分一次完整重编码。
- **【代码实现，未编译】**：将现有 `FrameUpload.renderer_extension_frame_id`（实际上每次 root `encode_scene(reset=true)` 都递增）统一重命名为 `FrameUpload.encode_generation`，避免给 renderer extension 单独维护错误概念。该唯一代次直接进入 `DrawStepCacheKey` 和 `PathMaskCacheKey`；重新编码即使 producer Scene revision 不变，也失效两个描述符缓存。递归子 Blur 的 `reset=false` 不增加代次；普通 `refresh_retained_animation_values` 只刷新动画值，不会递增代次，继续保留 per-slot DrawStep cache hits。现有 frame slot/size/atlas/alpha/resource-set 判据和主动失效逻辑都保留，避免跨 Slot 缓存指向错误 ResourceSet。
- **【测试源码，未执行】**：新增 same Scene revision + new encode generation 时的主 DrawStep 与 PathMask rebuild，重复相同代次时应命中；FrameUpload 根 `encode` 两次代次 1→2，动画值 refresh 不增加代次。已全局核对旧字段的其他具体调用点仅在上述文件中。该修改属于缓存正确性修复，可能增加过去错误命中情况下必要的 DrawStep 重建，并非承诺所有页面 CPU 下降。
- **【验证门槛】**：环境无 cargo/rustc/rustfmt、没有原生 Windows GPU；未编译、未跑 Rust 单测或四后端。需本地 `cargo fmt --all -- --check`、`cargo check -p gpui`、`cargo test -p gpui --lib draw_step_cache` 和 `path_mask_cache`，通过后用动画完成、Atlas repack、Element Blur Root Capture、resize 场景截图与 GPU trace 验证无陈旧 RenderStep。
- **【未完成】**：该代次是当前 FrameUpload 的**同一个根编码序列**，并非长期统一 Paint Chunk version；SceneSlot、Scene Replay、全场景 FrameUpload Staging、DrawStep/DirtyRegion 跨类型持久增量和独立 GPU Compositor Lane 仍待破坏性重构。下一步基于真实 Present / DrawStep 请求计数确认静态页面的重复提交来源。

## 2026-10-15：原生 RendererExtension 动态 DrawStep 的跨帧正确性

- **起点**：master `8f4f0f0da7d475ca7e160ad788491521f076dc4c`；检查 `crates/gpui/src/platform/nova/renderer/draw_steps.rs`、`renderer/draw_step_scratch.rs`、`platform/nova/draw.rs`、`renderer.rs`。本次代码仅修改 `draw_steps.rs`，同步本 TODO。不增加第二套 Scene、FrameUpload、缓存 Owner 或功能配置开关。
- **【定位的真实风险】**：此前新统一的 `FrameUpload.encode_generation` 仅在完整根 Scene 编码时变更，而 `prepare_renderer_extensions(frame_time)` 每帧重新调用扩展，生成可变的原生 `RenderStepDescriptor`。主 `DrawStepCacheKey` 在同一 revision/encode_generation/size 下仍可能命中旧描述符：GPU Buffer、管线、资源绑定或扩展输出变动均可变成错误绘制。另 `prepare_renderer_extension` 原先只有进入 native prepare 后才 `steps.clear()`；若上一帧可见、本帧 Scissor 不可见而提前返回，则 `renderer_extension_steps` 仍包含上一帧命令。
- **【代码实现，尚未 Rust 编译】**：凡根 FrameUpload 含 `renderer_extensions`，现有 `DrawStepScratch.prepare_steps` 使用 scene revision=0 的**现成非缓存路径**，每帧读取本帧原生扩展 DrawSteps；纯 GPUI 场景则继续以 `encode_generation` + Scene Revision/Frame Slot/Atlas/Alpha/Size 保留并复用 DrawStep。新增泛型 `prepare_renderer_extension_step_storage` 在每帧遍历扩展前一次性 resize/truncate 并 clear 全部步骤 Vec，沿用其 capacity；任一扩展因 bounds/mask/空 scissor 提前返回时已无历史命令；成功 prepare 后原有顺序、Scissor 及 `draw_steps_for_upload_into` 的输出不变。
- **【测试源码未运行】**：新增真实 `DrawStepScratch` 的扩展帧不命中、扩展移除后恢复稳定缓存测试；以及扩展命令在被遮挡前清空、Vec capacity 复用和数量缩减的纯函数测试。当前环境检查仍无 `cargo/rustc/rustfmt`，没有 GPU，四后端未验证；提交只证明源码层面规避旧描述符，并未实测 DrawStep 耗时或 GPU 20% 占用下降。应先在 Windows/Linux 用动态 RendererExtension 与窗口遮挡/移动对照源缓冲和 GPU Trace，核查空命令不造成意外空帧。
- **【继续任务】**：向源代码中对齐 `Render Graph → Present` 的帧请求源与静态场景跳过策略，不能仅因 Buffer Upload/DrawStep Cache 正常就认为 Present 不再被唤醒。统一 Paint Chunk → GPU Compositor Lane、SceneSlot/Scene Replay/FrameUpload 老路径的安全迁移仍未完成。

## 2026-10-15：合并 Element Blur Source 查找入口，移除每层重复线性扫描

- 起点 master `3120dbc3f91f5d3e5a21851f4d30f0ce4536f95f`。源文件 `crates/gpui/src/platform/nova/frame_upload/layers.rs`、`renderer/present.rs`、`renderer/draw_steps.rs` 和本唯一 TODO。GPU Filter Registry、Quad Buffer、DrawStep 顺序、Render Graph/Present API 均无改动；不新增 Source HashMap 或长期第二缓存。
- **【审计确认】**：`FrameUpload.element_blur_inputs` 按 packed GPU Blur Index 严格递增写入（ElementBlur 先分配 index、记录源，再递归 child；AutoStatic 只在编码后追加最新索引），而 `blur_content_ranges_cache` 按 depth 倒序、content_start 正序供滤镜父子依赖绘制。两种排序不能混用，原 Present `dirty_element_blur_indices` 每层 `iter().find`、DrawStep `prepare_element_blur_layers` 每层 `find` 和 `any`，嵌套多层时造成重复 O(L×S) source 扫描。
- **【代码实现，未 Rust 编译】**：`FrameUpload::element_blur_source_for_index(u32)` 作为唯一查找入口，利用**既有**按 packed index 排序的源 Vec 做 `binary_search_by_key`；Present 和 DrawStep 三处查找全部迁移。不复制 Source，不制造新索引或新生命周期。源拓扑在 `refresh_blur_content_ranges`（仅重新编码时执行）增加递增顺序的 `debug_assert!`，捕获未来编码插入顺序错误；每帧 Presenter 不再遍历 Source 前缀。理想静态过滤器集合查找复杂度由每层 O(S) 降为 O(log S)，整体常见帧成本需真实测量。
- **【测试源码，未运行】**：增加稀疏 packed Blur Index [1,4,7,13,21] 与反向/乱序查询、缺失值/空表的回归测试。该 Lookup 不改变 Deepest-First GPU pass 排序，避免嵌套滤镜父子顺序倒置。
- **【验证及风险】**：没有在容器执行 cargo/rustc/rustfmt、DX11/DX12/Vulkan/OpenGL 本地编译与原生绘制；静态审查仍不足以证明 GPU 占用或静态 Present 问题得到解决。本轮保留原有 Frame Scheduling 空动画 tick 跳过语义，没有引入旁路静态 Present skip（其 WSI、GPU 动画和待提交 Scene 关系尚需实机追踪）。后续聚焦 View/Window Frame Request → GPU Render Graph/Present 的实际 5 秒计数和 DirtyRegion 证据，再考虑独立 Compositor Lane。

## 2026-10-15：追踪静态窗口 Frame Request → Present，分离空动画 Cadence 与显式 WSI

- 起点远程 master `05443561392c97f82ca1e0725fb3d842b127696f`。本阶段修改 `crates/gpui/src/window/frame_lifecycle.rs`、`crates/gpui/src/platform/frame.rs` 与本唯一 TODO；不新增帧状态所有者、不变更原生 Swapchain/DrawStep/Render Graph 语义、不添加不受证明的「静态强制跳 Present」分支。
- **【静态审查】**：`run_presentation_phase` 已用 `PlatformFrameRequest::resolve_animation_presentation(presentation_ready)` 在空动画 Cadence、没有待激活 Scene/未完成 Present 时屏蔽 Swapchain 提交；Required（WSI/外部渲染/显式 Present）不能因为 UI 看似静态而取消。既有 per-window 指标统计 redraw/draw/present，但 `log_frame_work_decision` 在 Trace 中输出常量 `request_id=0`，也未显示已经有的 `DirtyFrameDiagnostics.frame_request_reasons` 与首次请求 caller，使 5 秒高占用追踪无法区分动画、notify 与外部原生请求。
- **【代码实现，未编译】**：新增不分配内存的 Trace 诊断 `gpui presentation gate`：分别输出 pending Scene、未完成 Present、animation_visual、ready、原始/resolve 后的 Presentation Request、UI Commit、已提交 Scene、窗口可见/最小化与实际 early submit。现有 `gpui frame work` Trace 去除始终为 0 的假 request_id，输出已收集的 request reason bitset 和首次 reason/source(file:line)，并保留 dirty / render provenance、draw/present/skip 标志。诊断仅在 `log::Level::Trace` 打开时执行，**不**额外派发平台帧或插入渲染 Thread/Queue，Trace 内状态仍保持原生命周期，不误称来源掩码代表全部 WSI 事件。
- **【测试源码，未运行】**：新增 `PlatformFrameRequest` 三组调度契约测试：无视觉变化的动画 Tick 取消 Present 但保留 UI Tick；与 WSI/Required 合并后的 Present 不可吞掉；有损伤动画保留 Present。行为生产代码未改，避免未验证的帧跳过造成 DX11 拖拽、Vulkan live resize、最小化恢复黑屏回归。
- **【验收与下一步】**：当前容器缺少 cargo/rustc/rustfmt，Windows/Linux 四后端实机、GPU trace 与耗电/吞吐尚未验证。需本地运行 FrameRequest/Window 测试并用 `RUST_LOG=gpui=trace` 对静态 5 秒、Dropdown 展开/收起、Blur on/off、动画结束 5 秒分别收集 `gpui presentation gate`、`gpui frame work` 和 Nova draw_step_cache_hit / upload_bytes / actual_present，对比帧请求源；若发生纯 Presentation 多余提交，再修改**真实发起者**，而非添加第三套节流或缓存。统一 Paint Chunk/Compositor Lane、SceneSlot/Replay 删除仍未完成。

## 2026-10-15：简化四阶段 Nova Render Graph，删除每帧动态拓扑搜索

- 基线 master `328f513332b7fb214f22ce821fdda5ec0cbd5258`；本阶段修改 `crates/gpui/src/platform/nova/renderer/frame_graph.rs` 和本唯一 TODO。未改动 GPU API、RenderPass 实际提交、跨 Slot 纹理缓存、Fence、外部 WSI Request/Present 判据。所有四类 Pass 仍用同一 `FrameGraphPlan` 传给现有 Present 执行入口。
- **【已实现，未 Rust 编译】**：之前 `FrameGraphPlan::compile` 每次创建 `SmallVec<Node>`、节点 `dependencies` bitset、顺序查找可执行节点并生成已固定的拓扑序，还在执行图生成时再次按 Pass 逐项查找 Node。当前硬编码的四节点资源依赖始终单向、顺序固定（PathMask → ElementLayers → BackdropBlur → MainPresent），不是用户插件可扩展 DAG。删除 `Node`、`ResourceUse`、`Pass::bit`、运行时 topo-sort 和所有临时 Node Access Vec；根据本帧确有的 Mask/Layer/Blur 直接产生执行序列，再按该序列生成现有 `BarrierIntent` 和 `ResourceLifetime`。Native Barrier 仍由 Backend 执行，不开启跨帧/跨纹理别名复用。
- **【静态帧快路】**：当本帧三个离屏阶段都不需要刷新时，`FrameGraphPlan::compile(false,0,0)` 直接返回唯一 `MainPresent`、空 Hazard 和 Swapchain ExternallyOwned 的 `first=last=0` 生命周期，不做通用依赖扫描。Render Graph 仍保留 MainPresent 的真实 WSI 提交，**没有**错误取消窗口暴露/恢复/显式 Present。
- **【测试源码，未运行】**：新增 8 种三阶段有效/无效组合全排列的固定顺序、Barrier 数、资源 Count、纹理 first/last/Residency 验证；另验证多个只读消费者不产生 Read→Read Hazard。使用独立 JS 逻辑参照逐个比较旧/新 8 种拓扑的 Pass、Hazard 顺序/访问类型及生命周期数组，结果一致；**不能替代 Rust 测试、native GPU Barrier Test 或本地性能测量**。
- **【后续风险与目标】**：本改动减少即使静态 DrawStep 缓存命中时仍执行的 CPU Graph Planning；并非证明实际静态窗口不会重复 Present，也不能证明 20–30% GPU 占用下降。下一步应拿刚加入的 `gpui presentation gate` / `gpui frame work` 与 Nova Pass/Present 实机 5 秒 Trace 确认帧请求源和 Graph 次数。依然未完成跨类型持久 Paint Chunk、GPU Resident Buffer/DrawStep/DirtyRegion 内容版本统一、SceneSlot/Replay/FrameUpload 淘汰、独立 Compositor Lane；不得仅凭固定 Graph 声称全链路统一。

## 2026-10-15：单遍动画 Stream 共享判断与 GPU Slot 签名差异复用

- **远程基线**：master `cf69dacaba2aff331fa80aa22473580a42463b60`。修改 `crates/gpui/src/platform/nova/resources/buffers/streams.rs`、`renderer/retained_upload/sharing.rs`、`renderer/retained_upload.rs` 和本唯一 TODO。只整合现有 BufferStream/RetainedUpload 的元数据工作；不创建第二套 GPU owner、FrameUpload 或重叠资源缓存。
- **【代码实现，未编译】**：`BufferStream` 使用显式 `#[repr(u8)]` 和 `index()`；10 个成员与 `ALL` 的 canonical 顺序相同，测试校验映射，避免 per-stream `ALL.iter().position()`。对于每帧 `RetainedUpload::update_shareability`，删除原来每个 Stream 重复扫描全部 `animated_primitives` 的逻辑：初始化可共享位，一次遍历全部 CPU-patched Primitive 按原有 AnimatedPrimitiveKind 标记 Quad/Shadow/Mono/Poly/BlurRecords 不可共享；`BlurPasses` 独立继续按 `has_animated_backdrop_blurs()` 判定，未改 GPU Indexed Animation promotion。
- **【Slot 生命周期正确性】**：原 `mark_uploaded(slot)` 对每个 10 流调用 `stream_state`，重新取 Slot 签名、复制签名并匹配。现在从同一权威 `StaticUploadSignature::diff(previous)` **一次**计算各流 dirty 位并与此前 `uploaded_shareable` provenance 合并。保留「CPU Patch 后即使之后不再动画、签名相同，也不能在没有静态基础字节重新上传时错误共享」的语义；新版本真正上传或 Buffer 重建后恢复共享。每个 Slot 独立，FrameSlot/COW Buffer/Fence/Barrier 不变。
- **【回归验证】**：新增顺序映射、全部 10 种 Stream 的 CPU Patch → 静态签名相同保持非共享 → invalidate/re-upload 恢复共享，以及 PolySprites 单流改变不污染 Quad 的测试源码。另独立 JavaScript 模型比较 100,000 组随机旧/新 Slot 签名与共享状态、2,048 组动画 Primitive 类型序列，结果完全一致。**这不是 Rust 编译/测试**。
- **【未验证/下一步】**：当前容器仍无 `cargo`、`rustc`、`rustfmt`，没有 Windows/Linux 原生 GPU。需本地编译、运行 `gpui` 单测及四后端回归，确认 Scene revision 切换、CPU Patch 尾帧、Slot 轮换、Buffer Replace、live resize 均不发生错误共享。性能收益仅限减少元数据查询与 CPU 扫描；真正重复的 Frame Request → DrawStep → Native Present 和 Paint Chunk/SceneSlot 重构尚未解决。后续应针对重复的 `AnimatedPrimitive` 编码与 `FrameUpload` 全场景 staging 开始纵向迁移，不能把简单循环减少误认为统一 Compositor 已完成。

## 2026-10-15：补齐 Retained Quad 局部上传时 CPU 动画字节的覆盖证明

- **基线**：远程 master `c749d1c0726dbb6158ebd7c67cc6516f013494c7`。修改 `crates/gpui/src/platform/nova/renderer/buffer_upload.rs` 和本唯一 TODO；直接修复旧 `FrameUpload → QuadUploadPlan → BufferUploadBatch` 同一条规划路径，不引入旁路动画缓存、新 FrameUpload 所有者或兼容层。
- **【已定位正确性风险】**：`FrameUpload::sample_primitive_records` 每帧就地修改 CPU-animated packed Quad 字节，而过去 `plan_animated_buffers` 使用 `if !dirty.quad` 决定是否对 Quad 动画进行 GPU patch。静态上传 `dirty.quad == true` 并**不**必然表示 Full 上传：QuadResidentLayout 可能生成局部 `Ranges`，甚至静态签名只因 animation topology 改动而产生 `None`；因此动画 Quad 可能落在其未覆盖的 Gap，产生陈旧 GPU 数据。
- **【代码实现，未编译】**：新增纯范围算法 `for_each_uncovered_animated_quad_range`：`Full` 表示已覆盖无需动画额外写入，`None` 必须提交全部 CPU 动画字节，`Ranges` 则对每个动画 Quad 只规划未被静态脏区间覆盖的补集。静态范围保持有序/不相交，使用 `partition_point` 定位首个相交区间，避免从第一个 retained span 反复扫描；补传通过唯一 `plan_quad_range` 和 `PackedQuadStream::slices` 完成，不复制数据，不重写 Buffer/Fence 生命周期，不对已经覆盖的字节重复提交。
- **【测试源码未运行】**：新增 Full/None/空范围、动画跨多个 retained islands、部分重叠与完全覆盖测试。独立随机模型比较 120,000 组不同顺序的有效静态范围与动画范围，逐字节判断计算出的补集准确（不是执行 Rust 单测）。
- **【正确性边界】**：当 `QuadUploadPlan` 不合法时上游应降级 Full，本辅助函数仅处理上游已验证的有序计划；其他 Shadow/Mono/Poly/BlurRecords 仍按原有静态 Full 与动画 patch 规则。所有非空 Backend Buffer Upload 仍遵守已有提交顺序，WSI Present 与 Render Graph 未改；本次主要是规避遗漏及重复写入，不承诺 GPU 使用率必降。
- **【未验证与下一步】**：当前环境没有 cargo/rustc/rustfmt，尚无 gpui 单测、DX11/DX12/Vulkan/OpenGL 真实 Buffer/Fence 测试或显存/帧延迟证据。必须本地重点验证“CPU Quad 动画 + Retained Static Chunk + FrameSlot 轮换 + Scene Revision 变化 + GPU COW Resize”组合。之后继续收敛局部 Paint Chunk/DirtyRegion/DrawStep 的权威内容版本，并逐步替代全场景 Scene/FrameUpload staging。

## 2026-10-15：停止完全静态 Present 的 Presentation Clock GPU 写入与空批次 GPU Owner 设备锁

- 基线 master `2b5428b957acb33e185f9dabb6d4aeb3178011fa`。修改 `crates/gpui/src/platform/nova/renderer/submission.rs`、`renderer/buffer_upload.rs` 与本唯一 TODO。不增帧时钟/状态权威所有者，不引入新的 Upload/Scene/FrameGraph Cache，不更改 Native Present 或 FrameSlot/Fence 行为。
- **【已确认的重复工作】**：此前 `upload_presentation_buffers()` 在每个已被请求的 Native Present 帧都对 `global_buffer[16..24]` 提交 8B Presentation Clock，即使静态 Scene 没有采样动画表、CPU 动画或 RendererExtension；随后空 GPU Indexed Table 的上传被跳过，但仍通过 SharedBackend Mutex 进入 Buffer 上传入口，静态画面「0 静态上传」不能代表整帧 0 GPU Buffer 写入。检索内置 Nova WGSL 显示 `globals.presentation_time_seconds`、`presentation_tick_60hz` 目前仅声明于 Core GlobalParams，没有被内置绘制 Shader 消费（自定义 RendererExtension 是独立边界，仍保留 Clock）。
- **【代码实现；未 Rust 编译】**：以现有 `FrameUpload` 动态状态判断是否上传 Clock：`renderer_extensions`、`sampled_animation_values`、`gpu_indexed_animation_slots`、`animated_primitives` 中任一非空都必须保留由 `frame_time` 单一帧时刻采样的 Uniform Clock；仅全部为空的内置静态场景跳过更新 `globals[16..24]` 和 8B Native 时钟写入。保留 `rebuild_indexed_animation_values`、`prepare_static_buffers`、`ensure_stream_capacity` 的顺序与所有 GPU 资源变更逻辑。
- **【原生锁/统计统一】**：`upload_buffer_batch` 与 `submit_presentation_buffers` 使用同一个 `record_empty_buffer_batch` 统计入口；完全空的借用批次在 SharedBackend Mutex **外**立即记录 requested/actual writes、backend calls/bytes 均为 0 与时间后返回。非空路径仍按原 DX11/DX12/Vulkan/OpenGL/Metal 调用、原顺序上传，不跳过 Native WSI Present、不忽略 FrameSlot 资源安全。
- **【测试源码，未执行】**：新增内置静态场景零时钟上传、GPU indexed slot 存在但 timeline 空时仍保留 Clock、CPU 动画 sample 保留 Frame Clock、相同 frame_time 的 Timestamp 确定性测试；原 `BufferUploadBatch` 空写入契约测试保留。需要本地复核存在 RendererExtension 时的 clock、静态曝光/resize、动画末帧、slot 替换四后端结果。
- **【未验证与下阶段】**：容器仍无 cargo/rustc/rustfmt；本次未完成 Rust 编译、单测、GPU Trace、DX11/DX12/Vulkan/OpenGL 实机。减少了静态帧逐帧 GPU Buffer 更新，但静态页面若仍不断 Present，DrawStep/RenderPass/Swapchain 仍消耗 GPU；必须基于 `gpui presentation gate` 对照实际 Native Submit 原因，再改 Frame Request 的真实来源。独立 GPU Compositor Lane、持久 Paint Chunk 与 SceneSlot/Replay/FrameUpload Staging 破坏性替换仍未完成。

## 2026-10-15：Root Color 物理身份验证与内容版本的保守独立保留（P0 阶段切片）

- **基线**：远程 master `a585dc4a143d1e3f829d58144e72c6c6e3ab44e3`。修改 `crates/gpui/src/platform/nova/{targets.rs,tests.rs}`、`renderer.rs`、`renderer/{filters.rs,draw_steps.rs,present.rs}` 与唯一 TODO。此前 Root Color 由 `retained_root_blur: Option<u32>` 指向零半径 BackdropBlurVariantTargets，`ensure_backdrop_blur_targets` 即使物理 Target 在 Capture reorder/数量减少中复用成功仍一律 `invalidate_backdrop_blur_cache` + 全部 Filter 捕获重新刷新，这与根窗口静态颜色驻留目标相悖。
- **【实现，未编译】**：`BackdropBlurTargets::retained_root_texture_view` 读取**实际 GPU TextureViewId**，而不是以扁平 Blur index 当身份；`can_preserve_root_color_capture` 只允许无共享 backdrop source、无 isolated source、旧 Capture 均零半径的目标链。GPU FilterRegistry 独立维护 `root_color_version: Option<TextureContentVersion>`，仅在**成功 Present 且 Root source pass 确实刷新**后递增；跳过 Root Pass 保留版本，Root layer 消失或原生资源失效则清除。该内容版本不再依赖 Element Source 的索引快照，也不因为其他 Capture 重新编码自动增加。
- **【复用正确性】**：`ensure_backdrop_blur_targets` 在 `reuse_layout` 和 `rebind_zero_radius_layout` 两条现有 GPU 目标重排路径中，只有**原 Target 已提交过有效内容**、此前 Filter 状态有效、旧新 Capture 均无 Gaussian、没有 Root BackdropSource、且重排前后 Root TextureViewId 完全一致时，才调用 `invalidate_element_sources_preserving_root_color`：保留根颜色 GPU 内容版本，失效所有 Element 捕获快照及它们的前帧动画样本。新 Capture/重映射 Capture 在 `prepare_element_blur_layers` 没有已验证 source content version 时，强制**完整 Capture 初始化**，不得用 Popup 的小 DirtyRect 填充未初始化纹理。其余路径继续原有全量 Filter 失效，目标重建继续在旧 GPU Fence 完成后销毁旧资源，绝不在资源代次不匹配时复用旧纹理内容。
- **【测试源码，未运行】**：Root 版本初始为空、实际 refresh 后建立、未 refresh 时不变、其他 Element source 失效不影响 Root、Root 消失/全失效重置；已在 `warmed_zero_radius_popup_targets_rebind_without_reallocating_textures` 增加多种弹窗数量和根索引轮换前后 TextureViewId 完全一致的断言；补充新/重映射 Capture 必须 full 初始化的测试。
- **【本阶段严格限制】**：此提交仅把 **Root Color 失效决策与内容 epoch** 从全局 Filter snapshot 分离，尚**未**将 Root Color 的原生 Texture/ResourceSet 从 `BackdropBlurTargets.variants` 独立为物理 `RootColorTarget`，packed 合成命令也仍使用零半径 Blur ABI，故 P0「真正资源所有权拆分」仍为 **未完成**。本阶段不会假称 Capture 已有稳定 `layer_id + GPU generation`，也没有实现完整静态背景与动态前景独立合成；当前无 Rust/Cargo/Windows GPU，Rust 单测、四后端提交/Fence、GPU 使用率收益均未验证。
- **【下一步 P0 资源拆分】**：把根颜色 `Texture+View+per-slot ResourceSet+content version+size/format generation` 全部放到唯一 RootColorTarget owner，真正移除 Root 对 FilterVariant 的物理依赖；以 Scene `PaintBlur.layer_id` 作为明确稳定 Capture ID（无 ID 必须保守处理），`CaptureId → Target/Generation` 单一索引替代当前 `BackdropBlurConfig.member_first/last` 作为资源身份，重排时保留稳定 Target，淘汰者由提交 Fence 完成后退休；建立 popup 插入、删除、重排、缩放动画、resize 和 in-flight 多帧测试。完成这一步后再复核一层局部 GPU Composite：背景像素与子 Capture 的 Dirty 必须分离，不允许 popup 的透明度/平移/缩放样本重放 Root static Scene。
- **【P1 GPU Timestamp 评估】**：仓库已有 Vulkan `vk::QueryType::TIMESTAMP` 及 DX12 `D3D12_QUERY_HEAP_TYPE_TIMESTAMP` 用于局部 GPU 上传计时，但 `gfx-core` 尚无跨 DX11/DX12/Vulkan/OpenGL 的通用 RenderPass 时间戳契约；不能把 CPU `Instant::elapsed` 记作 GPU Pass 耗时。后续需统一 `GPU_PASS_ROOT_REFRESH`、`ELEMENT_SOURCE`、`GAUSSIAN_H`、`GAUSSIAN_V`、`FINAL_COMPOSITE`、`SWAPCHAIN_PRESENT` 标签与每 FrameSlot QueryRange；在提交栅栏完成后延迟无阻塞 Resolve/Readback，DX11 disjoint、DX12 frequency、VK validBits/timestampPeriod、GL availability 单独校准，不支持时标记 unavailable，防止 UI 线程/共享 GPU Owner 同步等待。这项 P1 仍**未实现**。

## 2026-10-15：公共 Dropdown 单 Capture Reveal（待 GPU Trace 验收）

- 远程起点 `f24fe7cd88b01a817ed2e48f3480ca05d8307aa8`，修改 `src/ui/components/dropdown.rs` 与本唯一进度文档；仅影响公共 Dropdown 弹层，不修改首页独立版本下拉框、不改变 GPUI/Nova-GFX Shader ABI。
- **【源码证据】**：公共弹层原先嵌套 `composite_layer_with_id(active.id)`、Opacity 动画、ClipReveal 动画。`ClipReveal` 在 `paint_scene_animation` 必定经 `with_scene_composite_animation → paint_composite_layer` 捕获整个菜单，内部 Opacity 对显式 Composite Layer 再做一次独立零滤镜捕获；两级源目标可能扩大 Render Graph 和纹理写入。`.occlude()` 仅调用 `occlude_mouse()`，是命中测试而非 GPU 像素遮挡或 DirtyRegion 操作。
- **【代码实现；未编译】**：公共菜单只保留同一个 `active.id` 标识的 GPU ClipReveal 子树捕获，删除重复显式 Composite 和独立 Opacity Track；不引入另一套缓存、动画时间循环或条件性全页 Render。固定边缘、原有 220/160 ms 曲线、反向中断与菜单内容/滚动/输入事件不变。**视觉取舍**：移除了菜单展开/收起时 0.78↔1.0 的轻微整体渐隐，保留 Reveal；不是无损像素等价变更。首页独立下拉框不受影响。
- **【实例隔离审查】**：`DropdownState` 使用 `window.use_keyed_state(id)`，每个触发器持有独立选项/滚动/phase；全局 `DropdownOverlayState.active_by_window` 当前每个窗口最多一个活动菜单，其他实例的内容不会自动进入此菜单。切换 phase 仍会通知拥有该 Dropdown 的父 View，全窗口 Root/Paint 及 Root Color GPU Target 是否在动画每帧重绘仍需由现有 Trace 验证，不能仅凭源代码宣称已彻底局部合成。
- **【验收阻塞】**：当前环境无 cargo/rustc/rustfmt/Windows 原生 GPU，未执行编译、单测及 DX11/DX12/Vulkan/OpenGL 实机。必须对比相同窗口/刷新率、Blur on/off、单/多个 Dropdown 连续快速点击：`gpui frame work`、`gpui presentation gate`、Capture 数量/Pass 次数、Root target version、retained_copy_pixels、GPU Draw/Present/实际占用与动画完整性。期望单个公共菜单 Capture 从嵌套目标收敛至一个，**GPU 40%→多少尚未知**；RootColorTarget 物理所有权分离、Fence 退休和异步 GPU Timestamp 仍未完成。

## 2026-10-15：稳定 Element Capture 跨索引保留真实纹理版本（未编译）

- 起点 master `3e1324b6084a2e45530d06f206c23129b3a7031b`；改动 `frame_upload/blur.rs`、`targets.rs`、`renderer/filters.rs`、`renderer.rs`，并更新本唯一 TODO。公共 Dropdown 仍使用上一提交的单 ClipReveal Capture，首页独立版本下拉框不变。
- **【已定位】**：`ensure_backdrop_blur_targets` 在 `reuse_layout` / `rebind_zero_radius_layout` 复用 GPU TextureView 成功时，原先仍清除全部 `FilterRegistry.element_blur_inputs`，可能导致与弹层无关的保留图层在后续 Present 被重新离屏刷新。
- **【实现；未验证】**：仅当 Root Color 的原生 TextureView 和已成功提交的内容版本保留时，从现有 GPU Target 提取临时 `(packed index, TextureViewId)` 列表；在唯一 `FilterRegistry` 中按非空稳定 `PaintBlur.layer_id`、前后完全相同的 ViewId、零滤镜/单成员独立目标三重验证后迁移 Snapshot 的 flattened index。其他目标释放旧快照，依旧走强制初始化；不增加持久缓存、额外 RenderTree/Scene/兼容层，不改变目标分配、Fence 等待、Shader 和 Native Present。
- **【边界】**：新内容仍经过现有 `FilterRegistry::source_unchanged` 的 Source/BBox/Mask/Animation 验证；物理 View 变更、无稳定身份、重复旧身份、正半径 Gaussian、合并目标不复用旧像素。旧 Root 失效继续全失效；未实现 RootColorTarget 物理资源独立化。
- **【测试源码未执行】**：添加 FilterRegistry 层身份变化、纹理变化、扁平索引重排、Gaussian 与源 bounds 变化测试。环境没有 Rust 工具链，也没有四后端原生 GPU：需 `cargo fmt --all -- --check` / `cargo check --workspace --no-default-features`，以及 DX11/DX12/VK/GL 单/多个 Dropdown、弹层重排、尺寸变化、切页的实际像素和 Trace 校验。GPU 40% 的变化未知；下一步聚焦 Root 源 DirtyRegion 与独立 Property Composite 的实际重复 Pass。

## 2026-10-15：复用现有静态前缀 GPU Capture，让零滤镜 Dropdown 在 Root 外独立合成（未编译）

- **基线**：远程 `475e03cdd9ddef7bcc8264b984253d29d8acd4b1`。改动仅 `crates/gpui/src/platform/nova/renderer/retained_upload.rs` 和唯一 TODO。不改 `Scene` ABI、Nova Shader、Native Backend 或图层缓存所有权。
- **【源码事实】**：`append_retained_root` 把整个页面（包括零滤镜 Dropdown Capture）包装到根颜色纹理中；`reconcile_gpu_layer_animation_damage` 正确地将菜单动效作为 Root 的局部脏像素，每帧只能局部重画 Root。现有 `auto_capture_static_background → capture_encoded_static_prefix` 可把静态 Quad/Shadow 前缀的 GPU 源单独驻留，动态前景直接以 Painter Order 合成，但原准入规则拒绝含任何 `StartBlur` 的 Scene。
- **【实现】**：允许在完整静态 Quad/Shadow 前缀**之后**开始的具备稳定 `layer_id`、零半径、无子 Capture、无 BackdropBlur 的单级 Popup Capture。以其闭合后实际 `Primitive::Blur.order` 作为动态 DrawOrder 边界，要求两侧 `range_is_independently_replayable` 且全部静态 order 严格小于动态 order；复用唯一 packed FrameUpload 和 FilterRegistry 的 AutoStatic Cache，不创建第二套 Scene/Root Owner。对小页面把静态前缀准入从原至少 96 Quad/Shadow 扩为 **≥24 且静态裁剪栅格工作量≥2 倍视口像素**，原 ≥96 规则不变；阈值是保守启发式，不是 GPU 实测。
- **【正确性保护】**：无法独立切分的结构化 Painter Layer、真正 Gaussian、嵌套 Element Capture、BackdropBlur、未标识 Capture、纹理/路径/Surface 等在 Capture 前出现仍退回原全量链路；Drawable Resize / Atlas 内容版本 / Filter invalidation 保留既有一致性逻辑。不会绕过 Fence 或吞掉必要 WSI Present。
- **【单测源码，未执行】**：新增 24 个静态全屏 Quad + 独立 Popup 的准入及 FrameUpload 包装测试，验证 Root 不再包住菜单；新增 Gaussian/嵌套结构反例。`GPUI color retention result` 现有 Trace 新增 `static_prefix_candidate` 与 `static_prefix_captured`（仅诊断字段，无新增配置/计时器）明确区分准入成功、编码拒绝、Root 回退。当前容器未安装 Rust/rustfmt、没有 Windows 真实 GPU，尚无编译/四后端 Trace 结果。该优化只作用于可证明有序且足够重的静态前缀；不能声称所有 BMCBL 页面、所有 Dropdown 都自动适用。后续必须测量准入命中率与 `retained_root_draw_steps`、`filter_source_draw_steps`、`composite_passes`、GPU busy、UI frame 延迟。

## 2026-10-15：AutoStatic 实体纹理跨 Popup 拓扑重排保留 + E0616/重导出告警修复（待编译）

- **基线**：远程 master `f3c2c050db5d56c83713b9d2bd5039787ee91172`，已包含用户随后提交的多项 UI Entity/覆盖层/Toast/Task 局部刷新优化；本阶段不回滚或覆盖这些 UI 提交。代码改动范围：`crates/gpui/src/gpui.rs`、`platform/nova/renderer/{filters.rs}`、`platform/nova/{renderer.rs,targets/layout.rs,tests.rs}` 与唯一 TODO。
- **【修复构建阻塞】**：`RetainedFilterSource.index` 对父 `renderer.rs` 不可见，触发 E0616；仅改为 `pub(super)`，不向 GPUI 公共 API 暴露私有纹理索引。`gpui.rs` 的 `pub use scene::*` 导出的是仅 `pub(crate)` 的内部 Scene 项，收窄为 `pub(crate) use scene::*`，保持内部消费者可用并消除公开重导出层级告警。
- **【P0 根因】**：在没有合成窗口 Root 的静态背景前缀模式，旧 `root_color_reuse_eligible` 恒为 false；原 `remap_identity_capture_sources` 又只认 `FilterSource::Element`，所以 Popup 插入/收回即便复用了原生目标，也会错误清空 AutoStatic 的 GPU 内容版本。单迁移 packed index 也不够：布局重排可能把 AutoStatic 的索引绑定到**另一张**物理纹理。
- **【本阶段代码】**：`FilterRegistry` 使用现有 `AutoStatic { bounds, content_mask, content_hash, content_bytes }` 四字段精确相等，并要求只存在一个已成功呈现且内容版本非零的旧/新 AutoStatic，才给出旧→新扁平索引。无 Root/无共享 source/无 isolated source/全部零半径的有效目标布局允许保留 Element/AutoStatic 快照；迁移时同一 `TextureViewId` 是必需条件。旧 target 布局 `reuse_layout` 须证明**原物理目标**映射到了新的 AutoStatic 索引，否则回退 `rebind_zero_radius_layout`：在所有检查成功后对整个 `BackdropBlurVariantTargets`（Texture、View、per-slot ResourceSet 同一 owner）执行稳定槽位交换，随后更新 packed config；不创建另一套缓存或抽象图层。
- **【保守限制】**：只使用已有成功提交的版本；内容指纹、内容遮罩/几何、真实 TextureView 变化、非独占单成员、Gaussian/Backdrop、资源重建、旧缓存失效一律重绘。新目标数量超过已预热容量时仍走既有创建/销毁和 GPU Fence 等待，**未实现跨 GPU allocation 的零复制增长**；当前也未拆出独立 RootColorTarget 物理 owner、未引入 Fence-retired Target Pool。
- **【测试源码，未执行】**：FilterRegistry 自动前缀真实版本跨索引复用、变更内容/位置/物理 View 拒绝；Targets 层两次相反的 Popup 重排、独占 TextureView/ResourceSet 维持、非法重排原子失败；原零滤镜/root/Gaussian 目标测试调用同步更新。不新增 CI 自动修复，不把静态审查算作 Rust 编译或原生四后端验证。
- **【下一步验收】**：先在 Windows 执行 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、针对性 GPUI 单测，随后分别测试 DX11/DX12/Vulkan/OpenGL 的静态前缀 Capture + Popup 插入/删除/倒转动画/Resize/切页。Trace 需核对 `static_prefix_captured`、`auto_static_relocation`、`retained_sources`、`element_blur_refresh`、`retained_root_draw_steps`、`composite_passes` 与实际 GPU 占用，尤其警惕静态场景版本错误复用和未初始化目标。当前执行环境没有 cargo/rustc/rustfmt/Windows GPU，**源码尚未编译/运行**，不承诺 GPU 40% 已改善。

## 2026-10-15：公共 Dropdown / AutoStatic GPU 缓存 Trace 验收切片（P0 待实机）

- **远程基线**：`f7035b0c26bb77aab986889c7a1109a3cccc7702`。新增仅在既有 `RendererOptions::frame_metrics` 打开时记录的 `GPUI AutoStatic submitted` 事件，发生在**成功 native 提交及 `record_element_blur_inputs/record_submission` 之后**，不是仅由 UI Paint/CPU Encode 推测 GPU 缓存命中。指标为 `window_id`、`scene_revision`、`source_index`、`source_dirty`、`refreshed_layers`、`source_draw_steps`、`native_view`、`content_version`、`retained_root`、`frame_animation_values`；未增加新的缓存、线程、环境开关或采样状态。修改 `crates/gpui/src/platform/nova/renderer/present.rs` 与本文；新增一个 Present 源失效测试。
- **【发现旧版真实 DX12 基线】**：用户上传的 `粘贴的文本 (1)(20261015-012213).txt`（2026-10-15 01:22 UTC，**早于当前 P0 代码**）记录 24 次 `GPUI retained target layout recreated`、23 次 `recreation required`、0 次 `reused` / `rebound`，布局反复 3→4→6（11,664,000→18,662,400→27,993,600 字节）；972×600 主窗口单帧 `composite_passes=12`、`blur_source_pixels=1,489,240`、`blur_target_pixels=805,240`，GPU Owner busy≈20.6%–21.3%（不等于系统任务管理器 GPU40%）。旧日志 `old_isolated` 非零、`previous_root=Some(...)`，并且没有 `static_prefix_captured` / `auto_static_relocation` 字段：**只能作为预重构退化对照，不能证明当前 AutoStatic 正常/异常**。
- **【正确性断言源码，未编译】**：`renderer/present.rs::per_layer_damage_tests` 新增「已成功呈现 + AutoStatic 内容指纹不变 + 真实 TextureView 不变 + packed index 从 2→5」必须从 Dirty 集中消失；强制刷新仍包含 index5；View 改变且 index5→3 时必须重新刷新。与 FilterRegistry/Target 前轮单测互补。
- **【本机验收判据】**：打开应用已有 `RendererOptions::frame_metrics` 诊断（不新增任何环境变量），在最新 master 上分别抓取静止窗口、公共 Dropdown 连续打开/收起/快速反向、首页版本下拉、多个菜单互斥切换、Blur on/off、窗口 Resize/切页，并用相同尺寸/240Hz、同一后端对照。只有当某个场景`static_prefix_captured=true`、后续成功提交报告 `source_index=Some(...)`，稳定动画帧显示 `refreshed_layers=0`、`source_draw_steps=0` 且原 `native_view` 和 `content_version` 保持不变，才可宣称「该场景 AutoStatic 不再重复离屏绘制」。拓扑变化时可结合 `auto_static_relocation=Some((old,new))` 及 `GPUI identity Capture target rebind`；没有拓扑变化时该字段为 None 是正常。内容变化、Surface Resize 或纹理资源代次变更的第一帧**应重新刷新**，不可要求所有帧恒为 0。
- **【尚未达到】**：尚未拿到基于 `f7035b0c26bb77aab986889c7a1109a3cccc7702` 之后版本的 Windows Trace；当前容器无 Rust/Cargo 与 DX11/DX12/Vulkan/OpenGL 实机，不能声称编译、单测或静态背景停止刷新已被证明。旧版多 isolated/source 参与的 Gaussian/Backdrop 目标拓扑不能简单套用 AutoStatic 零滤镜优化；若新 Trace 仍显示 `retained_root=true`、`source_index=None` 或不断 `recreation required`，应转向孤立 Source 物理身份与目标容量/预算/Fence，再考虑根颜色独立 owner，不要误把 AutoStatic 已复用当成全部 40% GPU 的解决方案。

## 2026-10-15：隔离滤镜 Source 已预热目标容量复用（P0 子阶段，待编译）

- **起点及并发校验**：本轮初始远程 master `4735619f1850d3c135327378d7ee172ee5036e4b`；提交前 HEAD 变为 `b73ddb9188e06117242b3cd8e5f51f17dcf90fac`（新增 3 个 UI-only 提交，触及 `src/ui/main_window*`、任务页，和本次 5 个 GPUI/TODO 文件无交集），据此重新确认远程 Tree 并以新 HEAD 为父提交。不覆盖用户的 UI 优化。旧 2026-10-15 早期 DX12 Trace 记录 3→4→6 Variants 重建循环与约 11.66→18.66→27.99 MB 的 Target Pool 瞬时分配，且 `old_isolated` 非零；此日志早于当前代码，**不能证明本阶段 GPU 收益**。
- **【根因与实现】**：旧 `BackdropBlurTargets::reuse_layout` 要求全部 `isolated_sources` 长度及扁平索引与当前过滤后的需求一致；即使 GPU 已预热 6 组 Scratch，Popup 缩减到 3 组后也会判失败并销毁/重建全链。现在 `BackdropBlurTargets` 直接持有唯一 `active_isolated_source_count`，将全部已分配 Scratch 存于原 `isolated_sources` Vec、其前缀作为当帧活动索引；非活动 Scratch 从 `isolated_source_texture_view/resource_set` 查找中排除，`is_layout_compatible` 比较活动前缀而非全部已分配容量。仅在 GPU resident 目标和源容量均足够、root-source 类型一致、Variant 经过逐个独立成员/Downsample/Pass 数验证后执行一次原位变体排列，物理 Texture/View/ResourceSet 整体移动，Root/AutoStatic 仍固定原生 View。隔离 Scratch 不作为内容版本持有者，允许按当帧新 flattened index 绑定已有物理 Texture；FilterRegistry 的真实源版本由成功提交管理。
- **【顺序与安全】**：已有 `frame_submission_is_ready` 在 Filter layout 变动且尚有 in-flight GPU 提交时会延后变体重排；同步模式 `prepare_for_frame_submission` 继续等待 Fence。两阶段先完整验证再交换，失败绝不修改 Target 布局，内容身份无法证明的变化仍由 `invalidate_backdrop_blur_cache` 全量初始化；超过历史预热容量、格式或尺寸变化仍走原来的 GPU Target 创建/销毁与生命周期。活动数与分配数在复用、重建日志中分别打印，避免把备用容量误判为活动 Pass。资源池总显存按**全部分配纹理**计费，换取高频弹层下减少 native create/destroy；未设计异步显存压力下的 Trim。
- **【回归单测源码，未执行】**：增加 Gaussian × 5 + Root × 1 的 6→3→4→6→3→6 原位复用测试，断言 Root TextureView、全部 Gaussian Filter View、全部 Scratch View、当前活动数和 inactive Source 查找；增加容量溢出必须失败且不突变 ResourceSet/Texture 元数据的反例。所有现有 `BackdropBlurTargets` 测试字面量同步补齐活动 Source 数。编译/真实像素未验证，DX11/DX12/VK/OpenGL 的 live-reorder/Resize/Fence 状态仍需用户本地验收。
- **【后续 P0】**：先拿最新构建 Trace 比较 `GPUI retained target layout reused` 的 `active_isolated/allocated_isolated` 与 `recreation required`，并确认 Gaussian 输出在重绑后完整初始化；如初次 3→4→6 仍有新分配属于超过高水位容量，不能把它当回归。再决定隔离纹理过度驻留的回收策略、Filter target dimension-preserving 重新分配、真正独立 RootColorTarget 与 Fence-retired GPU 资源池；未验收 GPU 40% 降幅。

## 2026-10-15：BMCBL 启动器设置内置 GPUI / Nova 详细 Trace 开关

- **起始基线与并发复核**：`733ad2e3cb1dfdfabbb2524b9e349d39d54e2a30`；提交前远程 master 有连续 UI 优化提交；每次提交前用 compare 校验其文件与本轮配置、Settings 子模块、日志及语言文件无交集，并以实际最新 HEAD 为父提交，保留这些 UI 改动。本切片仅增加 BMCBL 应用侧启动器诊断配置与 UI，不更改 GPUI/Nova 的渲染 ABI、提交/Present、物理 Target Owner，不引入环境变量或第二套 Trace 缓存。
- **【代码实现；未编译】**：`src/config/{config.rs,defaults.rs,test.rs}` 在 `launcher` 下增加 `#[serde(default)] gpui_trace: bool`，默认 false；已有 TOML 不含字段时仍正常加载，序列化后开关状态可持久化。设置入口 `设置 → 启动器 → GPUI / Nova 详细 Trace`；`src/ui/views/settings/{state.rs,common.rs,launcher.rs}` 延续现有 ToggleSwitch→SettingsSnapshot→run_io_blocking(update_config) 保存路径。zh-CN/zh-TW/en-US/ja-JP/ko-KR 全量补充开关文案和“重启生效/高频日志有性能及磁盘开销”提示。
- **【日志/Renderer 联动】**：`src/startup.rs` 向现有单一 `init_logging` 转交 `launcher.gpui_trace`；`src/utils/logger.rs` 仅当 Trace 显式为 true 时为 `gpui::diagnostics` 开启 trace 过滤，其余 GPUI 为 debug、网络和其它模块沿用旧级别。普通 Debug 与 Trace 独立；开启 Trace 不自动显示 Debug 窗口。`src/app.rs` 的 RendererOptions 使用 `frame_metrics: bootstrap.gpui_trace_enabled`，同时 Trace 模式也启用已有每 5 秒 GPU Owner/Frame 汇总；关闭后原有默认 frame_metrics=false、info/debug 级别保持不变，省去常态逐帧负担。
- **【可观察日志】**：打开设置开关并完整重启后，`BMCBL/logs/latest.log` 可记录 `GPUI color retention result`、`GPUI retained target layout reused/recreated`、`nova-gfx frame diagnostics` 和 `GPUI AutoStatic submitted`。Trace 只能反映 CPU 侧构建与 GPU 提交规划，不等价于原生 GPU Timestamp。开启前已运行的 renderer 或日志 subscriber 无法动态改变，**必须重启**。
- **【测试与风险】**：增补默认值/TOML 旧配置/启用后的存取与 Debug 开关互不影响测试；增补日志过滤器 4 种组合的 Trace 开关断言。由于当前容器未装 Rust/Cargo/Windows GPU，尚未执行 fmt/check/单测与四后端实时日志验收。发布前在 Windows 依次检查旧配置读取、设置点击自动保存、重启后 True→False 两方向、日志滚动及压缩限额；GPU40% 与 AutoStatic P0 问题仍需数据验证。

## 2026-10-15：最新 DX12 22.6 秒 Trace 证据与 P0-03/04/05 精确定位

- **输入**：用户本次上传的 `latest(20261015-052153).log`，5718 行，约 2.4 MB，0–22.564 秒；`target/debug/BMCBL.exe`、DX12、Windows 11 26100、AMD Radeon 780M、DWM 239.987Hz；启动行 `debug_enabled=true gpui_trace_enabled=true`。此文件为真实**运行日志证据**，并非 Rust fmt/check/test 成功证明；用户未提供同时段系统 GPU 使用率截图/原生 GPU timestamp。Debug Window `window_id=4294967298` 亦被创建，分析时须将其与主窗口 `4294967297` 分离。
- **P0-02 部分验收**：应用开启了 Trace，包含 389 条 `GPUI color retention result`、1412 条 `GPUI AutoStatic submitted`。证明**开启与重启后的有效性**，不能据此证明关开切换、旧 TOML 兼容及日志留存上限均验收成功。
- **P0-04 关键失败**：389/389 `static_prefix_candidate=false`、389/389 `static_prefix_captured=false`；1412/1412 `source_index=None`，且 `auto_static_relocation=Some` 为 0。现有 `refreshed_layers=0` 是**没有建立 AutoStatic**，绝不意味着「已缓存成功并免除 Source Pass」。P0-04 由“待验证”升级成“现有 DX12 负载实测未命中”，不得直接宽松处理 Gaussian/Atlas/复杂 Capture 的正确性保护。
- **P0-05 部分改善但未闭环**：27 条 `GPUI retained target layout reused`，12 条 `recreation required` + 12 条 `layout recreated`。典型重新分配出现在 3/4 变体和 Root `Some(2) ↔ Some(3)` 迁移时，即使旧分配容量 ≥ 新需求；另一些 6→3→4→6 一部分命中已预热复用。必须追查 Filter 配置 Slot 不兼容还是 Source 容量不足，不因 `requested_isolated` 原始 flattened index 个数和实际 Gaussian Source 数混淆就断言是容量不够。记录 `active_isolated` 与 `allocated_isolated`；原本 3→4→6 的初次扩容不应标记回归。
- **额外 GPU 开销证据**：高频期间的 `nova-gfx frame diagnostics` 示例，`draw_step_cache_hit=true` 且 `upload_bytes=136` 时仍有 `gpu_passes=5`、`filter_source_draw_steps=6` 与非零滤镜采样像素。共 211 条详细帧抽样，部分为 6/7 Pass。4 组 5 秒汇总 `frame_requests=85/532/815/418`，`draws=27/292/503/56`；GPU Owner `owner_busy_ratio=11.1%/17.3%/21.4%/19.9%` 是跨运行时刻的**累计比率**，不是 Task Manager GPU40% 或 Shader Timing。Trace 本身对 CPU/IO 有成本。
- **当前这轮代码切片（未编译）**：`renderer/retained_upload.rs` 的 `auto_capture_static_background` 对每条**保持原样的保守准入条件**增加可检索 `GPUI AutoStatic admission rejected: reason=...`（e.g. `gaussian_element_capture_present`、`retained_quad_chunk_present`、`non_quad_or_shadow_static_prefix`、`prefix_not_independently_replayable`），不放宽 Layer/Atlas/Filter 排斥条件；`targets/layout.rs` 的布局重绑失败记录 `GPUI filter layout reuse rejected`，区分 variant/source 容量、root 身份、AutoStatic 身份、`target_slot_mismatch` 与重叠 Texture 槽；`src/utils/logger.rs` 让独立 Trace 开关额外仅启用既有 `bmcbl::ui::render_trace=trace`，使 Dropdown trigger/overlay View 的 render 进入同份日志，不开启通用模块全量 trace。以上均尚待当前环境 Rust 编译验证和新日志核对。
- **下一步验收 / 缺口**：Windows cargo fmt/check、GPUI retained-upload + gaussian_capture tests、logger 四种开关测试；DX12 最新构建重新录制 Dropdown（关闭额外 Debug Window 可做单窗 A/B），统计 reason 分布、相同时段 Variant reuse vs reject 原因、UI `dropdown-trigger`/`dropdown-overlay` rebuild 频次与 `gpu_passes`。待明确准入失败原因为安全不可拆时，走 P0-06 独立 Root Color/Element Filter DAG，不要强行把复杂场景包装成 AutoStatic。

## 2026-10-15：第二份新 DX12 Trace 复核、拒绝原因闭环与 Chrome/Nav 局部诊断（当前 P0 真实证据）

- **用户提供的完整运行文件**：`latest(20261015-053233).log`，9,307 行，最后时间约 24.629 秒；`target/debug/BMCBL.exe`、Windows 11（build 26100）、AMD Radeon 780M、`nova-dx12`、DWM 239.987 Hz；`debug_enabled=true gpui_trace_enabled=true`。没有当前构建的 Git commit ID/原生 GPU Timestamp、任务管理器实测占用率，也尚无 Cargo fmt/check/test 成功输出。用户另启动了调试窗口 `window_id=4294967298`，GPU Owner 统计包括它；单窗 Release + Trace 关闭数据不能用本日志代替。
- **P0-02 / P0-03 可观测性**：`bmcbl::ui::render_trace` 已在新版本输出。完整日志计数：Dropdown Trigger Render=408、Dropdown Overlay Render=35、Chrome Nav Render=618、Chrome Shell=424、Chrome Brand=5、Chrome Controls=5、Chrome Auth=20。**5–10 秒** Nav=362、Shell=361、Trigger=4；**10–15 秒** Trigger=198、Nav=134；**15–20 秒** Trigger=162、Nav=1。二者来自不同交互负载，不能把 5–10 秒归因于 Dropdown；目前 `chrome-nav` 仅报告 `theme_animating=false`，不足以判定是不是 Nav 胶囊或 Label 的真实动画。此轮 `src/ui/main_window/chrome_view.rs` 将 `nav_animating`、`labels_animating`、`active_route` 纳入同一条 opt-in Render Trace，不改变动画策略，**未编译**。
- **P0-04 准入阻碍已确诊**：642 条 `GPUI AutoStatic admission rejected`，其中 428 次 `nested_element_capture`、214 次 `non_quad_or_shadow_static_prefix`；642/642 `static_prefix_candidate=false` 且 `static_prefix_captured=false`；1,462 条 `GPUI AutoStatic submitted` 全部 `source_index=None`。因此不论 `refreshed_layers=0` 数量多少，**本负载根本没有 AutoStatic 纹理可复用**。这说明现有只容 Quad/Shadow 的静态前缀算法不覆盖正常带文字/图像的页面，且嵌套 Capture 未获独立内容身份；不证明可以安全删除拒绝保护。本轮 `renderer/retained_upload.rs` 在非 Quad/Shadow 静态前缀拒绝时增加 `GPUI AutoStatic prefix blocker: kind=... operation_index=... static_prefix=...`，精确区分 Mono/Poly Sprite、Path、Underline、NativeSurface、Backdrop Blur 等来源，**不放宽准入**，尚未实机验证。
- **P0-05 资源重建分类**：布局复用 8 次，重建 11 次，`reuse rejected` 共 9 次：`variant_capacity` 5、`isolated_source_capacity` 1、`target_slot_mismatch` 3。其余 2 次重建是初始窗口 Target 创建（两窗口）；其中 3 次 mismatch 分别在约 14.908、17.046、23.262 秒：`new_slot=1 radius=0 levels=1 old_active=4 old_allocated=4 pinned=Some((3,2))`。这是**已分配足量目标**情况下 Root 物理固定但其他零半径 Capture index/member range 重排导致的候选失败；`BackdropBlurConfig::same_target_slot` 检查 `member_first/member_last`，并非只检查 GPU Texture 规格。若允许按物理兼容度复用，必须先在 `FilterRegistry` 证明非 Root 内容版本失效、原位 ResourceSet 同步、Fence 读者退休与 Render Graph 重绑，做测试后才可改，不能把旧纹理值原样标记可信。Root 缩小后重建丢失 Scratch 高水位，下一帧再次增大导致 `variant_capacity`，需把「初次容量增长」与「丢弃保留容量后的增长」区分统计。
- **真实提交/抽样时序**：`nova-gfx copy attribution` 1,462 条；其中 `nova-gfx frame diagnostics` 403 条（**是详细抽样/采样条件下日志，并非全部提交帧**），GPU Pass：2×264、1×98、5×22、7×10、其余 9 条；`element_blur_refresh=true` 为 307/403，`draw_step_cache_hit=false` 为 382/403（后者仅适用于此抽样集合）。`frame diagnostics (5s delta)` 四个采样点 request/draw/present：5.112s=223/137/99、10.114s=847/705/614、15.133s=423/172/148、20.147s=351/28/44。`owner_busy_ratio` 同时刻约 16.6%、23.3%、21.3%、19.3%，为**生命周期统计**而非原生 GPU Shader busy。最后 5 秒出现 351 Requests 但 28 Draw 与 44 Present，需要结合原生 WSI / 非可见状态判定有效性；Trace、Debug 构建和双窗口造成额外负载。
- **后续 P0 执行策略**：先 Windows 本地 Cargo fmt/check/相关 GPUI 单测，避免将本次源级修改误报编译通过；新构建依次收集 `prefix blocker kind` 和 `chrome-nav nav_animating/labels_animating`，按时间段做 Chrome Navigation 单独与公共 Dropdown 单独 A/B。优先针对 `non_quad_or_shadow_static_prefix` 建立**具有 Atlas Generation / Content Version 的独立 Paint Chunk Source** 而非放宽过滤器；对真实 `nested_element_capture` 另建 Source 层次依赖，不能在未有 Filter DAG/资源读写证明前强行 Capture。然后才做 4→3→4 zero-radius 物理兼容重绑测试、保留 Root 和独立 Scratch 高水位的安全复用；如仅需 UI Render 减量，依据更新后的 Nav 动画字段定位上游无关通知。P0-07 的 40% Task Manager GPU 仍未被本日志直接测得。

## 2026-10-15：P0-05 混合 Gaussian/零半径 Capture 物理重绑定（源码切片，未编译）

- **起始 HEAD**：`7708b0facc2632c8b83830a86b0772da60f3dee3`，依据用户新 DX12 日志中 3 次 `reason=target_slot_mismatch new_slot=1 radius=0 levels=1 old_active=4 old_allocated=4 pinned=Some((3,2))`。是「物理容量存在但扁平 Capture 索引发生变化」的确定场景，不是 GPU 显存不足或已测得的 GPU 时间瓶颈。
- **单一职责修改**：`crates/gpui/src/platform/nova/targets/layout.rs` 在现有唯一 `BackdropBlurTargets::reuse_layout` 内实现**全部预检成功后原位交换**，不创建第二份 GPUI Scene、GPU Texture Pool 或缓存所有者。首先固定已存在的 Root Color / 指纹校验 AutoStatic 实际物理目标，再预留**所有严格匹配**（原 source_group/downsample/levels/member range/overlap），最后仅在**旧 Root 与新 Root 都存在且被同一原生纹理固定**时，为无法严格匹配的 `radius=0.0` 捕获分配尚未使用、`levels.len()==1` 且 byte_size/frame ResourceSet 数量与 Root 相同的全分辨率纹理。两级 Gaussian、Root source topology、孤立 Source 容量、目标容量仍严格验证；物理 TextureView/ResourceSet 随 `BackdropBlurVariantTargets` 一起移动、不创建新 native allocation。
- **内容正确性门**：既有 `renderer.rs::ensure_backdrop_blur_targets` 成功复用后检查 Root 的原生 TextureView 与 `root_color_reuse_eligible`，且 `FilterRegistry::remap_identity_capture_sources` 仅能按照**匹配 Native View + Capture Layer ID + 后续 source_unchanged**延续其他元素内容。不能证明身份的滤镜必须刷新；全局失效分支保留 `force_full_backdrop_blur_refresh` 与 `invalidate_backdrop_blur_cache`。异步 `renderer/submission.rs::frame_submission_is_ready` 在目标不相容且有 in-flight GPU 读者时延后重排，同步提交仍须等待 Fence。单元测试不能证明 DX12/Vulkan 原生资源同步安全。
- **新增非 GPU 单测源码**：`crates/gpui/src/platform/nova/tests.rs` 的 `mixed_gaussian_and_zero_radius_popup_reindex_preserves_root_and_scratch` 构造 Gaussian0/Gaussian1/Capture2/Root3 → Gaussian0/Capture1/Root2 → 原布局，验证固定 Root View、Gaussian View、原空闲一层纹理、保留全部 Scratch、活动索引变更和 `Vec::as_ptr` 不变；`zero_radius_fallback_never_steals_gaussian_or_mutates_failed_layout` 验证没有可用单层目标、或没有固定 Root 时不会挪用两级 Gaussian，失败完全不修改物理 View、活动数及 Root 索引。
- **可观测性**：新 Trace 仅当回退真正被选用时输出 `GPUI zero-radius mixed layout permutation: rebound=...`，沿用 `GPUI retained target layout reused` 与 `GPUI filter layout reuse rejected`。在**经过首次容量预热**的 4→3→4 操作下预期以前的 `target_slot_mismatch` 重建可变为 reuse；必须检查 `target_bytes`、`GPU Owner`、实际 DX12 Task Manager GPU、`gpu_passes` 及正确像素，不能从零拷贝身份映射直接推断 GPU 使用率下降。
- **验收未完成**：本环境无 `rustc`/`cargo`/Windows 四后端设备，尚未运行 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib mixed_gaussian_and_zero_radius_popup_reindex`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib zero_radius_fallback_never_steals_gaussian`。真实混合 Gaussian/Root、Dynamic Popup 的重绑定后 Blur 采样、窗口 Resize、DComp 透明与 Fence 退休、DX11/DX12/Vulkan/GL 应按 P0-05 的完整退出标准进行实测。若新日志是其他 reason，应继续分类，不扩大匹配权限。
- **下一切片**：先获取 Windows Rust 测试/新日志并验证上述 3 次 mismatch 是否消除；如果仍有 `nested_element_capture` / `non_quad_or_shadow_static_prefix` 阻止 AutoStatic，推进 P0-04「Atlas Sprite / Filter DAG 内容身份」设计与 P0-08「Chrome Nav 动画真正触发者」的 Trace 因果定位，**不直接关闭保护或伪造缓存命中**。

## 2026-10-15：用户同操作视频+DX12 Trace，定位 Presentation Damage→Element Filter 重复刷新并实施第一纵向切片（源码未编译）

- **同步证据**：用户上传 `QQ20261015-134716-HD.mp4`（约 24.78 秒，59 fps）及 `latest(20261015-054831).log`（运行到 55.91 秒，15,340 行）。视频中任务管理器明确显示 BMCBL 的 GPU 1 · 3D 占用约 46.3%～48.3%（不是 Nova GPU Owner CPU busy），当前操作是设置页启动器选项的语言 Dropdown 多次展开/回收；视频比完整程序日志更晚开始，操作活动大体落在日志 31～56 秒，不能逐帧按 0 秒对齐。Windows 11、AMD Radeon 780M、DX12、约 240Hz，用户当前运行的是 `target/debug/BMCBL.exe` + `debug_enabled=true gpui_trace_enabled=true`，并存在第二个 GPUI Debug Window，缺少关闭 Trace 的 Release 性能对照及 DX12 原生 GPU Timestamp。**这份日志没有** `GPUI AutoStatic prefix blocker`、`nav_animating=`、`GPUI zero-radius mixed layout permutation`，不能用来验收其后提交的最新版渲染修复。
- **P0-07 真实 5s 区间**：40～45s requests/draw/retained_presents/blur_frames=506/248/270/227；45～50s=487/61/450/420；50～55s=725/74/662/635。45～55s 对应 GPU `copy attribution` 共 1,113 次，其中**1,113/1,113** 均 `element_blur_refresh=true`、`dirty_full=true`，滤镜相关 Pass 累计 6,436 次，主合成 1,113 次，源区域平均约 109 万像素/次，而部分 Gaussian 单轴输出仅约 400 像素。Atlas Upload 与 retained present copy 在这段较少或为零，因此“只减 CPU 上传/Taffy”不能解释视频高 GPU。注意 Source Pixels 是 Scissor/区域估算，**不是 GPU Fragment 时间**；不能直接断言哪个 Shader 花费最多。
- **源码核对**：`src/ui/components/dropdown.rs::render_overlay` 使用 `with_visual_animation(AnimationProperty::vertical_reveal)`；`window/frame_lifecycle.rs::run_animation_engine_frame` 和 `scene/display_list.rs::gpu_sample_damage` 将动画样本转换为窗口损伤；`renderer/present.rs::dirty_element_blur_indices` 根据 `FilterRegistry::source_unchanged` 判定源纹理是否需刷新，`prepare_element_blur_layers` 做离屏绘制。当真正嵌套子 Capture 的像素随动画改变时，父 Capture 不可因为「只是动画」而跳过重绘。
- **【本轮真实优化，待验收】**：`crates/gpui/src/scene/display_list.rs::gpu_sample_damage` 对已编码的 `ClipReveal`，仅在新旧两次 GPU 裁剪矩形**共享固定边、单轴移动、边界有效且有限**时生成新增/隐藏像素带，而不再无条件把整块动画图元加入 Damage；按当前 `SceneAnimationValue` 实际 sample 计算，然后与图元/捕获的可见 Bounds 相交。多轴移动、重定向不规则矩形、非法数字/反向裁剪、缺少完整旧新采样仍走原有 swept-bounds 保守路径。没有改动 GPU Texture、Shader、Filter 内容缓存版本、动画/输入语义，也没有新增 RenderTree / 第二套 GPU 缓存。这个缩小的是**受影响像素范围**，并不绕过嵌套父 Source 的真内容依赖。
- **【定位增强】**：`crates/gpui/src/platform/nova/renderer/filters.rs::source_unchanged` 在 opt-in GPUI Trace 中给每次不复用的 Layer 标注 `GPUI filter source invalidated: index=... reason=...`，分类 `missing_presented_content_version`、`element_identity_or_geometry`、`nested_paint_or_animation_changed`、`sampled_child_pixels_changed`、`capture_source_kind_changed`，不改变是否返回 true 的事实约束。区分 Root/Child Capture 的真实刷新与窗口 Damage 拓扑扩大，为 P0-06 DAG 重构提供可验证原因。
- **【新增测试源码】**：`crates/gpui/src/scene/tests.rs` 覆盖固定上/下/左/右边沿的变化带、相同 sample 空损伤、不规则重定向/NaN/反向坐标回退、与输出 Bounds 的交集，以及真实 `Scene::gpu_sample_damage` 对两个 ClipReveal 动画采样生成 40px 高变化带。不绕过 `retained_filter_source_matches` 对**嵌套动态孩子改变像素**的现有保守保护。**当前环境未提供 rustc/cargo；编译/格式化/单测及 Windows 四后端实机均未完成。**
- **验收门槛**：本地 `cargo fmt --all -- --check`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib clip_reveal`、`cargo check --workspace --no-default-features`；新构建仍用同样的设置页 Dropdown 视频与日志，按同一 window_id 记录新 `Filter source invalidated reason` 统计、`dirty_full` 比例、Source Scissor、每帧离屏 Filter Pass、重建/复用 Target、GPU 实际任务管理器占用；最后关闭 Trace + 使用 Release 做 240Hz 10 秒 A/B。若绝大多数父 Source 是 `sampled_child_pixels_changed`，优先 P0-06 **独立 Root Color 与嵌套 Element Capture DAG**，而不是误把它标记可复用、强行跳过 Offscreen Pass。P0-05 上一轮零半径物理回退也仍需同一机器编译与四后端验证。

## 2026-10-15：P0-08 直接根因修复——Presentation 动画不再冒充 Resize 全窗口损伤（源码未编译）

- **本轮依据**：用户最新 `latest(20261015-080737).log` 的 DX12 运行到 11.286 秒、3,611 行。5.0～11.286s 共 468 条 `nova-gfx copy attribution`，468 条 `element_blur_refresh=true`、461 条 `dirty_mode=FullRedraw`（98.5%）、413 条 `element_blur_layers=4`/6 个 Blur Pass；Retained Present Copy Region 461 次为零。另有 4 次 `GPUI zero-radius mixed layout permutation`，未观察到旧 `target_slot_mismatch`，但 AutoStatic 捕获仍为 0。34 次 Filter Source 拒绝均为 `nested_paint_or_animation_changed same_scene=false old_coordinates_valid=true new_coordinates_valid=true`，并不足以解释 468 次重绘。仅凭 DX12 Owner 日志不能宣称实际 GPU 利用率减少。
- **确认的代码级直接根因**：`crates/gpui/src/platform/nova/surface_plan.rs::requires_full_drawable_damage` 除了 Live Resize / Viewport Changed，竟还将 **Retained Composite Layer + `!packet.scene.blurs.is_empty()` + GPU 动画采样/Active Timeline** 判为整个 Drawable 失效。`renderer.rs::draw_frame` 和 `::render` 两条入口都在已完成 `reconcile_gpu_layer_animation_damage(packet)` 的**精确 Scene GPU-sample Damage** 后，调用 `force_full_drawable_damage_after_resize`，将 `packet.dirty_region=full(viewport)`、`partial_present_mode=FullRedraw`、`force_full_backdrop_blur_refresh=true`。这会不必要地把普通 ClipReveal/Element 捕获动画提升为全窗口 Filter Source 强制刷新，抹掉上一轮 ClipReveal 像素带优化和局部 Scissor 的收益。此次定位比「AnimationEngine 粗 Bounds 双路径」假设有更直接、可执行的源码证据。
- **【本轮修复】**：仅保留 `requires_full_drawable_damage(live_resize_active, viewport_changed)` 两种**真正失去旧 Drawable 尺寸一致性证明**的失效原因，删除「有嵌套 Capture 且动画活跃就强制全窗口 Damage」条件；修改 `renderer.rs` 的两个调用点。**不禁用**真正 Resize 后整窗口完整刷新，也不禁用 `resolve_surface_packet` 对不支持 Native Partial Present 的 Swapchain 选择 `PartialPresentMode::FullRedraw`。Swapchain 全画面提交只是 WSI 限制，**不等于**所有 Filter Source/Gaussian 纹理均需要刷新；已经失效的 Scene/Atlas/Root/Element Capture 仍由 `FilterRegistry::source_unchanged`、`reconcile_gpu_layer_animation_damage` 和自身像素版本比较驱动必要的重绘。
- **【回归测试源码】**：`crates/gpui/src/platform/nova/tests.rs` 调整 `only_real_resize_requires_full_drawable_damage`，验证真实 Live Resize / Viewport Change 仍标记 full；新增 `nested_element_capture_animation_keeps_local_damage_even_with_full_swapchain_present`，构建真正嵌套 Capture 的 Scene，激活 Presentation 动画并保留小范围损伤，要求即使 Native Swapchain 只支持整帧 Present，`DirtyRegion`、Filter 强制刷新标志仍不被改为 Full，随后真实 Resize 仍完整刷新。
- **【验证 Trace】**：`renderer.rs` 两个 GPU 提交入口在精确 Damage Reconcile 之后增加 opt-in `GPUI surface damage gate: source=active/direct ... dirty_full=... dirty_area=... force_filter_refresh=...`，用于和同一帧 `nova-gfx copy attribution`、`GPUI filter source invalidated` 交叉检查，不额外创建 UI 动画调度。不可把仅出现日志当作修复性能已成功的证据。
- **下一步**：先 Windows `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib only_real_resize_requires_full_drawable_damage`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib nested_element_capture_animation_keeps_local_damage`；本环境未配置 Cargo/Rustc，尚未执行。再用**最新 master** 单窗口（最好关闭额外 Debug Window）、同设置页语言 Dropdown 5～10 秒录制，按 main window_id 对比 `dirty_full` 比例、`blur_render_passes`、`blur_source_pixels`、`root_color_preserved`、Target 分配和 Release Trace-off 的 Windows Task Manager GPU。若 Full 仍来自粗粒度 `tick.dirty_bounds` 或 WSI Resize，才推进 P0-08 的下一环，**不能无条件屏蔽真正的 UI/外部纹理损伤**。P0-06 RootColor 独立所有权和 P0-04 含字体/嵌套捕获的真正 AutoStatic 仍未完成。

## 2026-10-15：P0-06 局部所有权切片——Root Color 独占资源证明 + Filter 逐层损伤样本（待 Windows 验证）

- **基线**：`2e5d2f6c5f7cb229123473eb2a6f9b02f78ffb88`。上一轮修复错误的“任何嵌套 Capture 动画都触发 Resize 全帧 Dirty + 全 Filter 强刷”，本轮不重复或撤销；真实窗口 resize、WSI/full Present、Frame Fence 仍遵循既有路径。
- **P0-06 Root Color 身份准入**：`crates/gpui/src/platform/nova/targets.rs::can_preserve_root_color_capture` 原先检查 `isolated_sources.is_empty()` 和**全部（含闲置 Scratch）** `variants[*].radius()==0`，导致关闭弹层后保留物理容量的 Gaussian/隔离 Source 也错误否决独立 Root Color 内容驻留。现在只允许**当前处于 active 前缀内**的 Root 是**一层、零半径、独占 Native TextureView**，且其他**活跃的** Filter Variant 仍为零半径、无全窗口共用 Source；同时检查**所有已分配的** Variant 与孤立 Source 没有与 Root View 物理别名。空闲 Scratch/已关闭 Gaussian 不改变 Root 自身身份，若 Root View/格式失配或共用 Source，保守拒绝；`renderer.rs::root_color_reuse_eligible` 的旧 Layout Valid/有已提交 Root 内容版本、当前 Scene 无 Root Backdrop、当前配置全零半径等进一步限制保持不变。这不是绕过 Filter DAG 的半径/坐标/遮罩变动。
- **P0-06/07 真正 Filter Source 像素版本修复**：`crates/gpui/src/platform/nova/renderer/filters.rs::local_source_damage` 之前必须 `Arc::ptr_eq(current.content, old.content)`，即使 GPUI 发生一次结构等价的新 Scene commit 也放弃 **GPU 局部损伤**，强制整层离屏 Source；同时用 `last_gpu_animation_values` 这个**全局最后一帧动画样本**与最新值比较，当该 Filter 上一帧未刷新时，可能遗漏从其真正 GPU 纹理版本到当前帧的损伤。现在改用 `RetainedFilterSource::animation_values`（仅在该层实际成功刷新后更新的快照）作为**旧纹理源的确切样本**，并用已有的 `Scene::retained_filter_source_matches(old_scene, &[], &[])` 递归验证 Paint 操作和嵌套 Blur 子 Scene 的静态视觉等价性；只有确认静态内容一致、Layer/Bounds/Mask/Radius/坐标有效、且有实际 animation sample damage 的情况才返回局部 Damage。真正静态绘制/嵌套子内容变化、缺版本、无等价证明仍**返回 None → 完整源刷新**；不放宽 Cache 有效性或跳过 Gaussian Pass。仅在老、新 Arc 不同而成功证明局部等价时输出 opt-in `GPUI isolated source partial damage reused across equivalent Scene revisions`。
- **测试**：在现有 `identity_compositor_preserves_only_unchanged_immutable_source_geometry` 基础上，修改“等价 Arc 重建拒绝局部损伤”的历史预期；新增 `isolated_source_uses_last_resident_samples_not_last_submitted_frame`，模拟纹理停留在 sample 0.2 而全局 GPU 已采样到 0.7，检查新 Scene 完全等价时仍按原纹理版本做局部 Source Damage，真正 Quad 几何变更则必须拒绝。扩展 `warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`：6→4 预热后 Gaussian/isolated spare 不阻止独立 Root 复用，活跃 Gaussian 或与 Root View 别名仍禁止。
- **范围和待办**：本次仅 2 个 GPUI 源码文件、1 个现有单测文件与本文，不创建第二份 Root 缓存，不修改 Vulkan/DX11/12/OpenGL 后端 ABI，**没有把 Root 的 Native Texture 资源分配从零半径 Filter Target Chain 中物理抽出**；P0-06 的 `retained_root_blur` 永久语义删除、独立 RootColor 分配与 Fence 生命周期、完整的 Filter DAG 仍待后续切片。主窗口 `retained_root=false` 时该 Root 专项不会触发；Source 局部损伤还需要 `draw_steps.rs` 的 `direct_composite && retained_root_clear_quad && !force_full` 前提，因此不能把代码路径覆盖率或 GPU 使用率改善视为已验证。
- **下一步本地命令**：`cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib isolated_source_uses_last_resident_samples`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets` 及既有 Filter/retained tests；DX12 同版本 Trace 对比真实 Dropdown 5～10s 的 `dirty_full`、`GPUI surface damage gate`、`GPUI isolated source partial damage reused`、Root View/content_version、Filter Source Pass 和 Task Manager GPU，之后 DX11/Vulkan/GL 画面对照及 Fence/Resize。**当前环境无 Cargo/Windows GPU，尚未编译/运行测试，不许将预测结果记为实测收益。**

## 2026-10-15：P0-06 Root Color 驻留元数据统一为 Native Texture + View + 成功提交内容版本（待本机验收）

- **本轮起点**：`master=7d1cbdd367f4ac8cd8be45fb257002aa829f3914`。审查 `BackdropBlurTargets::retained_root_variant`、`FilterRegistry::root_color_version`、`Renderer::root_color_reuse_eligible` / `ensure_backdrop_blur_targets` 和成功提交后 `present.rs::record_root_color_present` 的生产/消费。上轮已消除了普通动画误触发 Nova Resize FullRedraw 的条件，并更正按 Filter 自身的已刷新动画样本计算局部损伤；两者均尚未得到此 HEAD 的 Windows 验收。
- **真实架构问题**：`root_color_version: Option<TextureContentVersion>` 原先只是裸版本号，GPU 纹理身份仅由 `ensure_backdrop_blur_targets` 的临时 `prior_root_view` 对比保障；逻辑滤镜重排和 Present 内容版本分别在不同模块判定。现在 `crates/gpui/src/platform/nova/renderer/filters.rs` 用唯一的 `RootColorResidency { texture: TextureId, view: TextureViewId, content_version: TextureContentVersion }` **替换**裸版本字段；没有平行保留旧 `root_color_version`、也没有创建另一张缓存 Texture。该元数据仍隶属于当前窗口唯一的 `FilterRegistry`，物理分配仍在 `BackdropBlurTargets`。
- **身份证明单源**：`targets.rs::root_color_texture_identity` 只在原 Native Root 所属目标位于活动前缀、`radius==0.0`、恰有一个目标层时返回物理 `(TextureId,TextureViewId)`；`can_preserve_root_color_capture` 复用此方法继续检查 Root 与其它 Variant/Isolated 的纹理物理别名，旧保守的活跃非零 Gaussian 和 root source 排斥仍保留。`FilterRegistry::root_color_resident_on(targets)` 要求已提交驻留状态的物理 Texture **及** View 全相等，不再只使用一个 View 或某个扁平索引对 Root 有效性作判断。
- **生命周期单源**：`renderer/present.rs` 仅在**成功 native 提交**后从实际目标取得 Root 原生身份并调用 `record_root_color_present(native_root, refreshed)`；刷新过则将新 `TextureContentVersion` 绑定给当前物理 Texture/View，没刷新的纯合成帧只在同一物理资源时延续版本，否则清空旧版本；Root 退出、重建/无 Native 目标、`FilterRegistry::invalidate` 均清除元数据。`renderer.rs` 删除临时 `prior_root_view` 判断，将重排前后的复用证明统一委托给 `root_color_resident_on`；`native_root` 若新分配但还未实际绘制，不能继承旧版本。
- **验收测试源码**：在 `renderer/filters.rs` 既有 Root Version 单测中增加物理纹理更换但不刷新时清除 Version、替换后成功刷新才推进 Version、Root 离场/全局 invalidate 必须清除；`crates/gpui/src/platform/nova/tests.rs::warmed_zero_radius_popup_targets_rebind_without_reallocating_textures` 扩展 6→4→5→6→4 Root 逻辑位次变更、同一物理 Texture/View 的 Version 持续不变，并验证 TextureId 更换而 View 未变、View 更换但 Texture 未变均拒绝；原有 inactive Scratch/Gaussian 物理别名保护保留。Trace 加入 `GPUI root color residency: window=... native_texture=... native_view=... content_version=... refreshed=... retained_root=...`，仅对 opt-in GPUI Trace 记录，不在默认静止帧增加额外日志。
- **本轮明确不做**：尚未创建独立的 `RootColorTarget` GPU 纹理分配/retirement 所有者，也未删除 `FrameUpload.retained_root_blur` 或旧 Zero-Radius Capture 编码。实现 P0-06 **完整完成**前还需把 Root 像素从 Filter Variant 物理所有权迁出、同步 native ResourceSet / GPU Fence / Resize 世代 / Render Graph 别名和所有消费者，并经 DX11/DX12/Vulkan/OpenGL 真实像素回归；避免另起一套平行 Root 缓存。本文与代码中的基线 SHA 是历史事实，恢复仍先刷新远程 HEAD。
- **尚无测试通过证据**：当前对话容器无 `cargo`、`rustc`，代码级 Git Blob / Diff 可以检查，但无法执行 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_version_survives_only_texture_preserving_topology_rebind`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`，也无 Windows 后端 GPU 真实验证。**不得推断 GPU40% 已改善**。用更新二进制分别跑语言 Dropdown 高频/静止/Blur on-off/Resize/多窗口、独立 Debug Window A/B，检验窗口级 Root Native ID、版本稳定、Root/Filter Pass 和视频真实 GPU 指标；在可验证之前，P0-06 仅记“源码已提交、待本机验证，真正分配迁移未完成”。

## 2026-10-15：P0-06 Root Color Native Texture/View/ResourceSet 独立退休职责——首条 GPU 资源迁移（未编译）

- **来源与边界**：在最新 `master=69d0e457b92c42be92107dbb19a3674e790dc41d` 上核对 `BackdropBlurTargets`、`BackdropBlurTargetDescriptor`、`renderer/surface_lifecycle.rs`、`renderer/draw_steps.rs`、`platform/nova/composite_target.rs`、`renderer/destroy.rs`，确认旧实现中 Root 的零半径 Variant 同时承担 Root 的 Native Texture 创建、GPU View、每帧 ResourceSet 引用与最后销毁职责。此前 `RootColorResidency` 只拥有已提交元数据，不拥有 GPU allocation。
- **本轮真实物理资源切分**：`targets.rs` 新增 **唯一 Native 退休职责持有者** `RootColorTarget { target, pass_resource_sets, composite_resource_sets }`（不建立新缓存，不额外创建 GPU 纹理），在 `BackdropBlurTargets` 记录 `root_color_target`。Target Descriptor 显式携带 `root_color_capture_index`；创建链之前从当前配置确定唯一 Root 逻辑 Slot，**先独立创建 Root 的全分辨率 Native Texture/View**，创建 Variant 时只有 Root Slot 借用这张 Texture/View；其他零半径 Element/Gaussian 仍按原方法创建其自身 Scratch。逻辑 Variant 的 Level/ResourceSet/Draw Pass 接口暂时保持，因此没有新增合成拷贝或第二张 Root Texture。目标是最终迁移所有消费者后能把 RootColorTarget **从 BackdropBlurTargets 直接移动到单独 GPU 所有者**。
- **Native 退役职责单一**：创建链时从 Root Slot 将两类 Native ResourceSet ID 归属 `RootColorTarget`，逻辑 Variant 只引用它们。`destroy_backdrop_blur_target_chain` 在遍历各 Variant 时**跳过属于 RootColorTarget 的 Composite/Pass ResourceSet 及其 Texture/View 销毁**；其他 Filter/Scratch 仍由 Variant 正常退休。最后由 RootColorTarget **唯一退休 Root 的两类 ResourceSet，再释放 Root Texture/View**。GPU allocation 和 Set 不重复创建。这些步骤都发生在现有 GPU submission Fence 的安全路径之后，不额外引入 driver wait；共享 TextureId/ViewId 不能在 Variant 和 Root 分别销毁。 `root_color_texture_identity()` 进一步核对 Root Variant 借用的 Texture/View 与真正 Native owner 完全一致，拒绝把重排后的另一张物理纹理冒充 Root Cache。
- **Binding 更新一致性**：`renderer/buffer_capacity/bindings.rs::rebind_slot` 在整批新资源集创建并原子发布后，调用 `BackdropBlurTargets::refresh_root_resource_sets`，将更新后的 Pass/Composite Set ID 从借用 Variant 同步回唯一 Root Owner，再由原绑定事务销毁旧 IDs；若 ResourceSet 创建失败，原来的目标仍保持不变。**回归源码**：`crates/gpui/src/platform/nova/tests.rs` 所有 `BackdropBlurTargets` 无设备测试夹具显式初始化 `root_color_target: None`；对 Root 6→4→5→6→4 的用例显式构造 RootColorTarget 物理所有权，检查 Native Owner 与借用 Level Identity/PassSet/CompositeSet 一致，经过 6→4→5→6→4 重排保持资源集归属；模拟 frame-slot binding 增长后成功回写新 Set IDs，改换借用 Level TextureId 后必须使身份和 Root 的既有 Residency Proof 失效；原有零半径别名、闲置 Gaussian、物理 View 变化等测试继续有效。无真实设备的测试只能验证元数据和重排，不等于确认所有原生后端的 GPU Descriptor/Fence 生命周期。
- **Root 角色变化与 Fence 预检**：新增 `root_capture_is_compatible` 检查当前 Root Role 与 Native Owner，`renderer.rs::ensure_backdrop_blur_targets` 只有 Filter Layout/Root Role 都相容才直接复用；`renderer/submission.rs::frame_submission_is_ready` 也使用相同条件，防止异步 GPU 读者在 Root Role 拓扑变化时被绕开。若之前从未分配 Root 而现在要出现 Root，则禁止直接将普通 Scratch Texture 冒充 Root，先执行 Fence 安全的目标链重建。已存在 Root Native Owner 但 Root 曾经暂时关闭时，`targets/layout.rs` 从当前所有物理 Variant（含 Spare）按 Texture/View 找回并固定真实 Root Owner，再做重排；绝不能把别的 Texture 重新贴 Root 标签。新增“Root 开→关→开、内容版本清除/重新提交、无 Native Root 不能假装命中”无设备回归。此项仍属**源码静态实现**，Native Fence/WSI 尚未实机验证。
- **仍未完成/需要真实 GPU 验收**：这次**没有**把 RootColorTarget 从 `BackdropBlurTargets` 迁移到 Renderer / 独立 Window GPU Owner 字段，已把原生 ResourceSet 的**退休职责**转给 RootColorTarget，但创建/重绑仍通过逻辑 Variant 进行，仍需要把 `append_blur_bindings` / GPU Draw 直接改成引用 RootColorTarget 并删除重复的借用 Set 元数据；用户要求的独立 **Window GPU Owner/Fence 退休队列、Render Graph Root Pass 资源引用以及删除 `FrameUpload.retained_root_blur` 语义**仍属于 P0-06 继续任务，不能标成“完成”。Root 当前生命周期仍依赖唯一 Window Owner 对整条 Chain 的 Fence 预检与 Resize 资源退役；在没有 Windows DX11/DX12/VK/GL 实机画面与 Fence 压测之前，不能解除这道门。以后必须迁移和**删除**逻辑 Slot 借用而非添加第二份 Root 缓存。
- **验证顺序**：本地 Windows `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_version_survives_only_texture_preserving_topology_rebind`，并回归滤镜/高频 Dropdown、Root Physical View、DComp/透明、频繁关闭窗口和 Resize。当前容器没有 Rust/Cargo，**这些测试尚未执行，提交代码不代表通过测试，亦不代表 GPU40–50% 已下降**。下一个切片应将 Root 直接从 Window GPU Owner 获取、让 `append_blur_bindings`/DrawSteps 不再引用逻辑借用的 Root Set，并独立按 Fence 退休 Root；先确认上述提交的原生四后端资源生命周期正确，再继续删除 `retained_root_blur` 语义。

## 2026-10-15：P0-06 Root ResourceSet 唯一生产方与 GPU Graph 原生消费端迁移（源码待验）

- **基线**：本次编辑前重新读取远程 `master=d7072c8ce9e1fa4eef1d89abd24e95c15522fc11`，其父为原交接 `69d0e457b92c42be92107dbb19a3674e790dc41d`；上一轮候选已经实际提交，禁止重复迁入。变更涉及 `targets.rs`、`renderer/buffer_capacity/bindings.rs`、`composite_target.rs`、`tests.rs` 及本文；不改 Filter Shader/Native Backend ABI。
- **原生创建与销毁权威**：Root 的全尺寸 Texture/View、Pass ResourceSet 与 Composite ResourceSet 全部由 `RootColorTarget` 直接创建并记录；对应零半径 `BackdropBlurVariantTargets` 只借用其句柄，不再以 Variant 为新 ResourceSet 的创建者。Root 资源依然通过唯一的 `destroy_backdrop_blur_target_chain` Fence 安全入口退休，先释放采样 ResourceSet，再释放纹理 View/Texture；没有第二份 Root 纹理。
- **扩容重绑定**：`append_blur_bindings` 跳过借用 Root 物理 Texture 的 Variant，改为**仅对 `RootColorTarget` 每帧绑定一次**。整批新 Set 创建成功并提交后，`sync_root_borrowed_resource_sets` 按真实 Texture/View 将 Root Owner 的 IDs 传播给逻辑 Variant，随后旧 Set 才被释放。创建任一个新 Set 失败时仍沿原回滚路径销毁新建 Set，不更新任何源句柄。Root 离场而物理 Texture 进入 spare slot 时也保留唯一绑定生产方。
- **Render Graph 读取权威**：`BackdropBlurTargets::resource_set_for_config`（合成采样）和 `direct_composite_target`（零半径离屏源）识别 Root 原生 Texture/View 并直接读取其 Composite/Pass Set，不把 Variant 的借用镜像当作权威；`root_color_texture_identity` 无 Root 原生所有者时必须返回 `None`，禁止 Filter scratch 冒充 Root 颜色驻留。
- **回归测试源码**：扩展 `warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`，确认无 Owner 时 Root native identity 无效；在模拟新 FrameSlot Set 替换后，刻意令逻辑 Variant 保留旧 IDs，先断言两个 Graph 消费端仍选用 Native 新 IDs，再同步借用 IDs 并验证 Root 纹理与内容版本稳定。旧 Gaussian/zero-radius/spare slot 测试未删除。
- **剩余边界和风险**：`RootColorTarget` 仍挂在 `BackdropBlurTargets` 内，借用 Variant 的向量镜像、`FrameUpload.retained_root_blur` 索引、Filter DAG Root Pass 路由与整条链共享的 GPU Fence/Resize 退役队列**尚未删除/独立化**。下个纵切必须将 Root 迁入 Window/GPU Owner 的独立资源槽及 per-submission 安全退休，并按 Renderer Graph 生产者/消费者清除冗余 Root Variant 语义。特别注意多 FrameSlot、GPU in-flight、异步取消、DX12 窗口关闭、DX11 Resize、VK Alias Barrier、OpenGL 资源回收。禁止引入第二个 Root Cache。
- **验证状态**：当前聊天容器中未检测到 `cargo`/`rustc`/`rustfmt`，未执行 Rust 编译、Cargo Test 或四后端实机像素/时间戳回归；没有任何证据支持称 GPU40%～50% 已下降。要求 Windows 执行 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_version_survives_only_texture_preserving_topology_rebind`；补齐 6→4→6、Popup 动画、Blur 开关、Resize、FrameSlot 大小变化、销毁前 Fence 的四后端日志。
 
## 2026-10-15：P0-06 删除 Root Filter Variant 的 ResourceSet 镜像，保留唯一原生所有权（源码待验）

- **编辑起点**：重新读取远程 `master=0534ec6151ec96604f39e0fce266dfb88c9b59e3`、本 TODO 与 `targets.rs`、`targets/layout.rs`、`renderer/buffer_capacity/bindings.rs`、`composite_target.rs`、`draw.rs`、原有 Root 物理身份测试。当前仍缺本地 Rust 工具链与 DX11/DX12/Vulkan/OpenGL 原生环境。
- **完成的唯一权威切片**：原先 `RootColorTarget` 拥有 Pass/Composite ResourceSet，但零半径 `BackdropBlurVariantTargets` 仍持有两份克隆的 `Vec<ResourceSetId>` 镜像、每次扩容后调用 `sync_root_borrowed_resource_sets`。现在 Root Variant 两种向量始终为空，原生 Root Color Texture/View 和 ResourceSets 都只由 `RootColorTarget` 拥有；移除镜像同步方法及扩容后的重复同步调用。没有新增 Root 缓存、纹理、RenderTree 或额外 FrameUpload。Filter 非 Root Variant 与 Gaussian 多级目标仍保留自己的 ResourceSets。
- **高水位重排防回归**：`targets/layout.rs::reuse_layout` 在 Root 固定、其它零半径 Element 改变索引的 fallback 中，按 Native `root_color_target.target.byte_size` 与 `pass/composite_resource_sets.len()` 进行容量检查，不再依赖已经删除的 Root 逻辑 Descriptor 镜像；保持原有 Texture/View/Variant permutation 和 Source Version 失效规则。
- **Render Graph 消费者核查**：`resource_set_for_config` 与 `direct_composite_target` 按原生 Texture/View 从 Root Owner 取 Composite/Pass Set；`renderer/draw_steps` 通过这两个入口获取 Root 资源；`draw.rs` 的两级 Gaussian 实现只读取两个级别的 Filter ResourceSet，不读取零半径 Root Variant；`append_blur_bindings` 跳过 Root Variant，仅绑定 Native Owner。资源退休仍由 `destroy_backdrop_blur_target_chain` 跳过 Root 物理句柄并对唯一 Owner 销毁。
- **测试源码**：扩充 `warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`，在构造原生 Owner 后清空借用 Variant 的两套 ResourceSet 向量，检查 6→4→5→6→4 的根物理 ID / 内容版本稳定、Root Graph 两种消费入口持续读取原生新 ID、借用镜像不再出现；新增含新成员索引 42 的 warmed zero-radius 回退重排，证明备用 Filter Slot 的容量比较使用原生 Root Owner 而非空的借用向量，且无需新 Native Texture。这是**测试源码**，不是通过测试的声明。
- **未完成／高风险边界**：Root 真实 Native Owner 仍置于 `BackdropBlurTargets`，其相同 Texture/View 的逻辑 Filter Slot 及 `FrameUpload.retained_root_blur` 仍存在；资源链重建仍须经现有每窗口 Fence/Resize 安全门。下一步先补齐本地编译／原生验证，再沿 `renderer.rs` 的目标重建、`surface_lifecycle.rs` Resize、`renderer/destroy.rs` Window teardown 实施 **独立 Window RootColorTarget 资源槽 + 保留 GPU in-flight 退休安全**，最终使 Render Graph 拿到 Root 原生资源而不依赖零半径 Filter Slot。不得在无证据情况下移除 Fence preflight 或添加第二张 Root Cache。
- **验证命令（尚未执行）**：`cargo fmt --all -- --check`；`cargo check --workspace --no-default-features`；`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`；`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_version_survives_only_texture_preserving_topology_rebind`。另需四后端 FrameSlot 扩容、反向打断 Dropdown、Blur on/off、Root on/off、Resize、DComp/透明、DX12 子窗口关闭、Fence 退休 / ResourceSet 双重释放检测，和 Trace 关闭后的 GPU 占用 A/B。**未测 GPU 性能，不声称已下降。**

## 2026-10-15：P0-06 Root/Filter Native 构建事务回滚与提交退休安全门（源码待验证）

- **基线**：编辑前重新读取远程 `master=d2bcb884e51c17b31d115b0d54649783bf42d3f2`、`docs/AI.md`、本 TODO 和 `targets.rs`、`renderer.rs`、`submission.rs`、`surface_lifecycle.rs`、`renderer/destroy.rs`。本切片先消除独立 Window Root Owner 迁移前的 GPU 资源异常路径，不把逻辑 Filter Capture 伪装成已迁移完成。
- **构建失败回滚**：`create_backdrop_blur_target_chain_with_configs` 以仅在单次调用存活的局部分配日志，追踪已成功创建的 Root/Element/Gaussian/Backdrop `RenderTarget` 与所有 `ResourceSetId`。任意 Texture、View、Set 或尺寸计算返回错误时，先逆序销毁新建 ResourceSet，再逆序销毁新建 TextureView/Texture；已驻留的旧 Filter Chain 和 Root Native 句柄不参与回滚。Root Variant 借用 Native Texture 不重复登记，也不重复释放。不增加帧间缓存或其他 Root Owner。
- **Texture/View 创建原子性**：`create_render_texture_target` 在创建 Texture 成功而 View 失败时主动销毁刚创建的 Texture，避免无法被上层回滚日志捕获的泄漏。成功时才记录完整 Texture/View 对。
- **GPU Fence 安全门**：`NovaRenderer::ensure_backdrop_blur_targets` 将 pending submission fence-drain 提前到所有物理 Filter layout 的 `reuse_layout`、`rebind_zero_radius_layout` 和 full-recreation 分支之前（完全兼容的快速路径不等待）。异步模式的 submission preflight 保留原本的 defer，不以每帧 `wait_idle` 替代；同步模式提交前已等待。`trim_gpui_memory` 的中／重度 Root/Filter 目标销毁必须同时满足 `submissions_drained=true`、当前无 Blur，等待失败不能主动释放仍被 GPU 命令引用的资源。
- **风险与下一步**：当前 `RootColorTarget` 仍在 `BackdropBlurTargets` 内；`FrameUpload.retained_root_blur` 与零半径逻辑 Capture、Render Graph Root Pass 路由没有删除，Window 独立所有权及 per-submission 资源退休尚未实现。下阶段需要先获取本地编译和四后端真实资源泄漏/双释放/Fence 证据，再拆分 Window Root Owner，迁移 Resize、trim、destroy 与 native Render Graph，禁止仅做字段搬迁却引入第二套同步状态。
- **验证未执行**：当前容器无 Rust 工具链，也未执行 DX11/DX12/Vulkan/OpenGL GPU 压测。需运行 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib warmed_zero_radius_popup_targets_rebind_without_reallocating_textures`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_version_survives_only_texture_preserving_topology_rebind`；另需注入各 Texture/View/ResourceSet 创建阶段失败并比较资源 allocation/free、连续快速弹出/关闭、GPU in-flight + Moderate/Aggressive Trim、窗口销毁/DComp Resize。提交是源码正确性改动，不表示 native failure injection / 性能测试通过，不能声称 GPU 40%～50% 已降低。

## 2026-10-15：P0-06 Root 下线物理槽保留 + 当前 TODO 一致性审计（代码待编译）

- 基线父 HEAD `259fa1747c12ea24cc1f6745352992b8c3d85adb`。核对 `targets.rs`、`targets/layout.rs`、`composite_target.rs`、`renderer.rs`、`renderer/submission.rs` 及 P0-06 旧测试。Root Native Texture/View 的逻辑 Variant 已无 Pass/Composite ResourceSet，不能像普通 Filter Scratch 一样重绑。
- `root_capture_is_compatible` 在 Root 下线后要求原生 Texture/View 不位于活动 Filter 前缀；`reuse_layout` 将其排除出普通 zero-radius 精确/回退匹配；`rebind_zero_radius_layout` 在有 spare slot 时将 Root 纹理停放于闲置后缀、再次有 Root 时从 Native ID 找回，没 spare 时拒绝并交现有 Fence 保护的目标链重建。AutoStatic 身份/独立重排路径保留。
- 新增 `root_color_removed_reserves_native_target_until_reintroduced` 单测源码：满容量无 spare 的拒绝应原地不变、缩容后 Native Root 停放、活跃 Filter 均有真实 ResourceSet、AutoStatic 改绑、Root 再次进入 Filter DAG 时从 Native Owner 查询两类 Set。已有 warmed 6→4→5→6→4 测试保留。
- TODO 校验：`P0-01` 阻塞不能标成已通过；`P0-06` Window 独立 GPU Owner、异步 Fence 退休、`retained_root_blur` 移除**全部仍未实现**；GPU 原生时长和公共 Dropdown 性能不能从 CPU busy 推导。历史提交说明与当前源码矛盾时，以最新状态表、源码及测试结果为准。
- **未进行验证**：容器无 Rust/Cargo，`cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib root_color_removed_reserves_native_target_until_reintroduced` 尚未执行，四后端 Root on/off、Popup 高频、Resize/Alpha、GPU Fence/ResourceSet 生命周期、性能 A/B 尚无本轮结果。下一步先完成 P0-01 后再扩大 Root Window GPU Owner 的破坏性迁移。

## 2026-10-16：DX12 43s Trace 分离 Dropdown 与导航/首页成本，P0-05 Gaussian 同规格回退（源码待验）

- **当前基线**：编辑前远程 `master=600023a7cd4070a12ed7c9d3610254486d1668a9`，相对旧 `e47f7ff0d419df4e4715a231bb1f84ad084f5c27` 已有 10 个用户/并行提交。尤其 `0fe7e571cbad` 修复 DX12 Swapchain 子窗口销毁，`a4103b05c769` 更新稳定采样动画及损伤，`6d15651ccaee` 迁移导航水平边缘动画并更新首页版本与引导动画；本轮**不恢复旧代码，不撤销 DX12 子窗口修复，不修改业务 UI 动画**。P0-09 保留“DX12 子窗口关闭复验通过、其它后端待验”。
- **用户新实测（系统任务管理器，不能从日志反算）**：公共 Dropdown 模糊关约 GPU 8% / CPU 5%；BMCBL 模糊开约 GPU 12%；标题栏 Tab 和首页启动下拉在有/无模糊对照中依旧约 GPU 20%。GPU 使用率和 GpuOwner CPU busy 属于不同指标，暂时没有任何本地 GPU pass timestamp 证明直接瓶颈比例。
- **日志证据**：`latest(20261015-161414).log`（DX12、Debug、780M、1458x900、约 44 秒，注意日志未显式绑定确切 commit）按原始行聚合：`nova-gfx copy attribution` 2430，`frame diagnostics` 1372，`element_blur_refresh=true` 2343，`backdrop_blur_refresh=false` 2428，`layout recreated` 138，`layout reused` 10，`rebound` 0；reuse 拒绝源包括 `variant_capacity` 71 次、`target_slot_mismatch` 54 次（多为 radius=3/levels=2）、`isolated_source_capacity` 10 次。详细帧 `dirty_mode=FullRedraw` 1371/1372、`present_damage=None` 1371/1372，但 `dirty_full=false` 1268/1372。**这是日志细节采样的计数，不可与 2430 次合成帧一一对应**。
- **归因边界**：`surface_plan.rs::resolve_surface_packet` 对 `scene.has_backdrop_blurs()` 显式禁用原生局部 Present，原因是 DXGI/透明/旋转 Backbuffer outside-dirty 内容的正确性未证明；不能简单移除这一保护。Element/Root Capture 刷新（大面积 `blur_source_pixels`，频繁 Root 版本增长）和真实 Gaussian `backdrop_blur_refresh` 是两个不同成本中心。新旧 UI 动画迁移已有实现，但 GPU 绘制/布局占比仍需 Trace 与 GPU Timestamp 同源实测。Trace 本身产生大量逐帧 WARN，生产性能 A/B 必须关闭详细 Trace。
- **本轮 P0-05 源码切片**：`targets/layout.rs::reuse_layout` 保留所有精确成员索引匹配和 Root Native pinned 分配的优先级。仅在此前匹配失败、Root 独立 Owner 存在且 pin 成功时，允许从未分配的普通 Gaussian scratch 选择**同 downsample、相同 pass_count（2）、同各 FrameSlot Pass/Composite ResourceSet 容量**的物理目标；全配置校验通过后才发生 swap；root Texture/View 不参与候选，也不重复释放。逻辑输出配置随重排更新、之前 Filter 图内容仍需根据 `ensure_backdrop_blur_targets` 当前 invalidation 重新绘制。增加 `GPUI Gaussian scratch target rebound` 低开销 opt-in Trace，便于下次观察命中。
- **新增测试源码**：`gaussian_capture_shift_reuses_compatible_scratch_without_moving_native_root`，检查 3 个两级 Gaussian 加 1 个独占 Root 的插槽，索引 41 的新 Capture 占用空闲等规格 Gaussian，后续 exact match 均未被抢占；Root Native Texture/View 与两类 ResourceSet 不变；downsample 不同的非法替换拒绝且变更前后 TextureView/配置完全不变。与 `gaussian_source_capacity_overflow_keeps_layout_unchanged`、`gaussian_capture_reorder_reuses_views_without_mutating_gpu_allocations`、Root 下线/重上线原有测试共同验收。
- **仍待办**：这条优化不覆盖 `variant_capacity` 71 次或 `isolated_source_capacity` 10 次；不能预测能消除多少次实际重建。P0-04/07 继续核对从 Scene animation damage 到 Root capture scissor 的全屏扩散、`retained_root_draw_steps`、`blur_source_pixels`、局部输出与 Present 的正确性。P0-06 独立 Window GPU Owner / Fence 仍未完成。标题栏动画 GPU20% 尚未解决。
- **验收**：当前对话环境没有 `cargo/rustc`，没有实机。必须由 Windows 本地执行 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib gaussian_capture_shift_reuses_compatible_scratch_without_moving_native_root` 以及既有 Gaussian/Root 相关测试，并检查 DX11/DX12/Vulkan/OpenGL 图像等价；DX12 Debug trace 复测 `Gaussian scratch target rebound` 与 `layout recreated` 前后次数，再关闭 Frame Metrics+Trace 对相同窗口大小、模糊开关/帧率比对系统 GPU%，无需预先承诺收益。

## 2026-10-16：P0-07 四后端共用 Scene PaintBlur → DirtyRegion 全输出扩散优化（代码未编译）

- **原始源码基线**：`bb4bd89e539438a3a1e876b8cc60c83b63119196`。提交前远程 HEAD 已推进到 `21f12ce0c31566a51c64912dc38258efdd856911`：并行修复撤销首页逐行 sampled Capture，恢复原先弹性位移/透明度。本轮只修改 GPUI Scene 损伤，不改业务 UI/动画轨迹，也不撤销该新提交。本轮遵循用户纠正：DX11/DX12/Vulkan/OpenGL 均出现类似导航/下拉动画高 GPU，根因候选位于 GPUI 公共 Scene/Damage/Root Capture 与多后端共享 Compositor，不把已修复的 DX12 子窗口关闭当作动画瓶颈。保留前序 P0-05 Gaussian scratch rebinding 和 DX12 Swapchain 修复。
- **用户分场景本机指标（非日志推算）**：公共 Dropdown 模糊关 GPU≈8% / CPU≈5%，开模糊 GPU≈12%；标题栏 Tab/首页启动下拉 GPU≈20%，模糊开时可能更高。这不是同一个 GPU Owner busy 百分比，也不是 Gaussian 实测执行时间，性能收益尚未证明。
- **层级分析**：BMCBL 标题栏 `src/ui/main_window/chrome.rs` 用 `.background_blur(glass_backdrop_blur_style())` 绘制背景采样，然后再添加 Nav 胶囊前景。前景动画不需要重算其下方 Gaussian：之前 DX12 Trace 的 2430 条 `copy attribution` 仅约 2 条 `backdrop_blur_refresh=true`；但 Element Layer、Root Color 经常刷新，native Present 仍以 FullRedraw 为主。模糊开销可来自增加的最终纹理采样/合成和公共 RenderPass/Root 重绘，而不代表滤镜缓存逐帧失效。
- **确认公共代码扩散点**：`Scene::for_each_changed_bounds` 对 `PaintOperation::Primitive(Primitive::Blur)` 内容发生任何变化时，原实现按 `PaintBlur.bounds`（可能是整个页面/窗口）报告旧新输出边界；`Window::add_direct_view_damage` 把这些边界送到 `DirtyRegion`，随后 Root 直接重绘、Filter 判断/Present 成本被放大。这与后端无关。独立 `Scene::for_each_element_blur_damage` 仍有保守的结构变化保护。此路径是具体可证实的错误放大机制，但不代表所有 GPU 20% 都来自它。
- **实现边界**：只在前后恰好各有一个匹配的 `Primitive::Blur`、同一 Painter Order/Layer、没有捕获动画变换、Texture 输出边界/ContentMask/opacity 相同、radius=0、内部不含 Element Blur、Backdrop Blur 或不支持的 Surface/Renderer Extension 时，向相同输出遮罩内递归 `new.content.for_each_changed_bounds(old.content)`，传播真正变化的旧新像素。被裁切的旧区域由 previous Scene 一同计算；任何不匹配回退原 full-output damage。不会借此跳过 Native Filter Pass、破坏 Gaussian halo 或强制开启透明窗口的 unsafe partial Present。
- **测试源码**：`scene/tests.rs::identity_capture_child_changes_do_not_dirty_full_capture_output` 对比 100×70 的稳定零半径 Capture 内 8×6 Quad 的局部位置变化，保证只报告 old/new 8×6；改变 Capture opacity 必须报告整个输出；非零 Gaussian 源内容变化仍全输出受损。新增 opt-in `GPUI identity capture child-damage refinement` Trace 事件，方便按 Layer 标识确认命中；只是源码，**不表示 cargo test 通过**。
- **仍未覆盖**：含真实 Backdrop Blur 的页面本就可能使 `surface_plan.rs` 保守选择 Full Present；不能以 `dirty_full=false` 直接推论后端一定有安全 Native Partial Present。动画导致的多操作差异、父路由切换时真实内容大面积改变、RetainedRoot 与 Element 动画同步刷新、DX11/VK/GL 的 swapchain/clip 约束、每帧提交基础成本仍待 Trace+GPU timestamp A/B。P0-06 Window Root Color 独立 GPU Owner、独立 Fence 退休与 `retained_root_blur` 旧语义删除均未完成。
- **验收要求**：Windows 本地 `cargo fmt --all -- --check`、`cargo check --workspace --no-default-features`、`cargo test --manifest-path crates/gpui/Cargo.toml --lib identity_capture_child_changes_do_not_dirty_full_capture_output` 与所有 Scene、Root、Gaussian 单测；四后端开启/关闭模糊分别测试标题栏 Tab、首页下拉、公共 Dropdown、页面切换、高频重定向、最小化恢复和多窗口，记录 `dirty_area`、`retained_root_draw_steps`、`blur_source_pixels`、`element_blur_refresh`、`identity capture child-damage refinement`、GPU timestamp 和 GPU 使用率。静态代码检查不是实机性能验收，不能宣称 20% 已修复。

## 跨对话恢复协议（必须执行）

1. 读取本文件和 `docs/AI.md`，再读取**最新远程 master SHA、Tree SHA**。检查最近 Git 历史是否已改动相关模块。
2. 阅读本文件中最后完成项、未完成项和验收证据。逐条检查实际代码，**不能仅依据 TODO 勾选状态**。
3. 选取可以独立验证的一条纵向切片，先定位真实依赖和正确性约束，避免新增更多冗余生命周期状态。
4. 修改后至少静态检查；能运行 Rust 时优先 `cargo fmt --all -- --check`、针对性测试、必要的 Windows 4 后端截图/GPU 测试。**CI 不得作为本地测试替代品**。
5. **每次提交同步更新本文件**：变更路径、commit SHA 或历史指针、已验证事实、未验证风险、下一步起点。避免一边提交代码，一边让状态过期。
6. 提交前重新读取 master HEAD，比较基线，使用 blob → tree → commit → compare → update_ref(expected_sha) **非强制快进**；HEAD 发生变化则停止并重新对照，禁止覆盖他人提交。
7. 提交消息采用**中文 Conventional Commits + [skip ci]**，例：`perf(gpui): 复用滤镜目标并减少纹理布局重建 [skip ci]`；不要把未通过的测试写成“通过”。
8. 更新 README 或引用入口应始终指向**本文件的固定路径**，禁止创建第二套互相冲突的重构进度文件。
9. 破坏性修改时同步更新**删除清单中的符号搜索结果**与真实消费者；若存在多个等价的 Frame/Scene/Chunk 权威所有者，视为技术债未清，不能标记阶段完成。
10. 此文档的勾选状态必须区分【代码实现】【编译通过】【单测通过】【DX11/DX12/VK/GL 原生验证】；不可因为静态审计与提交创建就视为已验证。

## 下一次对话的直接起点（动态恢复入口）

1. **先重新读取远程 master HEAD、本文顶部 `最新执行看板`、`docs/AI.md` 与最近提交差异**，不要复用本文历史 commit 当最新基线。
2. **按 ID 选择最小切片**：已新增 P0-07 零半径 Capture 子像素损伤 diff（待编译验证），并行最新提交撤回了首页逐行 sampled Capture（不得回退该 UI 改动）；后续优先确认四后端 GPU 源/目标损伤数据，不能将 DX12 的单份 Trace 误作 DX12 专属瓶颈。本轮新日志明确 P0-05 Gaussian 物理目标回退与 P0-07 根内容大面积刷新/局部 Present 阻断尚待实机证明；P0-09 DX12 子窗口关闭已有用户本机复验，禁止回退修复。先验证当前 HEAD 构建及新增 Gaussian 测试，不要把旧 8%/12%/20% GPU 实测与 GPUI Owner busy 混同。P0-06 已将 Root Native Texture/View 与 Pass/Composite ResourceSet 的生产／退休职责归到 `RootColorTarget`；逻辑 Variant 仅借用 Texture/View，**不再有 ResourceSet 镜像及重绑回写**。当前补强了 Root 关闭后的物理槽停放与误用拒绝。**Root 原生 ResourceSet 唯一所有、Filter 逻辑镜像删除；本轮补齐新目标构建失败时的临时 GPU 资源回滚、Filter 拓扑重排前 Fence 安全门及 GPU Trim 成功退休门。独立 Window Root Owner、Render Graph Root 语义迁移与真实 Fence 退休仍未完成，本地 Rust/四后端验证仍缺失**。下次先 Windows `cargo fmt/check`、`warmed_zero_radius_popup_targets` 以及所有 Root/Blur 单测，再在 DX12/11/VK/GL 复测 6→4→6、Root View、Fence/Resize/关闭子窗口。**先完成 P0-01 编译及新 Root 下线测试**，验证 DX11/DX12/Vulkan/OpenGL 的 Root 开关、6→4→6/AutoStatic、Fence/Resize；然后再迁移 Window GPU Owner、DrawSteps/Filter Pass 的 Root 输入与独立 GPU 退休，最后删除 `FrameUpload.retained_root_blur` 旧语义，绝不增加第二个 Root Cache。P0-09 窗口正确性优先。
3. **每次更新需要写在顶部任务表**：状态、源码位置/commit、对应测试命令、日志/截图或未提供原因、是否通过实际 GPU 后端验证；历史小节仅追加事实，不重新建并行 TODO。
4. **提交规范**：下一次实现任务须按非强制快进 GitHub blob→tree→commit→compare→expected_sha 提交流程，并同步更新本文件；禁止在未重读 HEAD 时覆盖用户并发提交的 UI 变更。

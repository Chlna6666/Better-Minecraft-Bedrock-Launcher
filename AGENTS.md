# AGENTS.md

本文件给在此仓库工作的 AI/自动化代理与开发者提供最短、可验证的工作约束。更完整的说明见
[docs/README.md](docs/README.md)、[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)、
[docs/BUILD_AND_TEST.md](docs/BUILD_AND_TEST.md) 与 [docs/NAMING.md](docs/NAMING.md)。

## 1. 项目是什么

BMCBL 是用 Rust + GPUI 编写的 Minecraft Bedrock 原生桌面启动器。主平台 Windows；Linux 处于
测试阶段；macOS 不支持。UI、业务、下载、配置、插件和 Bedrock 解析都在本工作区中实现。

## 2. 先读文档，再改代码

按任务类型选择：

- 结构与 crate 职责：[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)、
  [docs/CRATES.md](docs/CRATES.md)
- 构建、feature、测试矩阵：[docs/BUILD_AND_TEST.md](docs/BUILD_AND_TEST.md)
- 命名规范：[docs/NAMING.md](docs/NAMING.md)
- 变更流程与提交规范：[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)

## 3. 硬性边界

- **不可偏离的分层**：`nova-gfx`（`gfx-core`、`gfx-*`）是类似 wgpu 的轻量、通用图形 API；
  `gpui-3d` 是可用于游戏的专业 3D 渲染库；`gpui` 是**可选的 UI 侧载消费者**，
  不是 3D 命令编码器、3D 场景调度器或图形驱动。
- `nova-gfx`/`gfx-*` 只定义通用 Device/Buffer/Texture/3D Texture/Pipeline/BindGroup、
  Graphics/Compute/Transfer、ResourceBarrier、CommandEncoder、Queue、Submission/Fence、
  时间戳与设备限制；不得依赖 `gpui`、`gpui-3d`、`egpui`、应用代码，也不得引入
  地图/方块/材质/光照/场景策略和 UI 的 damage/retained/atlas/glyph/blur 概念。
  只允许平台 backend 实现通用图形语义，不允许为 BMCBL 特化公开 API。
- `gfx-core` 不依赖 `winit`/`raw-window-handle`；native surface 由 backend 的
  `SurfaceDevice::SurfaceTarget` 承担。
- `gpui-3d` **拥有游戏级 Scene、GPU Resident 数据、剔除、构网、材质/光照、RenderGraph、
  DrawIndirect、原生离屏 Pass 调度及其资源生命周期策略**。纯 3D 引擎模块只依赖
  `gfx-core` 和无 UI 的库；不得引用 `gpui::`、`RendererExtension*`、UI 布局和 BMCBL
  业务代码。当前 crate 尚包含 GPUI 适配代码，应持续将其限定在
  `scene_view` 等 sidecar/adapter 模块，并由默认启用的 `gpui-sidecar` Cargo feature
  控制；`cargo check -p gpui-3d --no-default-features --lib` 应不再引入 `gpui`。
  不能以已有历史依赖为由继续增加核心耦合。
- BMCBL 使用 `gpui_3d::scene_view` 的地图编辑器、地图预览和皮肤预览必须
  **显式开启 `gpui-3d/gpui-sidecar`**，不可仅依赖可复用库默认 feature。
  只允许独立 3D 应用使用 `--no-default-features` 去除 GPUI 依赖。
- 普通 GPUI Sidecar 的 3D 离屏生产者与 UI Consumer 默认使用**同一条有序
  GPU 图形队列上的独立原生命令提交**，避免默认创建 Vulkan/DX12 第二队列
  的 Semaphore/Fence 握手。只有基于 GPU 时间戳证明有收益时才允许改为独立
  物理队列；`nova-gfx` 的通用 `IndependentGraphicsIfAvailable` 仍应保留。
- `gpui` 负责窗口/swapchain、输入/布局、GPU Owner 设备访问协调、纹理 Sidecar 消费、
  Present 与 Fence 生命周期跟踪。不得接收后重新编码、排序或合并 3D 的 DrawIndirect；
  只允许在设备所有者线程调用通用 `nova-gfx` 原生提交入口，并跟踪 `SubmissionId`。
  3D producer 与 GUI consumer 必须保持 GPU 侧同步，不在热路径插入 CPU Fence wait。
- `gpui` 只依赖 `gfx-core` 能力 trait；不得按 backend 调用固有 resize/present/alpha 方法。
- 各层 API 命名必须通用且基于所有权：`nova-gfx` 使用资源/提交语义；
  `gpui-3d` 使用场景/渲染语义；GPUI 只使用 Sidecar/Consumer 语义。
  不得通过重命名私有函数伪造公共库能力，不得把公共 API 限定成 Minecraft 专用功能。
- `egpui` 不拥有 BMCBL 路由、下载、地图等产品语义。
- `bedrock-*` 不依赖 UI/GPU 呈现；`bedrock-voxel` 只产出计划，不直接写世界。
- 不要修改 `crates/easytier`、`crates/raknet`、`vendor/*`（外部/ vendored 代码），除非任务
  明确要求并在 PR 中说明上游版本与补丁原因。

## 3.1 3D 引擎和 UI Sidecar 的验收规则

- 修改 3D 提交链路时，列出两条独立路径：
  `gpui-3d → nova-gfx → GPU`（场景/Compute/Indirect/原生离屏生产）和
  `gpui-3d → GPUI → Present`（纹理消费/窗口/UI）。严禁让 GPUI 2D 合成器成为
  3D Pass 的实际命令编码器；一张最终 3D 结果纹理的 UI 合成不等于“伪 3D”。
- `GpuWorkPlan` 的依赖验证不等于原生 Barrier/状态转换；只有后端确实编码同步并能保证
  提交顺序、Fence 退休和资源可见性，才能宣称支持完整 Graphics→Compute→Graphics。
- 侧载颜色/深度纹理、BindGroup/Placement、Pipeline 的释放必须依据实际
  UI Consumer `SubmissionId` 的 `gpu_submission_completed` 证明，或 GPU Owner
  全局完成证明；`gpu_submission_accepted` 不代表 GPU 已执行完毕。
  GPU Resident DrawParams/Instance/Bounds 的物理槽位卸载后必须先进入隔离队列：
  以该 SceneView **实际最后一次 Native DrawIndirect Producer SubmissionId** 的
  完成状态为首选依据；原生后端未返回可靠 ID 时，仅允许使用同一有序 GPU Owner
  上后续真正已完成的 Consumer Fence，或 GPU 全局完成证明作为保守上界。
  逻辑槽位释放不等于物理地址可复用，Tail Trim 也不能回收未完成的槽位。
  GPU Resident Mesh 子分配可能被多个 Compute/Draw 提交引用，不能直接套用
  Draw Slot 的单 Producer 完成策略；仍需独立追踪使用者或保守全局退休。
- 上传、Compute Meshing、DrawIndirect 的**原生 GPU 时间戳**必须与
  `sidecar_cpu_prepared`、`gpu_owner_phase`、`offscreen_native_submit`、
  `native_bridge_cpu`、`gpu_owner_submission_wait`、Present 等主机耗时分开记录；
  不把 CPU API 延迟称作 GPU 执行时间，不臆造 FPS 收益。
- 不以降低 3D 质量、减少正确的 Shadow Pass、降帧或每帧 CPU 同步换取假加速；
  多 SceneView、窗口尺寸变化、挂起/恢复、GPU 设备丢失及在途资源复用必须有失败路径。
- 破坏性开发允许，但公共 API 变更必须处理所有调用方和四后端的能力回退；
  未实际执行的编译/硬件测试不能标记为通过。

## 4. 修改流程

1. 先声明 IN/OUT SCOPE、受影响 crate、平台与 feature。
2. 找到最小改动点；不要顺手重构无关模块。
3. 公共 API 变更先确认边界，再更新所有调用方、rustdoc 和文档。
4. 新行为必须有测试；硬件相关测试保持 `#[ignore]` 并注明前提。
5. 文档中英文成对更新；不提交测试输出、benchmark 原始数据、改名历史、带日期计划。

## 5. 验证命令

最少验证：

```powershell
rustfmt --edition 2024 <本次改动文件...>
cargo check -p <受影响 crate>
cargo test -p <受影响 crate>
pwsh -File scripts/check-docs.ps1
```

常用矩阵：

```powershell
cargo check -p gfx-core -p gfx-dx11 -p gfx-dx12 -p gfx-vulkan -p gfx-opengl -p gfx-metal -p gfx-webgl
cargo check -p gpui --no-default-features --features "nova-gfx-dx11 nova-gfx-dx12 nova-gfx-vulkan nova-gfx-opengl"
cargo check -p gpui-3d --tests
cargo check -p bmcbl
$env:RUST_MIN_STACK = '67108864'
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib platform::nova::
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-gpui-3d-boundary.ps1
pwsh -File scripts/check-bmcbl-gpui3d-integration.ps1
pwsh -File scripts/check-naming.ps1
```

硬件 gate（需要 GPU/窗口）：

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-vulkan -- --ignored --nocapture --test-threads=1
```

不要把未实际运行的命令写成“已验证”；不要编造性能数字。

## 6. Rust API、接口与命名规范（AI 必须遵循）

本节约束**新代码及本次变更触及的代码**：公共 API、trait、内部函数、方法、构造器、参数、
结构体字段、enum、局部变量与模块名称。先遵循 Rust API Guidelines 和
[docs/NAMING.md](docs/NAMING.md)，再借鉴 JS/TS/Java 的**易读、职责明确和参数对象**设计；
**不能照搬** Java 的 camelCase、`getXxx()`、`IInterface`、重度面向对象或不必要的 getter/setter。
Rust 标识符继续遵守 `snake_case`（函数、变量、字段、模块）和 `UpperCamelCase`（类型、trait、enum）。
Edition 2024；库代码避免用 `unwrap()`/`expect()` 处理可恢复错误，每个 `unsafe` 块写
`// SAFETY:`。

### 6.1 核心原则：短动作 + 明确上下文 + 类型承载细节

- **名称表达动作或概念，所属模块/类型表达上下文，参数类型表达选项。**
  优先 `TaskManager::create(options)`、`TaskManager::update_progress(id, update)` 这种可读契约，
  不要把“任务 + 详情 + 可见性 + 工作流 + 条件”逐词串成方法名。Rust 自由函数
  `task_manager::create_task(options)` 同样合理；**不为模仿 Java 强制引入类/trait**。
- 一次操作应有一个主入口；不同参数组合优先用 `*Options`、`*Descriptor`、
  `*Request`、builder 或类型明确的 enum 描述。禁止不断新增
  `create_x_with_a`、`create_x_with_a_and_b`、`create_x_for_c_with_a` 的排列组合。
- 接口名宜在 **2～4 个有意义的单词**内，能短则短；超过约 **32 字符**优先检查职责和
  重复上下文，超过 **40 字符**必须解释不可压缩的领域术语。**不是硬截断**：
  宁可精确也不要 `run()`、`do_it()` 或难以识别的缩写。测试名称可更长。
- 名称不包含不必要的实现过程、执行时机和隐藏前提，例如
  `*_inner`、`*_impl`、`*_if_needed`、`*_this_frame`、`*_after_result`、
  `*_deferred_id`；仅当同步/异步、阻塞/非阻塞、队列/资源所有权等是**调用方必须选择的
  契约差异**时，才保留限定词。内部 helper 也遵守，不因为 `private` 就允许长句。
- 遵循已有图形术语：`nova-gfx` 用资源、设备、命令、队列、提交语义；
  `gpui-3d` 用场景、剔除、渲染语义；GPUI 只用 UI/Sidecar 消费语义。
  后端专有名词可以出现在专有实现里，不能泄漏为通用接口的硬编码命名。

### 6.2 函数、方法及构造器

- 动作使用明确的常用动词：`create`、`open`、`start`、`load`、`read`、
  `write`、`encode`、`decode`、`prepare`、`submit`、`present`、
  `update`、`set`、`insert`、`remove`、`take`、`clear`、`validate`、
  `sync`、`refresh`、`ensure`。避免万能 `handle`、`process`、`execute`、
  `perform`、`manage`，除非它们在所属领域具有明确且一致的含义。
- 语义区分：`set_*` 是替换确定状态，`update_*` 是更新已有状态，
  `refresh_*` 从来源重新读取，`sync_*` 使两个状态一致，`ensure_*` 保证后置条件，
  `try_*` 表示可能失败且由返回类型体现；禁止同义词随意互换。
- `new`/`default` 用于惯用构造；带有复杂配置的构造用 `Options`/`Descriptor`
  或 builder。不要设计 `new_with_x_and_y` 家族。`with_*` 可以用于标准消费式 builder，
  但不应靠 `with_*` 叠出多个公共函数重载。
- 谓词优先 `is_*`、`has_*`、`can_*`、`should_*`，返回 `bool`；
  可失败的检查另用 `validate_*`/`check_*` 并表达 `Result`，不要让 `is_*`
  兼做错误读取。简单字段读取遵循 Rust 惯例 `status()` 而不是 `get_status()`；
  按 key/index 查询允许 `get`/`get_mut`。
- `as_*` 表示借用/视图，`to_*` 表示转换或生成，`into_*` 表示消费所有权，
  `from_*` 用于构造；这些名称与实际所有权、成本和返回值必须相符。
- 不用冗长后缀区分返回值，如 `*_id`、`*_result`、`*_timed`：
  能用清晰返回类型（`Result<T, E>`、命名 result struct）或单一返回契约解决时就不拆方法；
  **确实存在不同语义的操作**才使用不同方法名。不要凭字数机械合并不同的业务操作。

### 6.3 参数设计：类型化配置代替句子和参数列表

- 接口显式参数（**不算 `self`**）达到 **5 个**，或有超过 2 个同类型、易混淆的位置
  参数时，优先引入有领域含义的 `*Options`、`*Descriptor`、`*Request`、
  `*Update`。内部热路径也应评估；必须保持零开销时可用借用的参数结构体，
  不为包装而分配或克隆。参数对象只分组同一职责，禁止设计万能上下文大包。
- 不用 `bool` 位置参数表达**运行模式或策略**，例如 `independent: bool`、
  `voxel_shader: bool`、`track_native_producer: bool` 在决定不同执行分支时应评估
  `enum` 或 options 中明确的策略字段。普通布尔**状态值**（如
  `enabled: bool`、`visible: bool`）是正常的 Rust API，不必无意义地 enum 化。
- 参数名只描述**角色、作用域、单位和歧义**，不重复方法或类型名：
  `task_id`、`parent_id`、`source`、`destination`、`encoder`、
  `elapsed`、`bytes`、`timeout`。避免 `id_opt`（直接 `id: Option<_>`）、
  `some_data`、`obj`、`val`、`param`、`arg1`、`tmp`、`flag`，
  以及 `requested_initial_current_*` 这类堆叠修饰。
- 同时出现多个标识符时写清角色（`task_id`/`parent_id`）；
  同时出现多个计数时写清维度（`completed_bytes`/`total_bytes`）；
  数值单位可用 `_ms`、`_bytes`、`_px` 标明，但 `Duration` 等有类型单位的
  参数不要重复标注。坐标数学中的 `x`、`y`、`z`，迭代器的 `i` 等惯用短名可以保留。
- 输入 `&T`/`&str`、所有权 `T`、可选 `Option<T>`、失败 `Result<T, E>`
  必须忠实表达；优先使用已有领域类型（ID、Handle、Descriptor、Error），而非为了
  通用性统一成 `String`、`bool` 或无类型元组。跨 FFI/协议的名字和序列化键必须与
  外部契约一致，必要时通过 `serde(rename)` 保持线格式而使用内部惯用命名。

### 6.4 trait、类型、字段和模块

- trait 表达**可替换能力/行为**，以领域名词或能力命名（如 `TaskStore`、
  `SubmissionDevice`），方法使用 `create`、`submit` 等短动作。
  不用 `ITaskManager`、`*Interface`、`*Base`、`*ImplTrait`；
  没有多实现或约束需求时不为封装而新建 trait。已有 `gfx-core` 能力 trait 是公共契约。
- 类型名为稳定名词；`Request` 表示一次输入、`Options` 表示可选配置、
  `Descriptor` 表示资源完整描述、`Update` 表示增量、`Snapshot` 表示状态快照、
  `Handle` 表示生命周期引用。不要用 `Info`、`Data`、`Stuff`、`Manager`
  作为不说明职责的兜底后缀。
- struct 字段、enum 变体、const、type alias、泛型类型参数也纳入命名检查；
  字段名不把所属结构体名字重复一遍。boolean 字段可用 `enabled`/`visible` 或明确
  的 `is_*`/`has_*`；多状态要用 enum，禁止相互矛盾的布尔组合。
  泛型采用 Rust 常用 `T`/`E`/`K`/`V`，多个同类泛型歧义时改用语义化名称。
- 模块、文件名以**领域/职责**命名，不以调用链句子、实现步骤、临时版本号命名。
  一个大接口包含多个独立职责时按模块、子 trait 或参数/结果类型拆分，
  而不是用更长的函数名掩盖边界错误。

### 6.5 内部实现、局部变量与回调

- 规范适用于 `fn`、`pub fn`、`impl`、`trait` 的所有方法，也适用于闭包参数、
  解构绑定、局部 `let`、临时状态字段；**不能只修公开 API**。
- 局部变量短但要能区分来源和生命周期；仅在极短作用域使用 `i`、`item`、
  `value` 等惯用名。长过程中的 `data`、`result`、`state` 如出现多个角色，
  应改为 `download_result`、`scene_state` 等清楚的语义。
  `old_*`/`new_*` 仅用于对照，优先 `previous_*`/`next_*` 表明阶段。
- `*_inner` 不应成为规避职责设计的常用命名：若确实只有单个私有 helper，
  按它实际做的动作命名；若它与外部 API 做同一件事，先评估能否合并调用层。
  特别避免一组只在可选字段上有差异的薄包装函数。
- `async`、线程/队列模型与 GPU 资源生命周期属于**语义契约**：不能为了缩短名字
  消除等待/阻塞差异或将 Fence、所有权、错误处理藏起来。应优先通过明确类型、trait、
  返回值与 rustdoc 说明，而不是连续添加多个形容词。
- 文档注释只解释**不显然的约束、失败模式和时序**，不把长函数名翻译一遍；
  公共 API 写明错误边界，适用时提供 `# Errors`/`# Panics`。

### 6.6 当前项目的典型 API 设计方向（示意，非现有接口声明）

```rust
// 避免：通过方法名与位置参数不停追加配置。
// create_task_with_details_and_visibility(id_opt, title, detail, stage, total, pause, visibility)
// update_progress_with_visualization(id, delta_bytes, total, stage, visualization)
// start_install_with_parent(request, parent_id)

// 推荐：以稳定的动词入口 + 类型化输入表达差异。
fn create_task(options: TaskOptions) -> Result<TaskId, TaskError>;
fn update_progress(id: TaskId, update: ProgressUpdate);
fn start_install(options: InstallOptions) -> Result<InstallHandle, InstallError>;

// 图形 API 同理：不要通过 deferred/timed/id/with_* 等后缀组合模拟重载。
// 使用 CommandEncoderDescriptor、SubmitOptions、PresentTimings 等已有语义类型；
// 保留 gfx-core 现行通用 trait 的明确契约及四后端的一致性。
```

这些是**未来设计范式，不代表现在必须重命名或新增对应类型**。任务/工作流关系通过
`TaskOptions`、`TaskScope` 等明确概念表达；“独立任务”仍是默认独立任务，
父子关系必须由调用者明确建立，不能因函数名带 `workflow` 就隐式改变所有权。
同一动作在设置页、版本管理页、安装流程中应复用同一入口和同一类型语义，
而不是各复制一套 `*_with_parent`/`*_for_workflow` API。

### 6.7 AI 编码前、评审时的强制检查

1. 先找所属模块已有的公共入口、options/descriptor、trait 和领域术语；
   同功能不得在多个模块创建近义别名。
2. 检查**本次新增或触及**的函数、trait 方法、参数、字段、局部变量：
   是否重复上下文、连词过多、名称超过约 32/40 字符、参数过多、布尔模式不清、
   结果语义与名称不一致。
3. 对每个长名称先回答：职责能否拆分？限定词能否放入类型/enum/options？
   返回类型能否代替后缀？是否能直接复用已有接口？**不能无脑截短或改成缩写**。
4. 公共 API 变更必须同时检查所有调用方、trait 实现、平台 backend、序列化/FFI、
   rustdoc 和所需 feature；不保留没有明确需求的旧名称代理、`compat`/`legacy` 壳。
5. 已存在的问题在**下一次触及相关模块时**逐步治理。**本规范变更本身不授权
   批量重命名、不修改业务代码、不制造纯改名提交**；测试函数和外部约定名称按原有规则豁免。

### 6.8 `frame` 命名与帧生命周期语义

- **`frame` 是领域概念，不是通用后缀或万能上下文。** 在函数、方法、trait、
  字段、参数、局部变量、模块和类型中使用 `frame` 前，必须说明它指的是：
  CPU 侧准备/布局帧、GPU 命令录制帧、提交批次、交换链图像、实际呈现帧、
  动画帧、采样时间区间，还是网络协议帧。不同概念不得仅靠 `frame` 一词混用。
- 对 `*_frame`、`frame_*` 和独立的 `frame` 统一做**上下文与职责审查**，
  不能只检查后缀；尤其审查 `render_frame`、`update_frame`、
  `process_frame`、`prepare_frame`、`record_*_frame` 这种可能把多个阶段
  合并或重复上下文的名称。先确定动作实际处理的**对象**，再判断是否需要
  `frame` 限定，而不是机械把它替换成 `render`、`pass` 或 `present`。
- **函数名突出真正的操作和目标。** 仅录制命令用 `record_*`/`encode_*`，
  资源更新用 `update_*`/`upload_*`，提交用 `submit_*`，呈现用
  `present_*`，调度用 `schedule_*`，具体渲染阶段用 `render_*`；
  当类型或模块已经表明帧上下文时，避免再加 `_frame`。
  例如 `record_render_step_list_frame` 应首先评估是否实际是在录制交换链
  Render Pass 命令，若是可设计为 `record_swapchain_pass`；**必须先验证
  方法行为，示例不是未经审查的重命名指令**。
- **保留语义明确的惯用名称**：`begin_frame`/`end_frame` 仅用于真实帧边界；
  `next_frame` 用于推进帧序列；`frame_index`、`frame_count`、
  `frame_time` 表示具体索引、计数或时序；
  `PresentFrameDescriptor`、`PresentedFrame`、`AnimatedFrame`、
  `FrameUniforms` 等类型在含义准确时保留。不能因包含 `frame` 就强制改名。
- **不得把 GPU 命令录制、队列提交、Present 误称为同一个 frame 操作**。
  尤其在 `nova-gfx` / `gpui-3d` / GPUI Sidecar 中，`FrameId`、
  `SubmissionId`、`SwapchainImage`、`RenderPass`、`PresentedFrame`
  各有不同所有权、完成状态和生命周期；命名必须匹配实际返回值与同步语义，
  不得为了缩短名称而隐藏 Fence、CPU/GPU 时序或错误路径。
- **变量名按作用域判断**：在只有一个清楚的帧对象的短闭包/局部作用域中，
  `frame` 可用；同时存在上帧/下帧、动画帧/呈现帧或 CPU/GPU 数据时，
  用 `previous_frame`、`next_frame`、`animation_frame`、
  `presented_frame` 等明确角色。`_frame` 作为 Rust 故意忽略未使用绑定
  的惯用写法，不因这个前导下划线被判为长句式后缀问题；如果实际使用参数则
  应正常命名。
- AI 审查时记录“**名字 → 所属类型/模块 → 实际执行阶段 → 输入/返回语义**”：
  如果仅凭 `_frame` 无法区分职责，先检查实现与调用方，再调整名称或抽取职责；
  不对所有匹配项批量替换，不改协议、trait 或外部 API 约定的名字而不处理依赖。
  **本规范更新只修改 AI 规范，不授权修改 Rust 业务代码。**

### 6.9 构造器、工厂、转换与配置对象的边界（防止表面改名）

- **先建模、后命名，禁止“重命名式重构”。** 将 `create_task_with_details(...)` 改成
  `TaskOptions::from_parts(...).create()`，若仍沿用同一串位置参数、布尔模式、
  隐式默认值与任务创建副作用，**不算遵循规范**。要审查接口拥有者、必要配置、
  默认值、输入所有权、返回错误和调用链，而不是仅检查是否出现禁用单词。
- **`new` 是普通构造入口，不因名字短而禁用。** 如
  `TaskOptions::new(title, stage)` 创建纯配置；带可选属性时使用 builder
  （`.detail(...)`、`.total(...)`、`.visibility(...)`）或结构体字面量。
  若必填参数本身已过多，应进一步引入语义化的 `Request`/`Descriptor`，
  而不是改成 `new_with_*`、`new_from_*`、`from_parts` 规避长度检查。
  `Default` 仅在默认状态本身合法、含义明确时实现；
  需要校验且可能失败的构造使用 `Result`（如 `try_new`）并保留失败信息。
- **`from_*` 必须有来源语义，不是任意构造器别名。**
  `From<T>` 表示惯用且不会失败的转换，`TryFrom<T>` 表示可失败转换；
  `from_bytes`/`from_path` 等仅在输入确实是该数据表示时适用。
  `from_parts` 只能用于领域中真实、稳定且有定义的「组成部分」
  （例如明确的字节片段或解析组件），**禁止**作为多参数列表、布尔开关或
  `new_with_*` 的统称。若方法创建通道、注册回调、启动监控、访问全局资源或
  获取外部句柄，它是在**初始化/建立服务**，不应仅为避开 `new_with_backend`
  而改成 `from_backend`；优先 `new(backend)`，或者由工厂/服务对象承担。
  `from_*` 并非所有含初始化步骤的方法都绝对不可用，但须证明其调用者真正
  将输入理解为转换来源，而不是只是一个工厂选项。
- **纯配置对象不执行业务副作用。** `TaskOptions`/`*Options`/`*Descriptor`
  的职责是收集、校验和携带配置，不应自动拥有 `create()`/`install()`/
  `submit()`/`run()` 等注册全局任务、I/O、GPU 提交动作。
  这类动作属于有权限和生命周期的 `create_task(options)`、
  `TaskManager::create(options)`、`TaskScope::create_task(options)`、
  `Device::submit(...)` 等调用者或服务。若 builder 的终结操作确有必要，
  **builder 必须显式持有执行服务/上下文**（而非纯 Options），且要说明
  所有权、错误及副作用；不要在配置结构体上偷加“便捷 create”。
- **选项类型不等于“所有调用都要造对象”。** 两三个清楚的必要参数可以使用
  `new(a, b)` 或简洁函数；大量易混淆的位置参数、可选字段和模式策略才适合
  parameter object。避免 `None, false, None` 一列无意义的默认值。
  builder 只配置真实的可选字段，`None` 应由默认配置表达；不要重复存放含义
  相同的字段或为每种组合创建一个包装构造器。
- **返回值与失败模式必须保真。** 从旧的 `create_task...` 改为新入口时，
  不能偷偷把 `Result<TaskId, Error>` 改成只返回 `TaskId`、
  `expect`/`unwrap`，也不能丢失重复 ID、无效 parent、取消/线程安全等校验。
  若 API 真正需要同步或异步、不透明句柄、独立任务作用域，靠明确类型、服务
  和错误契约表达；不能用 `.create()` 掩盖跨模块全局写入。
- **能力、配置与状态谓词不得互换。**
  `supports_*`/`can_*` 表达功能能力或前置条件；
  `is_*_enabled`/`is_*_active` 表达当前配置/运行状态；
  `is_*_visible` 表达可见性；
  `should_*` 表达策略决策。
  例如将全局开关 `task_visualization_enabled()` 改成
  `supports_visualization()`，若它仍只读取启用配置，就属于**语义错误**，
  而不是成功缩短名称；必须检查实现、配置来源和调用者条件。
- **更新动作应区分增量、绝对量与元数据。** 使用 `ProgressUpdate` 等类型时，
  明确 `advance_bytes` 与 `completed_bytes` 的不同含义，
  `total_bytes` 和阶段修改的可选语义。不要让
  `ProgressUpdate::new().total(x).stage(y)` 意外产生“进度增加 0 字节”
  或覆盖上次状态的副作用；确保空更新的行为、原子性、通知频率与旧接口一致。
  不要为了显得像 JS/TS 就引入额外分配、克隆或间接调用。
- **本项目示意（名称需依据实际类型与返回值确定）：**

  ```rust
  // 反例：多参数被藏到 from_parts；Options.create() 偷偷创建全局任务。
  // let id = TaskOptions::from_parts(None, title, detail, stage, None, false).create();

  // 正例：TaskOptions 仅收集选项，创建操作由任务服务负责。
  let options = TaskOptions::new(title, stage).detail(detail);
  let task_id = create_task(options)?;

  // 反例：仅把 new_with_backend 改成 from_backend，真实初始化行为未变。
  // fn from_backend(backend: SessionBackend) -> Arc<Self> { ... }

  // 若这是创建会话、通道和监控，命名为初始化；不是表示转换。
  // fn new(backend: SessionBackend) -> Arc<Self> { ... }
  ```

  示例中的 `detail` 为非可选字符串，调用方持有 `Option<String>` 时应只对
  `Some(detail)` 调用 builder；不得凭示例把 `None` 转为空字符串。
- **AI 改动验收必须审查 Diff 而非仅审查结果名。**
  对构造器/工厂/更新 API：列出「修改前契约 → 修改后契约」、
  参数个数和角色、可选默认值、创建方/所有者、是否触发 I/O/状态写入、
  `Result` 传播、调用方变更、兼容性与性能影响；
  检查是否仅发生 `with_*` → `from_*`、`create_task(...)` →
  `options.create()`、`*_enabled` → `supports_*` 等语义可疑替换。
  必须优先复用工作区已有的 `TaskOptions::new` 和 `create_task` 等已建立入口；
  不允许新增等价别名、薄包装或“方便 AI 搜索”的冗余 API。
  **本规范追加不授权修改正在进行的本地大规模重命名、未提交文件或业务实现。**

### 6.10 完整 Rust 标识符规范的判定级别

本规范覆盖**所有新增或本次实际触及的 Rust 声明与绑定**：自由函数、固有方法、
trait 声明与 impl 方法、参数模式、返回类型、struct/enum/union 字段、关联项、
类型别名、泛型与生命周期、常量与静态变量、闭包参数、局部 \`let\`/\`for\`/
\`match\`/\`if let\` 绑定、模块、文件及宏生成的可维护代码。不得只检查 \`pub fn\`。

以下标记在本节及后续审查中具有确定含义：

- **[H-SYNTAX] 强制且可立即机器判定**：Rust 编译器/rustfmt 能确定的命名
  风格、语法、可见性、类型匹配、trait 实现及未使用绑定等；须运行适用的实际工具。
- **[H-AST] 强制，需语法树检查器才能可靠自动判定**：函数及参数绑定的结构、
  标识符占位符、显式参数数量、重复命名模式、方法所属类型等；原则上是阻断条件，
  但**现有脚本未实现的检查不得声称已经机器验证**。在完成工具实现前，
  由 AI 针对变更范围逐项检查并如实说明未覆盖项。
- **[H-REVIEW] 强制、不可仅凭名称/正则判定**：语义、所有权、失败模式、
  外部契约、API 职责、是否机械改名、线程/GPU 资源生命周期；需要阅读实现和调用方，
  不通过就不能将命名变更称为完成。
- **[A] 审查建议/启发式告警**：词数、长度阈值、位置参数数量、疑似冗余词、
  \`frame\`/\`data\`/\`context\`、同类型参数、模式 \`bool\` 等。触发时需要解释、
  拆职责或修改；**并非自动改名指令，也不是可无条件阻断的语法错误**。

**适用边界**：规则只对新增、修改的符号及其依赖调用链强制生效；历史未触及代码
先报告，不自动全局重命名。Rust 标准库约定、第三方 trait 强制签名、
\`extern\`/ABI、FFI、序列化字段、协议字段、宏生成代码、测试中的描述性长名称
及上游代码可保留，但必须区分「不可更改的外部契约」与「可修的内部命名」。
允许 \`self\`、\`Self\`、合法的原始标识符 \`r#type\`、匿名占位 \`_\`、
短作用域的 \`x\`/\`y\`/\`z\`/\`i\`、惯用类型参数 \`T\`/\`E\`/\`K\`/\`V\`、
忽略值的 \`_name\`；不得靠格式规则将它们批量替换。

### 6.11 标识符、参数及返回契约的强制规则

| 对象 | 强制验收 | 需语义审查的告警 |
| --- | --- | --- |
| 函数/方法名 | **[H-SYNTAX]** 遵循 \`snake_case\`；**[H-REVIEW]** 名称描述实际动作、对象和副作用，不用假 \`from_*\`、\`*_frame\`、\`*_inner\` 或 \`*_with_*\` 规避设计问题 | **[A]** 超过约 32 字符、4 个以上语义单词或超过 40 字符，检查是否重复所属模块/类型上下文 |
| 函数参数/接收者 | **[H-SYNTAX]** 声明及借用/所有权合法；**[H-AST]** 新增 API 不使用 \`arg1\`/\`arg2\`、\`param1\`/\`param2\`、\`foo\`/\`bar\` 等无角色的编号占位名；**[H-REVIEW]** 名称与实际角色一致 | **[A]** 显式参数 ≥5（不计 \`self\`）、重复同类型参数、\`bool\` 模式、\`id_opt\` 等把类型信息写进名字 |
| 返回类型及返回值 | **[H-SYNTAX]** 类型与所有路径、trait 签名匹配；**[H-REVIEW]** \`Option\`、\`Result\`、引用/所有权、错误和异步完成语义真实 | **[A]** 多成员无名元组、\`String\`/裸数字承载多种领域含义、靠 \`*_result\`/\`*_id\` 后缀区分相同动作 |
| struct/enum/union | **[H-SYNTAX]** 类型与 enum 变体 \`UpperCamelCase\`，字段 \`snake_case\`；**[H-REVIEW]** 表示稳定领域概念 | **[A]** \`Info\`/\`Data\`/\`Context\`/\`Manager\` 作为无职责后缀、类型与字段重复命名 |
| trait/impl/关联类型 | **[H-SYNTAX]** 实现与 trait 契约匹配；**[H-REVIEW]** trait 是明确能力，方法名跨实现一致，关联类型如 \`Item\`/\`Error\`/\`Output\` 与用途一致 | **[A]** 人为创建 \`I*\`/\`*Interface\`/\`*Base\`、为缩短名称引入空转发 trait |
| 闭包/模式绑定 | **[H-SYNTAX]** 标识符合法；**[H-REVIEW]** 闭包入参与捕获状态、解构别名清楚，避免变量遮蔽导致值来源混淆 | **[A]** 长闭包中仍用 \`a\`/\`b\`/\`res\`，或把每个 \`item\` 强行扩写成长句 |
| 局部变量/字段 | **[H-SYNTAX]** \`snake_case\`；**[H-REVIEW]** 变量表达状态角色、单位和生命周期；布尔字段明确是状态还是策略 | **[A]** \`temp_data\`、\`current_new_*\`、不必要的 \`_frame\`、过长的流程描述 |
| const/static、模块/文件 | **[H-SYNTAX]** 常量/静态变量 \`SCREAMING_SNAKE_CASE\`，模块 \`snake_case\`；**[H-REVIEW]** 模块名描述职责，不以临时步骤或版本名命名 | **[A]** 技术术语重复、阶段性 \`new_v2\`、和父模块完全同义 |
| 泛型/生命周期 | **[H-SYNTAX]** 符合 Rust 类型/生命周期规则；**[H-REVIEW]** 多个同类参数和生命周期要能区分契约 | **[A]** 只有一个惯用 \`T\` 却改成冗长名，或复杂泛型仍使用无法区分的连续单字母 |

**关于“返回类型命名”**：Rust 函数的 \`-> T\` 并没有独立的「返回参数名」。
需要规范的是返回类型及其领域名称、关联类型名称、错误类型、返回的生命周期/所有权，
以及 \`Result<T, E>\` 与 \`Option<T>\` 的选择。不得虚构 Java 式命名返回参数。

**函数参数声明具体要求**：

1. **[H-REVIEW]** 同一函数中 \`id\`、\`parent_id\`、\`source\`、\`destination\`
   等必须能唯一表明角色；\`old\`/\`new\`、\`previous\`/\`next\` 取决于真实状态阶段。
   数值需要区分数量、累计量、偏移、索引、容量、限额和时间；例如
   \`bytes_written\`、\`total_bytes\`、\`offset_bytes\`，不能仅因都为 \`u64\`
   就重命名成 \`count\`、\`value\`。
2. **[H-AST]** 新引入的公共接口参数不以 \`arg[0-9]+\`、
   \`param[0-9]+\`、\`value[0-9]+\`、\`foo\`、\`bar\` 命名；有外部 ABI
   或 trait 强制契约时可记录豁免。普通单个 \`value\`、\`input\`、\`output\`
   只有在所属方法与作用域能明确意义时允许；不要把所有 \`value\` 无条件报错。
3. **[A]** 显式参数 ≥5，或者两个以上同类型位置参数容易传反时，提出具名
   \`Options\`/\`Descriptor\`/\`Request\`/\`Update\` 或领域类型的方案；
   只有确实属于同一职责且不引入不必要拷贝/分配才采用，不能通过
   \`from_parts(a, b, c, d, e, f)\` 继续传递位置参数。
4. **[H-REVIEW]** 多个 \`bool\` 如果代表互斥模式/执行策略，使用 enum；
   \`enabled: bool\`、\`visible: bool\` 等自然状态值可以保留。
   \`Option<T>\` 与 \`Result<T, E>\` 不能只凭名字判断；参数名不必加 \`_opt\`
   或 \`_result\`。数字单位以类型和真实意义为准，避免同单位重复后缀。
5. **[H-REVIEW]** 回调的 \`move\` 捕获、参数借用、\`Arc\`/\`Rc\`、可变引用、
   生命周期与错误传递不得因重命名或改用参数对象而改变；
   \`self\`/\`&self\`/\`&mut self\` 应反映实际状态权限与方法职责。

**返回值与命名的对应关系**：

- 查询可能缺失 → 通常 \`Option<T>\`；操作可失败 → \`Result<T, E>\`；
  两者可同时存在（如 \`Result<Option<T>, E>\`）；不要把错误伪装成 \`None\`
  或 \`false\`。返回 \`bool\` 的 \`is_*\`/\`has_*\` 等只能表示可判断的谓词。
- 使用 \`&T\`/\`&mut T\`、\`Arc<T>\`、\`Cow<'_, T>\`、\`T\` 时明确借用、
  共享和消费关系；\`as_*\`/\`to_*\`/\`into_*\` 选择与成本和所有权一致。
  \`async fn\` 的返回值是 Future 完成后的输出，不能用 \`_async\`
  后缀假装已完成 I/O 或 GPU 提交。
- 多个需共同使用的返回值优先命名领域结果 struct，简单标准元组可以保留；
  避免为规避长函数名发明 \`SomethingResultData\` 空壳，也不要用
  \`Result<String, String>\` 取代已有适当的错误/ID 类型而损失语义。

### 6.12 自动验收契约与现有工具边界

**不得将“写在 AGENTS.md”误称为“已由脚本自动强制执行”。**

- **现状**：\`scripts/check-naming.ps1\` 以逐行正则检查一部分
  \`nova-gfx\`/GPUI 的**公开函数名**、长名称和部分词汇；不会完整解析
  多行函数签名、模式解构、trait 方法/参数、泛型、闭包、返回类型或局部变量。
  脚本不覆盖整个 BMCBL Rust workspace，且原有未触及代码可能存在告警。
  因此运行成功**不能证明**本节所有标识符检查通过。
- **[H-SYNTAX] 立即执行**：对本次实际修改的 Rust 代码运行
  \`cargo fmt --check\` / \`rustfmt --check\`（按项目适用范围）、
  对受影响 crate 执行 \`cargo check\`，并按需要运行
  \`cargo clippy\`；检查 rustc 的 \`non_snake_case\`、
  \`non_camel_case_types\`、\`non_upper_case_globals\` 等命名 lint。
  不在未审查的整个 workspace 上临时添加 \`deny(warnings)\` 制造无关失败。
  修改纯 Markdown 时不需要伪造 Rust 编译检查。
- **[H-AST] 未来自动检查器的实现合同**：必须以 Rust AST/HIR
  （如 \`syn\`、rust-analyzer 解析信息或等价可靠解析器）识别声明和绑定；
  按 Git Diff 范围定位新增/修改的符号，并保留所属文件、行、列、模块、可见性、
  属性、参数类型与函数签名。不得用全局文本替换或简单正则来“修复”语义问题。
  若解析宏展开、条件编译、外部生成文件不可确定，给出
  \`manual-review\`，不可假报 \`pass\`。
- **[H-AST] 最小自动规则集**：
  1. 收集 \`fn\`（自由、固有、trait、impl）、接收者与参数模式
     （含多行和解构）、返回类型语法、struct/enum 字段、
     关联项、\`let\`/\`for\`/\`match\` 绑定、闭包参数；
  2. 校验约定命名风格与可识别的无角色编号占位参数；
  3. 对 >32/>40 字符、显式参数 ≥5、多个同类型位置参数、
     \`*_frame\`、\`from_parts\`、\`new_with_*\`、\`*_inner\`、
     \`supports_*\`、\`*_enabled\`、\`bool\` 模式参数、冗余
     \`Option\` 后缀等**只报语义审查候选**，不能直接决定改名；
  4. 输出 \`path:line:column\`、symbol、规则编号、\`error\`/\`warning\`/
     \`manual-review\`、原因；仅确定性违规或人工确认的语义违规才能阻断；
  5. 测试、宏、第三方实现、协议/FFI、工具生成、已有未触及命名必须
     具有明确作用域和豁免记录，不能静默忽略，也不能仅因名称长而阻断。
- **验收状态分离**：\`syntax-pass\`、\`ast-pass\`、\`semantic-reviewed\`
  分别报告；当前没有 AST 检查器时必须写
  \`ast-not-implemented / manually-reviewed\`，**禁止输出“全量自动通过”**。
  只有脚本真的存在并运行成功，才能在改动说明中宣称相应自动检查通过。

### 6.13 禁止机械重命名的验收流程

1. **先审调用语义再审名字**：读取原实现、所有调用方、trait 实现、
   类型定义、宏/FFI/序列化边界；写出“原职责 → 目标职责”和必须保持的
   状态、错误、返回值、线程/异步/资源生命周期、不变量。
2. **先复用后抽象**：工作区存在 \`TaskOptions::new(...)\`、
   \`create_task(options)\` 等正确入口时，应复用；**不通过**
   \`from_parts(...).create()\`、\`from_backend(...)\` 或
   \`supports_visualization()\` 等词汇替换掩盖相同位置参数、隐藏副作用、
   错误传播变化或“开关状态与能力”语义不一致。
3. **只触及本次工作范围**：不能因一次修改匹配到 100+ 个符号而批量改名。
   发现历史问题时以评审意见记录；只有此次明确要求全项目治理，
   才分模块、有证据地逐项处理。改动前的 1 个接口不应无理由变成
   3 个同义 wrapper、2 个冗余配置类型或层层转发。
4. **变更后验证**：检查受影响调用方、trait impl、泛型约束、UI 调用、
   错误分支、线程/回调、GPU 提交顺序及执行成本；做适用的编译和测试。
   单纯 \`cargo fmt\` 通过、长名称减少、正则检查通过都不足以证明语义正确。
5. **AI 的完成说明必须分开报告**：
   - 改了哪些标识符、为什么必须改、对应职责有何改善；
   - 保留了哪些名称、为什么符合 Rust 惯例或外部契约；
   - 强制项检查、人工语义审查、建议告警各有哪些结果；
   - 已执行与未执行的工具、剩余风险。**不得以重命名数量作为质量指标**。

## 7. 文档规则

- 中文是项目文档第一语言；crate `README.md` 为英文，必须有 `README.zh-CN.md`。
- `docs/` 只放稳定知识：架构、构建/测试、贡献、命名、ADR、crate 目录。
- 禁止新增：测试输出/benchmark 数据/截图/日志、改名历史/迁移映射、带日期的计划或草稿。
- 文档中的命令、版本、feature、路径必须与当前代码一致；修改后运行
  `pwsh -File scripts/check-docs.ps1`。

## 8. Git 与发布

- 未经用户明确要求，不要执行 `git push`、发布或部署操作。
- 提交遵循 Conventional Commits（`cog` 校验）；`CHANGELOG.md` 自动生成，不手工编辑历史。
- 一个 PR 只做一个逻辑变更；格式化、重命名、行为变更尽量分开。

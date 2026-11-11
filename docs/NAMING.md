# 命名规范

本规范统一 BMCBL 工作区的 crate、模块、公共/内部接口与文件命名。规则以
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html) 和
[wgpu](https://docs.rs/wgpu/latest/wgpu/) 的图形词汇为基准。

目标不是最短名称，而是让维护者只读名字就能确定：**动作、对象、所有权、返回类型和失败模式**。
测试函数名为了描述行为可以更长，不受 40 字符建议限制。

## 1. 分层词汇

| 概念 | nova-gfx（wgpu 对齐） | gpui | 禁止 |
| --- | --- | --- | --- |
| 图形设备 | `Device` / `Backend` | `NovaBackend` | `GfxDevice` |
| 提交 | `SubmissionDevice`、`SubmissionId` | `PendingSubmission` | `BackendQueue` |
| 交换链 | `Swapchain`、`SwapchainId` | `swapchain` 字段 | `backbuffer`（除 DXGI 原生语义） |
| 绑定资源 | `BindGroup`、`BindGroupLayout`、`BindGroupEntry` | 内部资源绑定 | 新 API 使用 `ResourceSet` |
| 绘制命令 | `RenderCommand::Draw(DrawCommand)` | `DrawCommand`（打包结果） | `RenderStep` / `draw step` |
| 一帧 | `PresentFrameDescriptor`、`render_and_present` | `PresentationPacket` | `RenderStepList` |
| 离屏 pass | `RenderPassCommands`、`render_to_textures` | `PreparedColorLayer` | `TextureRenderStepList` |
| 呈现设施 | `SurfaceCapabilities`、`PresentTimings`、`PresentedFrame` | `PresentReadiness` | `PresentationCapabilities` |
| 脏区域 | 边界参数 `present_region` | `DirtyRegion`、`damage` | gfx-core 内使用 `damage` |
| 保留层 | 不出现 | `retained`、`RetainedPaint` | nova-gfx 出现 `retained` |

## 2. 通用规则

1. **先语义，后形式**：`create_buffer`、`destroy_texture`、`write_texture`、`render_and_present`、
   `request_frame_ready` 是常见的动词 + 对象风格，但不是重命名模板。对 `handle_*`、
   `process_*`、`do_*` 等泛动词仅在确实模糊时提出审查建议，不得按词批量替换。
2. **谓词描述查询意图**：`is_*`、`has_*`、`can_*`、`should_*` 通常返回 `bool`；
   查询本身可能失败或状态未知时，`Result<bool, E>` / `Option<bool>` 也可能是正确契约，
   不得为了匹配命名形式而丢弃错误或未知状态。
3. **转换前缀**：`as_` 零成本借用；`to_` 分配/计算；`into_` 消耗 self；`from_` 构造。
   不要用 `get_` 修饰普通字段 getter；只有按 key/index 取值才用 `get`/`get_mut`。
4. **参数对象**：位置参数达到 5 个，或出现 2 个以上同类型参数时，使用
   `*Descriptor`/`*Options` 结构体。禁止 bool 模式参数，用 enum 或 descriptor 字段。
5. **精确优先于短小**：`*_and_*`、`*_with_*`、`*_for_*`、`*_if_needed`、
   `*_this_frame` 及长度超过约 40 字符只属于**人工审查信号**，不是禁令。
   `with` 可标识有意义的配置或约束，`and` 可表达不可分割的组合动作；不得为了
   消除匹配模式而丢失限定条件。仅在新名字同样准确且更清楚时才缩短名称。
6. **术语按概念区分**：同一概念保持一致，不同概念必须分别命名。
   `step`/`command`、`present`/`presentation`、`surface`/`swapchain`
   不是字面禁词；应核对真实类型、领域与后端语义，不得强行统一不同概念。
7. **缩写按单词处理**：`Dx12Device`、`OpenGlDevice`、`Dx11Surface`，不写 `DX12Device`、
   `OpenGLDevice`。
8. **可见性最小化**：先私有，再 `pub(crate)`/`pub(super)`，最后 `pub`；公共 API 必须有
   `///` 文档和 `# Errors`/`# Panics`。
9. **测试命名**：`行为_条件_期望结果`，允许长名；不要在产品代码里复制测试长句。
10. **参考专业项目，不自造词**：改名与新增 API 必须先对照同领域成熟项目的既有词汇，
    不凭语感拼新词。参照基准：

    | 层 / 领域 | 参考项目 | 直接沿用的词汇与形态 |
    | --- | --- | --- |
    | `nova-gfx` / `gfx-*` | [wgpu](https://docs.rs/wgpu/latest/wgpu/) | `*Descriptor` + `create_*`/`request_*`；`validate_*` + `*Error` 枚举；`Device::poll`、`PollType`、`SubmissionIndex`；`TextureDescriptor::array_layer_count`；`DeviceError::from_hal` 式 backend→通用错误映射；`dispatch_workgroups`、`transition_buffers` 等「动词 + 对象」 |
    | `gpui` / UI 侧状态 | [Qt](https://doc.qt.io/qt-6/properties.html) | 属性名 `enabled`/`visible`，访问器 `isEnabled()`/`hasFocus()` → Rust 落为 `is_enabled()`/`has_focus()`；`setEnabled()` → `set_enabled()`；`enabledChanged` 信号 → `*_changed` 事件 |
    | 任务 / 业务 API | JS/TS 生态 | 一个主入口 + 参数对象（`create(options)`、`update(id, update)`）；能力探测 `CSS.supports()` → `supports_*()`；`Option` 表达可选而非 `_with_*` 重载家族 |

    **参数对象只承载配置，不承担副作用**：`create_task(options)` 是唯一入口并返回
    `Result`；不得把注册动作藏进 `TaskOptions::create()` 之类的配置对象方法。
    `new(title, stage)` 是最小必要构造，其余可选字段用 builder（`.detail()`/`.total()`/
    `.parent()`），不得用 `from_parts(...)` 把一长串位置参数包一层。
    `from_*` 只表达「有明确来源的转换或构造」，不作为规避 `new_with_*` 限制的改名工具；
    实际执行初始化的构造用 `new(...)`，带配置的构造用 `with_*` 或 builder。
    增量、绝对值与元数据更新要在名字和 rustdoc 中区分（如 `ProgressUpdate::advance()`
    表示增量推进、`total()` 设置绝对上限、`stage()` 设置元数据）。

    **布尔命名按语义分三类，不强制统一前缀**（与第 2 条互补，不是推翻）：
    - 状态属性查询 → `is_*` / `has_*`（Qt `isEnabled` / `hasFocus`，Rust API Guidelines）。
      功能开关是否启用用 `is_*_enabled()`；`supports_*` 只用于真正的特性/能力探测，
      不要用它表达「是否启用」，避免语义混淆。
    - 能力 / 特性探测 → `supports_*`（JS/TS `CSS.supports()`）。
    - 可用性 / 状态形容词短语 → `*_enabled` / `*_available` / `*_supported` / `*_valid`。
    - 「动作 + bool 返回值」表示「是否改变 / 是否成功」（`set_*`、`remove_*`、`pause_*`）
      不受前缀约束。
    - 主谓倒装短语（`path_is_under_*`、`model_has_detail_*`）与比较动词
      （`eq`、`overlaps`、`contains`）视为合法布尔表达，不误报为缺谓词。

    后端专有实现词（如 DXGI `GetDeviceRemovedReason`、`VK_KHR_incremental_present` 的
    `damage`）只出现在专有实现里，不泄漏为通用接口名；通用错误映射用 `backend_error`
    一类领域词，不把平台 API 名拼进函数名。

## 3. 名称与契约审查

- 规则覆盖本次新增或实际修改的声明与绑定，包括内部函数、trait 方法、字段、局部变量、模式绑定和闭包参数；不只检查 public API。先读实现、类型和调用方，确认名称描述真实对象、阶段、所有权、错误和副作用。
- frame 只在实际指代明确帧概念时使用。按行为区分 CPU 准备、命令录制、提交批次、交换链图像、呈现帧、动画帧或网络帧；FrameId、SubmissionId、SwapchainImage、RenderPass 和 PresentedFrame 的含义不可混用。类型或模块已说明上下文时，不要机械追加 frame；也不要批量替换所有含 frame 的名字。
- Options、Descriptor 等类型只承载配置。业务动作由拥有相应权限和生命周期的服务执行。new/default 用于惯用构造；from_* 应表达真实来源转换，不能只为缩短名称而改名。改接口时保留 Option/Result、默认值、所有权、错误传播、状态更新语义和调用方可观察行为。
- 命名告警是审查线索，不是自动改名指令。仅处理本次范围及其必要调用链；历史问题先记录，不顺手批量重命名。检查外部 trait、平台实现、FFI、协议和序列化契约，不能只看最终名称是否变短。
- 工具只能证明其覆盖范围内的结果。命名脚本不是完整 Rust AST 检查器；未由工具覆盖的语义、局部绑定和宏/条件编译路径要明确标为人工审查，不能声称全量自动通过。

### 3.1 纯命名修改的前置条件（强制）

**纯命名修改不等于代码质量提升。** 每一个候选符号都必须经过以下核对，不满足时保留原名：

1. **确认事实**：阅读完整函数体、直接调用方、类型和返回值、rustdoc、测试、公开入口及模块上下文；
   关注 I/O、CPU 计算、缓存、副作用、错误、取消、数量限制、线程与资源所有权。仅凭 grep、
   截图或函数名无法确定语义时必须标记待审，不得给出确定性替换。
2. **证明更准确**：对照改名前后的动作、对象、数据来源、数量、过滤条件、默认行为、
   结果、错误、性能路径与副作用。若新名更泛化、暗示不存在的限制或丢失关键语义，
   即使缩短了也属于不合格改名。
3. **分析入口/实现关系**：`_inner` 并非禁词。公共 wrapper 与内部实现如仅机械转发，
   应判断是否能保留对外 API 并直接整合实现；不要直接把 `*_inner` 替换为 `collect_*`
   等模糊动词。泛型、trait、`cfg`、公共 API、多个调用方及依赖关系均需检查。
4. **保持契约**：纯改名不得顺便更改参数类型或顺序、`Option`/`Result`、默认值、
   可见性、条件、执行顺序、锁、缓存、取消、错误传播或性能；确需行为变更时必须作为
   独立的功能变更单独验证。
5. **控制范围**：只处理当前需求及必要引用。涉及多个模块、几十个文件或用户未提交
   的修改时，不得用正则批量套用新词，不得一并提交未经语义核验的代码。

### 3.2 动词与限定词的语义（强制）

| 词汇 | 实际语义 | 审查要点 |
| --- | --- | --- |
| `query_*` | 向数据源/存储发起查询，可以读取、筛选和构造结果 | 不因结果包含集合就改成 `collect` |
| `load_*` | 从明确来源加载数据或资源，通常存在读取/缓存路径 | 仅拼接已有对象的情况不宜使用 |
| `collect_*` | 核心操作是遍历已有数据并聚合成集合 | 不得掩盖主要的查询、哈希、解析或计算 |
| `fingerprint_*` / `hash_*` | 生成确定性的指纹或哈希值 | 不得因返回 `Vec` 就改为 `collect` |
| `scan_*` | 逐项扫描并发现/检查源数据 | 不是按已知 key 批量读取的同义词 |
| `*_many` / `*_batch` | 同级 API 中有意义的批量语义 | 上下文不区分单/多时可人工评估 |
| `*_inner` | 真实区分包装层和内部实现 | 不得无理由替换；先评估包装层职责 |
| `*_with_*`、`*_limit` | 限定条件、特殊版本或资源限制真正影响调用 | 可选参数未启用时，不得暗示总是受限 |
| `build_*`、`encode_*`、`write_*` | 构造、编码、写出各自的结果 | 不可与查询或收集互换 |

**具体反例（用于判断语义，并非批量重命名映射）：**

- `query_chunk_records_many_inner` 具有可选的 `max_actor_records`；
  `load_chunk_records_with_actor_limit` 会误导读者以为限制在所有调用中生效。
  首先应确认限制的具体粒度、默认行为和调用入口，再决定名称。
- 批量区块指纹的核心步骤是 XXH3-128 计算；
  `collect_chunk_record_fingerprints` 丢失了计算语义，并不是天然优于 `fingerprint_*`。
- 区域叠加查询涉及范围检查、计算史莱姆区块、加载区块记录、处理实体以及查询村庄；
  `collect_region_overlays` 过于泛化。仅转发的 `query_region_overlays_inner`
  包装层可以评估是否合并，但不能仅因 `inner` 后缀就机械改名。

### 3.3 AI 执行与验收（强制）

- 每次实际改名必须有 **旧符号 → 新符号 → 完整职责 → 更名理由 → 影响调用方**
  的逐项审查；删除 `inner`、`with`、`many`、`limit`、`async` 等限定词时，
  必须解释该词为何多余或不正确。无法证明改进就保留原名。
- 校验差异仅为命名与必要引用变动，再运行受影响的 rustfmt、`cargo check` 和
  有针对性的测试；涉及宏、feature、外部 API 或跨平台路径时扩大验证。
- 出现误导性名称应撤销那一处重命名并重新检查；编译通过不等于语义正确。
- 正则、命名扫描器、长度阈值、字符串替换均不能代替调用图与语义审查，
  不得生成无条件“修复全部命名问题”的改名清单。

## 4. 边界规则

- `nova-gfx` 不得出现 UI 词汇：`Window`（除 native surface target 的平台类型）、`ui`、
  `widget`、`scene`、`retained`、`damage`、`atlas`、`glyph`、`blur`。这些属于 gpui/gpui-3d。
- gpui 调用图形 API 时使用 `gfx-core` 能力 trait，不按 backend 分支调用固有方法；缺少的能力
  先加到 trait，再由各 backend 实现。
- backend 固有方法不作为公共契约：与 trait 重复的实现应删除或降为 `pub(crate)`，文档只
  宣传 trait。
- gpui 渲染器内部按动词族组织：`prepare_*`（CPU 侧准备）、`encode_*`（生成命令）、
  `submit_*`（提交）、`present_*`（呈现）、`sync_*`（资源同步）。避免 `handle_*`。

## 5. Rust 文件、模块和目录路径规范

本节约束 `src/`、`crates/*/src/` 及工作区自有 Rust 源码的**模块路径与物理文件布局**。
先读目标模块的 `mod` 声明、公开导出、实际职责和相邻模块；路径名是领域模型的一部分，
不能只凭字符串长度或文件大小批量改名。测试、平台适配、宏展开及代码生成目录仍需遵循
相应工具和 Rust 语言的真实路径契约。

### 5.1 强制规则（新增和本次实际修改的代码）

1. **拼写与一一对应**：Rust 模块、文件名（不含 `.rs`）和目录名采用小写
   `snake_case`，模块声明 `mod foo;` 与实际文件 `foo.rs` 或 `foo/mod.rs` 对应。
   crate 的 Cargo 包名可用 `kebab-case`，不得把包名拼写强行复制到 Rust 模块名。
   Rust 规定的 `lib.rs`、`main.rs`、`mod.rs`、`build.rs` 等入口为合法特例；
   FFI、生成源码及 `#[path]` 的确切文件名以其显式契约为准。
2. **文件路径必须表达模块树**：例如 `src/item/format.rs` 对应 `item::format`。
   同一模块不得同时使用 `foo.rs` 和 `foo/mod.rs` 两个入口。不要为了缩短字符串而
   滥用 `#[path]`、绕过模块层级或制造文件系统与 Rust API 不一致的隐式映射。
3. **先定义所有者，再划分目录**：目录按稳定的领域/职责/生命周期边界组织，而不是
   按 `struct`、`enum`、`fn`、`utils`、`misc`、`common`、`new` 或开发阶段机械分类。
   同一份核心状态、协议实体或资源的所有权只能有一个明确位置；跨层共享应通过明确
   的公共模块/API，而不是复制实现或建立循环依赖。
4. **避免无信息量的路径重复**：同一概念已由父目录提供时，子文件名不应无意义地再
   拼接相同的领域名，例如在单一物品历史转换模块中反复构造 `*_saved_item.rs`。
   **但不能机械去词**：当 `saved_item`、`block_state`、`scene_view` 等是区分
   不同数据模型的完整领域术语时，保留该术语。必须通过职责、同级文件以及最终
   `crate::...` 模块路径检查是否真的重复；仅凭重复词片段不得判为违规。
5. **格式/版本/平台限定词必须有真实含义**：`classic`、`medieval`、`modern` 只有
   在明确对应持久化格式或历史语义时可用；`dx12`、`vulkan`、`windows`、`linux`
   只放在真正的专有实现边界。禁止以 `new_`、`old_`、`temp_`、`final_`、
   `copy_`、`v2_`、`v3_`、`dev_`、`fix_` 等名称表达开发迭代阶段。
   若 `v3` 是线协议/磁盘格式的**不可省略的外部身份**，经核实后可保留；
   不得把它等同于内部代码的第三版实现。
6. **模块族必须采用一致维度**：同一组文件要么按领域实体、要么按操作阶段、要么按
   协议/目标格式组织；禁止同级混合多套互相重叠的分类，令维护者无法判断新功能放哪。
   每个新文件必须有单一主要职责和明确拥有者；文件不因“超过 N 行”自动拆分，
   也不因很短就必须与其他文件合并。
7. **改路径时维护完整依赖契约**：同步核对 `mod`/`pub mod`、`pub use`、
   `use`、`#[path]`、`include!`、`include_str!`、`include_bytes!`、Cargo 配置、
   `cfg`/feature、构建脚本、生成代码、测试、文档链接及跨 crate 调用。
   `pub` 的对外路径变化必须显式审查稳定 API 和破坏性影响，不能用大量兼容别名
   掩盖边界错误。仅大小写变化的文件名应检查 Windows/git 大小写处理。
8. **不修改运行时数据路径作为源码整理的副作用**：Minecraft 存档键、
   `premium_cache`、`mods`、资源包/插件约定目录、用户配置和协议字符串不是
   Rust 模块路径；不能用源码 `snake_case` 规则改写它们。路径拼装与文件名校验
   应遵守其数据格式、平台和安全边界。

### 5.2 需要人工审查的建议（不作为自动改名指令）

- **优先看完整路径，而非孤立文件名**：若 `item` 模块已经专门承载保存物品的
  历史转换，`item/classic.rs`、`item/medieval.rs`、`item/modern.rs`
  可能比 `item/classic_saved_item.rs` 等更清楚；若 `item` 还包含其他 Classic
  领域职责，则优先考虑按共同上下文组织为 `item/saved/classic.rs`、
  `item/saved/medieval.rs`、`item/saved/modern.rs`（仅为候选布局）。
  必须核对 `saved_item_history`、`format_evidence`、`modern_target` 等
  的真实职责，不能因为看见 `item` 就批量剥离 `saved_item`。
- **模块入口优先采用现代扁平形式**：新模块族通常使用 `foo.rs` + `foo/bar.rs`；
  `foo/mod.rs` 仍是 Rust 支持的合法布局，已有模块无需仅为风格统一而迁移。
  不要同时创建 `foo.rs` 与 `foo/mod.rs`。选择布局时考虑目录导航和仓库既有习惯。
- **名字描述核心对象/能力，不描述实现细节**：需要区分解析、校验、转换、写入、
  存储或渲染时可使用 `parser`、`validation`、`conversion`、`writer`、
  `storage`、`renderer` 等真实职责词；不是所有模块都必须带后缀。
  `impl`、`helpers`、`internal` 等只有在职责无法通过领域语义表达且确有边界时才保留。
- **模块深度按认知成本决定**：优先少量稳定的领域层次，不设机械的最大目录深度。
  若缩短路径会使模块用途模糊，宁可保持必要限定词；若目录只是重复父目录，不新建层级。
- **同一文件内的内部类型名与模块名可共用领域词**：模块路径可以短，公开类型仍需
  可读，例如 `item::classic::ClassicSavedItemCheckReport`。不要强求类型名也一起缩短。

### 5.3 修改前的人工验收

1. 记录当前物理路径、Rust 模块路径、主要公开符号、调用方与真正职责。
2. 给出最多两个备选布局，比较重复词、歧义、所有权和公开 API 成本；
   不以“更短”作为唯一理由。
3. 仅调整被本次需求涉及的模块族，更新真实引用，并按影响范围执行
   rustfmt、编译或文档检查；只改规则文档时不执行 Rust 编译。
4. **自动化边界**：脚本可阻止非法字符、非 `snake_case` 或冲突入口，
   对语义重复、目录合理性、历史术语和公共 API 影响只能报告候选并人工复核。
   禁止自动批量改名、自动移动文件或生成同义兼容层来“满足命名检查”。

### 5.4 非 Rust 源码与文档

- crate 文档：`README.md`（英文）+ `README.zh-CN.md`（中文），成对维护。
- 集中文档：`docs/UPPER_SNAKE.md`（如 `ARCHITECTURE.md`、`BUILD_AND_TEST.md`），
  英文镜像使用 `*.en.md`。
- ADR：`docs/adr/NNNN-kebab-case.md`，按决策顺序编号，不按日期命名。
- 不使用 `*-audit.md`、`*-migration.md`、`*-rename*.md`、`*-todo.md` 作为长期文档；
  迁移与待办放在 PR/issue。测试输出、benchmark 原始数据、截图和日志不进入 `docs/`。

## 6. 评审检查清单

- [ ] 新名称准确表达真实动作、对象、限制、返回值和副作用，不比旧名更模糊。
- [ ] 公共 API 没有 `get_` 前缀（除 keyed lookup）。
- [ ] 没有 bool 模式参数；参数 ≥5 时使用 descriptor。
- [ ] `inner`、`with`、`legacy`、`compat`、`limit` 等限定词的增删已经逐项说明真实语义依据。
- [ ] 同一概念在本次改动中只使用一个术语。
- [ ] 新增 trait 方法同步更新所有 backend 与文档。
- [ ] 新增/移动 Rust 模块的文件名、模块路径、归属职责与 `mod` 声明对应，无无意义的路径词重复。
- [ ] 测试名描述行为，不泄漏实现细节。

## 7. 自动化

```powershell
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
pwsh -File scripts/check-docs.ps1
rustfmt --edition 2024 <改动文件...>
```

自动命名检查只能阻止**确定性违规**；长名、`with`、`and` 等模式是人工审查提示，
不能作为自动改名清单或强制失败项。允许清单仅包含已审查的真实契约例外。

当前 `check-naming.ps1` 扫描部分 nova-gfx、GPUI Nova 的公开函数名，
并对部分 GPUI 名称提供提示；它不是 Rust AST 检查器、不覆盖所有工作区源码，
也不能判断 query/load/collect 是否准确。零警告不代表语义审查通过；
rustfmt、编译器和命名脚本各自只验证实际覆盖的内容。

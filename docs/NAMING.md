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

1. **动词 + 对象**：`create_buffer`、`destroy_texture`、`write_texture`、`render_and_present`、
   `request_frame_ready`。避免 `handle_*`、`process_*`、`do_*` 这类泛动词。
2. **谓词返回 bool**：`is_*`、`has_*`、`can_*`、`should_*`，不得返回 `Result`/`Option`。
3. **转换前缀**：`as_` 零成本借用；`to_` 分配/计算；`into_` 消耗 self；`from_` 构造。
   不要用 `get_` 修饰普通字段 getter；只有按 key/index 取值才用 `get`/`get_mut`。
4. **参数对象**：位置参数达到 5 个，或出现 2 个以上同类型参数时，使用
   `*Descriptor`/`*Options` 结构体。禁止 bool 模式参数，用 enum 或 descriptor 字段。
5. **避免链式长句**：公共/内部接口不使用 `*_and_*`（原子操作除外）、`*_with_*`（builder/
   参数对象优先）、`*_for_*`、`*_if_needed`、`*_this_frame`。名称超过约 40 字符必须能在
   评审中说明“为什么不能拆”。
6. **单一术语**：同一概念只保留一个词。禁止 `step`/`command` 混用、`present`/`presentation`
   混用、`surface`/`swapchain` 混用。
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

- [ ] 新名称读完能回答“动作 + 对象 + 返回类型”。
- [ ] 公共 API 没有 `get_` 前缀（除 keyed lookup）。
- [ ] 没有 bool 模式参数；参数 ≥5 时使用 descriptor。
- [ ] 没有新增 `*_compat`、`*_legacy`、`*_old` 名称。
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

允许清单只包含已评审的例外；新增例外需要在评审中说明理由，不能用来绕过规则。

当前 check-naming.ps1 逐行检查部分 nova-gfx 与 GPUI Nova 路径中的公开函数名，并对部分 GPUI 名称给出提示；它不解析 Rust AST，也不覆盖整个工作区的局部绑定和所有声明。rustfmt、编译器与该脚本各自只验证其实际覆盖的内容。

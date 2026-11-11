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

## 3. 边界规则

- `nova-gfx` 不得出现 UI 词汇：`Window`（除 native surface target 的平台类型）、`ui`、
  `widget`、`scene`、`retained`、`damage`、`atlas`、`glyph`、`blur`。这些属于 gpui/gpui-3d。
- gpui 调用图形 API 时使用 `gfx-core` 能力 trait，不按 backend 分支调用固有方法；缺少的能力
  先加到 trait，再由各 backend 实现。
- backend 固有方法不作为公共契约：与 trait 重复的实现应删除或降为 `pub(crate)`，文档只
  宣传 trait。
- gpui 渲染器内部按动词族组织：`prepare_*`（CPU 侧准备）、`encode_*`（生成命令）、
  `submit_*`（提交）、`present_*`（呈现）、`sync_*`（资源同步）。避免 `handle_*`。

## 4. 文件与文档命名

- crate 文档：`README.md`（英文）+ `README.zh-CN.md`（中文），成对维护。
- 集中文档：`docs/UPPER_SNAKE.md`（如 `ARCHITECTURE.md`、`BUILD_AND_TEST.md`），
  英文镜像使用 `*.en.md`。
- ADR：`docs/adr/NNNN-kebab-case.md`，按决策顺序编号，不按日期命名。
- 不使用 `*-audit.md`、`*-migration.md`、`*-rename*.md`、`*-todo.md` 作为长期文档；
  迁移与待办放在 PR/issue。测试输出、benchmark 原始数据、截图和日志不进入 `docs/`。

## 5. 评审检查清单

- [ ] 新名称读完能回答“动作 + 对象 + 返回类型”。
- [ ] 公共 API 没有 `get_` 前缀（除 keyed lookup）。
- [ ] 没有 bool 模式参数；参数 ≥5 时使用 descriptor。
- [ ] 没有新增 `*_compat`、`*_legacy`、`*_old` 名称。
- [ ] 同一概念在本次改动中只使用一个术语。
- [ ] 新增 trait 方法同步更新所有 backend 与文档。
- [ ] 测试名描述行为，不泄漏实现细节。

## 6. 自动化

```powershell
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
pwsh -File scripts/check-docs.ps1
rustfmt --edition 2024 <改动文件...>
```

允许清单只包含已评审的例外；新增例外需要在评审中说明理由，不能用来绕过规则。

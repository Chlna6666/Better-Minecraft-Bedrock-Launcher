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

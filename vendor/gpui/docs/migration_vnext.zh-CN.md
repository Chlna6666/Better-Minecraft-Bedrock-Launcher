# GPUI vNext 迁移说明

[English](migration_vnext.md)

本文记录 vendored GPUI 在当前仓库中的 vNext 分阶段迁移状态。内容以当前代码现状为准，并且明确区分“已经完成的结构拆分”和“尚未落地的生命周期/API 变更”。

## 目标

GPUI vNext 的核心目标有三项：

- 降低框架大文件的结构耦合；
- 为显式帧生命周期、layout/style cache 优化预留空间；
- 让 BMCBL 可以按阶段迁移，而不是一次性混合重写。

## 当前阶段

当前仓库处于第一阶段：

1. 结构拆分；
2. 行为不变；
3. 现有公开 API 仍可编译；
4. 还没有启用新的 vNext 生命周期。

## 已完成的结构工作

以下区域已经拆成正常 Rust 模块：

- `element.rs` 现在作为 `render_pipeline.rs` 与 `render_pipeline/*` 的 facade；
- `elements/div.rs` 现在作为 `elements/div/*` 的 facade；
- `style.rs` 现在作为 `style/*` 的 facade；
- `layout.rs` 现在作为 `layout/*` 的 facade。

`elements/div/` 当前已经拆分出：

- `element.rs`
- `state.rs`
- `style_state.rs`
- `scroll.rs`
- `tooltip.rs`
- `drag_drop.rs`
- `inspector.rs`
- `event.rs`
- `event_handlers.rs`
- `event_runtime.rs`

当前仓库也已经加入第一层布局体验辅助：

- `layout_builders.rs`
- `ParentElement::child_if`
- `ParentElement::child_some`
- `ParentElement::children_array`
- `ParentElement::extend_any`

## 尚未实现的内容

以下 vNext 项目仍然只是计划，当前代码不应声称已经完成：

- 显式的 `prepare/layout/prepaint/paint` 帧上下文；
- layout-only style fingerprint 拆分；
- 迁移计划里提到的 retained layout cache 优化；
- 兼容层删除；
- BMCBL UI 对未来 breaking API 的正式迁移。

## 验证规则

每一步结构调整都必须保证以下命令通过：

- `rtk cargo check -p gpui`
- `rtk cargo check -p gpui --examples`
- `rtk cargo check`

如果 workspace 里原本就存在 warning 或与 GPUI 无关的失败，应单独记录，不能混进 GPUI 迁移状态。

## 后续阶段

结构拆分稳定后，后续顺序保持不变：

1. 增加布局体验辅助 API；
2. 引入显式 element 生命周期；
3. 优化 style/layout 热路径与指标；
4. 分批迁移 BMCBL UI；
5. 删除兼容层并补全文档。

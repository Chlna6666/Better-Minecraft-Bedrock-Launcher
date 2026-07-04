# Style 与 Layout 性能

[English](style_layout_performance.md)

本文记录 GPUI vNext 中 style / layout 性能优化的计划方向。

## 当前状态

当前仓库已经完成以下结构分离：

- `style/*`
- `layout/*`
- `elements/div/style_state.rs`
- `style/layout_style.rs` 占位骨架

在 `style/*` 中，`Display`、`Overflow`、`FlexDirection`、`Position`
这类布局相关枚举仍然归属于“样式语义层”，而不是“布局引擎层”。

这意味着代码结构已经为后续性能工作做好准备，但更完整的 vNext style/layout cache 变更还没有全部实现。

## 计划中的优化方向

后续工作包括：

- 统计 style refinement 次数；
- 统计 style 到 layout 转换次数；
- 区分 layout-only style 与 paint-only style；
- 提高 retained layout cache 复用率；
- 减少 retained subtree 比较中的额外分配。

## 后续验证方向

后续版本应补上这些指标的前后对比：

- style refinement 次数；
- layout conversion 次数；
- layout cache hit rate；
- retained subtree 复用行为。

在当前阶段，`LayoutStyle` 已经不再包含 `visibility`，因为它仍然更偏向
paint 语义，而不是 layout 语义。

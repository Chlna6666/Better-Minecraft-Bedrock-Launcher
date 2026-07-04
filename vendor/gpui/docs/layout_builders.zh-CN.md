# 布局构建器

[English](layout_builders.md)

本文记录 GPUI vNext 中布局构建器 API 的当前状态。

## 当前状态

当前仓库已经暴露这些顶层辅助 API：

- `v_stack()`
- `h_stack()`
- `center()`
- `absolute_fill()`
- `relative_fill()`

这些 helper 只是对现有 `div()` 与样式 API 的薄封装，不改变底层布局模型。

## ParentElement 辅助

`ParentElement` 现在也包含几类批量拼装辅助：

- `child_if(...)`
- `child_some(...)`
- `children_array(...)`
- `extend_any(...)`

## 计划方向

这些 helper 的目标是提升调用侧可读性，尤其是减少 UI 代码里反复出现的 flex / position 链式组合。

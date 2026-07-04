# Element 生命周期

[English](element_lifecycle.md)

本文说明当前 GPUI 的 element 生命周期，以及计划中的 vNext 方向。

## 当前生命周期

当前框架仍然使用现有三段流程：

1. request layout；
2. prepaint；
3. paint。

最近的结构拆分已经把生命周期相关代码移动到 `render_pipeline/*`，但还没有把当前 API 切换到计划中的 vNext typed frame context。

## 计划中的 vNext 方向

目标模型仍然是：

```rust
trait Element {
    type State;

    fn prepare(&mut self, cx: &mut PrepareCx) -> Self::State;
    fn layout(&mut self, state: &mut Self::State, cx: &mut LayoutCx) -> LayoutId;
    fn prepaint(&mut self, state: &mut Self::State, cx: &mut PrepaintCx);
    fn paint(&mut self, state: &mut Self::State, cx: &mut PaintCx);
}
```

但这套接口目前还没有在当前代码库中真正启用，仍然属于后续阶段的迁移目标。

## 价值

计划中的拆分主要是为了：

- 防止 layout 阶段注册只属于 paint 的行为；
- 防止 paint 阶段反向改动 layout 状态；
- 在整帧内共享一份 frame-local state；
- 为后续 cache 优化提供更清晰的边界。

## 当前结构占位

当前仓库已经加入 `render_pipeline/context.rs`，其中包含占位性质的
`PrepareCx`、`LayoutCx`、`PrepaintCx`、`PaintCx` 类型。

这些类型目前还不是生效中的 element API，只是先把未来生命周期边界以
结构形式固定下来。

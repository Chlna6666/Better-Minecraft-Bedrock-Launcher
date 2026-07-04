# Div 交互结构

[English](div_interactivity.md)

本文说明当前 `Div` 在“行为不变的结构拆分”完成后的内部组织方式。重点是记录现在代码实际长什么样，而不是提前声称后续 vNext 生命周期重写已经完成。

## 对外 API

`gpui::Div` 仍然保留熟悉的 fluent API：

- `.on_click()`、`.on_mouse_down()` 等鼠标事件；
- focus / hover 辅助；
- scroll 跟踪辅助；
- tooltip 与 drag/drop 辅助。

调用侧仍然按原样导入 `InteractiveElement` 和
`StatefulInteractiveElement`。

## 当前内部拆分

现在的实现已经按职责拆开：

- `elements/div/element.rs`：`Div` 包装和 child 组合；
- `elements/div/state.rs`：`Interactivity` 数据和帧入口；
- `elements/div/style_state.rs`：computed style 与 group-hover 辅助；
- `elements/div/scroll.rs`：scroll handle 状态与 scroll 运行时逻辑；
- `elements/div/tooltip.rs`：tooltip 生命周期辅助；
- `elements/div/drag_drop.rs`：drag/drop 载荷结构；
- `elements/div/inspector.rs`：debug 与 inspector 状态；
- `elements/div/event.rs`：公开事件 trait；
- `elements/div/event_handlers.rs`：listener 注册与回调存储；
- `elements/div/event_runtime.rs`：运行时绑定以及 click/drag 状态流转。

## 状态模型

`Interactivity` 仍然是以下数据的单一所有者：

- element identity 与 focus 状态；
- hover / active / group style refinement；
- listener 注册；
- scroll 与 tooltip handle；
- drag/drop 与 click 状态。

这次结构拆分没有改变这些字段的归属，只是把读写这些字段的代码移动到了更窄的职责模块里。

## 当前限制

当前拆分仍然建立在现有 GPUI 帧模型上：

- request layout；
- prepaint；
- paint。

未来显式的 `prepare/layout/prepaint/paint` vNext 生命周期仍属于后续阶段，目前还不是生效中的 API。

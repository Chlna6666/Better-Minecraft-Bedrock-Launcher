# 上下文与实体

[English](contexts.md)

GPUI state 存在于 entities 中，并通过 context objects 访问。渲染、事件处理、异步
任务和测试都应使用与当前操作匹配的最窄 context。

## `App`

`App` 是根上下文。用它来：

- 通过 `cx.new` 创建 entities；
- 打开和管理 windows；
- 注册 actions、menus 和 key bindings；
- 设置 globals、assets 和 HTTP client；
- spawn 前台或后台任务；
- 当 window-level 代码需要触发渲染时，按 entity id 发出 notify。

prelude 会导入 extension-style methods 所需的 traits，因此大多数应用代码可以导
入 `gpui::prelude::*`，而不是直接命名这些 traits。

## `Context<T>`

`Context<T>` 会在创建、更新、渲染或监听 `Entity<T>` 事件时提供。使用当前闭包中
传入的 `Context<T>`。如果嵌套 read 或 update 闭包收到新的 context，不要继续使
用外层 `cx`。

常用操作：

- `cx.entity()` 返回当前 `Entity<T>`。
- `cx.weak_entity()` 返回 `WeakEntity<T>`。
- `cx.notify()` 为当前 entity 安排 observers 和 rendering。
- `cx.listener(...)` 把 element callback 适配成可以更新当前 entity 的 callback。
- `cx.spawn(async move |handle, cx| ...)` 启动前台异步任务，之后可以升级 weak
  entity handle。

## `Window`

`Window` 是显式参数。用它处理：

- focus 和 tab navigation；
- input state 以及 mouse 或 keyboard handlers；
- 从 focused element tree 发起 action dispatch；
- frame requests 和 next-frame callbacks；
- layout、paint 和自定义 element state；
- image cache scope；
- GPU extension point 创建与绘制。

同时存在 `window` 和 `cx` 时，惯例参数顺序是 `window` 在 `cx` 前面。

## `Entity<T>` 与 `WeakEntity<T>`

`Entity<T>` 是 GPUI-owned state 的强 handle：

- `entity.read(cx)` 返回 `&T`；
- `entity.read_with(cx, |state, cx| ...)` 返回闭包结果；
- `entity.update(cx, |state, cx| ...)` 修改 state；
- `entity.update_in(cx, |state, window, cx| ...)` 在带 window 的 context 中修改；
- `entity.downgrade()` 返回 `WeakEntity<T>`。

`WeakEntity<T>` 可以避免 ownership cycles，适合 detached tasks、subscriptions，
以及 entity 被 drop 后应优雅失败的 callbacks。

不要在 entity 已经处于 update 中时递归 update 同一个 entity。应让一次 entity
update 拥有 mutation，然后在 update 返回后 notify observers 或 dispatch 后续工
作。

## Events 与 Subscriptions

会发出事件的 entities 实现 `EventEmitter<Event>`。更新 entity 时调用
`cx.emit(event)`。

其他 entities 使用 `cx.subscribe(entity, |this, emitter, event, cx| { ... })` 订
阅。把返回的 `Subscription` 存在 owner entity 中，这样 owner drop 时 subscription
也会被移除。

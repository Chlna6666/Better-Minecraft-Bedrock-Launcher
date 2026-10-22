# gpui-hooks

面向 GPUI 的 React 风格 hooks。


## 目录结构

- `src/lib.rs`: crate 入口与导出。
- `src/element.rs`: 兼容层 `HookedElement`、`HookedRender`、`execute_hooked_render`。
- `src/hooks/mod.rs`: hooks 总入口。
- `src/hooks/dependency.rs`: 依赖比较抽象。
- `src/hooks/storage.rs`: hook 存储与 render 周期控制。
- `src/hooks/use_state.rs`: `UseStateHook`
- `src/hooks/use_memo.rs`: `UseMemoHook`
- `src/hooks/use_effect.rs`: `UseEffectHook`
- `src/hooks/use_ref.rs`: `UseRefHook`
- `src/hooks/use_callback.rs`: `UseCallbackHook`
- `src/hooks/use_reducer.rs`: `UseReducerHook`
- `../gpui-hooks-macros`: `#[hook_element]` 属性宏

## 快速开始

```rust
use gpui::prelude::*;
use gpui::*;
use gpui_hooks::{hook_element, hook_render};
use gpui_hooks::hooks::{UseEffectHook, UseMemoHook, UseStateHook};

#[hook_element]
pub struct CounterView;

#[hook_render]
impl Render for CounterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.use_state(|| 0_u32);
        let double_count = self.use_memo(|| count.with(|value| value * 2), [count.get_cloned()]);

        self.use_effect(
            || {
                Some(Box::new(|| {
                    // cleanup
                }))
            },
            [count.get_cloned()],
        );

        div()
            .child(format!("count = {}, double = {}", count.get_cloned(), *double_count))
            .on_click(cx.listener(move |_, _, _, cx| {
                count.update(|value| value + 1);
                cx.notify();
            }))
    }
}
```

## `hook_element`

`#[hook_element]` 会做三件事：

1. 为结构体注入 hook 存储字段。
2. 实现 `gpui_hooks::hooks::HasHooks`。

## `hook_render`

`#[hook_render]` 应用于 `impl gpui::Render for YourView`，会在真实的 `Render::render`
前后自动插入 hook 生命周期管理。

这样你写的仍然是标准 GPUI `Render` 实现，不需要额外实现 `HookedRender`。

### 构造方式

- 无字段 struct：可直接用 `Type::default()` 或 `Type::new_hooked()`。
- 有字段 struct：使用 `Type::new_hooked(...)` 构造。

当前 `hook_element` 支持：

- 命名字段 struct
- unit struct

暂不支持 tuple struct。

## 可用 hooks

### `UseStateHook`

```rust
let value = self.use_state(|| 1_u32);
```

- `value.get_cloned()` 返回当前值的克隆。
- `value.with(|value| ...)` 以引用方式读取，避免不必要复制。
- `value.set(next)` 或 `value.update(...)` 更新状态。
- 更新后通常仍需要你自己 `cx.notify()`。

### `UseMemoHook`

```rust
let memo = self.use_memo(|| expensive(), [dep_a, dep_b]);
let value = &*memo;
```

- 依赖未变化时不会重新计算。
- 返回 `MemoHandle<T>`，读取只克隆 `Rc<T>`，不会复制整个值。

### `UseEffectHook`

```rust
self.use_effect(
    || {
        Some(Box::new(|| {
            // cleanup
        }))
    },
    [dep],
);
```

- 首次 render 执行 effect。
- 依赖变化时先执行旧 cleanup，再执行新 effect。
- hook 被移除或 `cleanup_effects()` 时也会执行 cleanup。

### `UseRefHook`

```rust
let cache = self.use_ref(|| String::new());
cache.borrow_mut().push_str("hello");
```

- 适合保存不直接驱动渲染的可变引用状态。

### `UseCallbackHook`

```rust
let callback = self.use_callback(|| 42_u32, [dep]);
let answer = callback.invoke();
```

- 依赖不变时保持稳定 callback。
- 返回 handle，不再每次 render 分配 `Box<dyn Fn()>`。

### `UseReducerHook`

```rust
let count = self.use_reducer(
    |state: &u32, action: u32| state + action,
    || 0_u32,
);
```

- 比 `use_state` 更适合一组明确的状态变换规则。
- `count.dispatch(action)` 分发更新。
- `count.with(...)` / `count.get_cloned()` 读取当前值。

## 比参考实现更强的部分

这版实现比简单的“按顺序存 Vec”多做了几件事：

- render 结束时会检查 hook 数量是否变化，防止条件 hook 静默污染状态。
- 当 hook 数量减少时，会自动截断多余 hook，并触发 effect cleanup。
- `use_effect` 支持 cleanup，并且在依赖变化和 hook drop 时都会正确执行。
- 额外提供了 `UseReducerHook`，便于复杂状态更新。
- `use_state` / `use_memo` / `use_callback` / `use_reducer` 改成轻量 handle，避免每次 render 分配 getter/setter 闭包。
- 依赖存储使用小容量栈缓冲优先的结构，常见小依赖列表可减少堆分配。

## 性能说明

当前设计的主要成本来自三部分：

1. hook slot 使用 `Box<dyn Any>` 做类型擦除，render 期间只保留一次 downcast。
2. 带依赖的 hook 需要逐项比较依赖。
3. 状态更新时仍会创建新的 `Rc<T>` 快照；这比每次 render 复制整个值更便宜，但不是零成本。

实际使用建议：

- 大对象状态优先用 `with(...)` 读取，不要默认 `get_cloned()`。
- 依赖列表保持小而稳定。
- 把昂贵计算放在 `use_memo`，不要放在 render 主路径反复跑。

## 约束

- hook 调用顺序必须稳定。
- 状态更新后需要调用方自己决定是否 `cx.notify()`。
- `hook_element` 目前还不支持 tuple struct。

## 后续可继续增强

- `use_async`
- `use_interval`
- `use_timeout`
- `use_context_selector`
- 支持带参数 callback hooks

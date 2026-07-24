# 示例

[English](examples.md)

GPUI examples 应保持小而当前，并能在 active platform 的标准 example check 下编译。

## API 形态

使用当前 API surface：

- `Application::new().run(|cx: &mut App| ...)`；
- `cx.open_window(options, |window, cx| cx.new(|cx| View { ... }))`；
- `impl Render for View { fn render(&mut self, window: &mut Window, cx:
  &mut Context<Self>) -> impl IntoElement { ... } }`；
- state handles 使用 `Entity<T>` 和 `WeakEntity<T>`；
- 需要修改当前 entity 的 event handlers 使用 `cx.listener(...)`；
- 前台异步任务使用 `cx.spawn(async move |cx| ...)` 或
  `cx.spawn(async move |handle, cx| ...)`。

不要演示旧名称，例如 `Model<T>`、`View<T>`、`ModelContext<T>`、`WindowContext` 或
`ViewContext<T>`。

## Dependencies

尽量使用 GPUI 已导出的 dependencies。图片示例应使用 `gpui::http_client` 设置 HTTP
client，避免引用不存在的 crates。

示例明确属于某个领域时，可以使用 GPUI 声明的 dev-dependencies，例如用于模型加载
的 `tobj`，或用于 GPU buffers 的 `bytemuck`。

## Platform Guards

依赖平台专用 API 的示例必须：

- 使用 `cfg` guard implementation；
- 为不支持的 targets 保留小型 fallback `main`；
- 不让 `cargo check --examples` 在不支持的平台失败；
- 在 example output 或 docs 中说明 active platform。

## Error Handling

示例可以把 startup failures 打印到 stderr，但不应隐藏导致示例不可用的错误。window
creation 和 setup paths 优先使用 `if let Err(error) = ...`。library-style helper
functions 应返回 `Result`。

## Visual Scope

示例应聚焦 GPUI framework capability。不要把 product routes、application launch
policy、default backgrounds 或 application-specific window chrome 加到 GPUI examples。

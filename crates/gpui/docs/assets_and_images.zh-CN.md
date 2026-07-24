# Assets 与图片

[English](assets_and_images.md)

GPUI 可以加载 local assets、通过配置的 HTTP client 加载 remote images，以及处理应
用自己拥有的 image resources。

## Asset Sources

应用或示例需要 packaged assets 时，实现 `AssetSource`。`load` 为路径返回 bytes，
`list` 返回子项名称。通过 `Application::with_assets(...)` 注册 assets。

asset paths 应相对于 asset source。不要在 framework code 中写入应用专用 absolute
paths。

## HTTP Images

Remote `img(...)` sources 使用 app HTTP client。通过
`Application::with_http_client(...)` 或 `cx.set_http_client(...)` 设置。

示例应使用 GPUI 导出的 HTTP client。如果示例不打算执行网络 IO，使用
`gpui::http_client::BlockedHttpClient`，让 dependency surface 保持显式。

## Image Caches

当 image lifetime 需要 scoped 管理时，使用 image cache elements 和 providers：

- `image_cache(provider).child(...)` 把 provider 限定到 element subtree。
- `RetainAllImageCache` 会保留 images，直到 clear 或 drop。
- 自定义 `ImageCacheProvider` 可以从 window-local element state 构建 caches。

cache evict image 时，如果有 current window，通过 `cx.drop_image(image,
Some(window))` drop；release cleanup 中使用 `cx.drop_image(image, None)`。

## Guidelines

- 不要把 decoding 和 cache mutation 放在 render-only code 中。
- 可能阻塞的工作使用 `background_spawn` 或 cache loader tasks。
- 异步 image load 完成后，在下一帧 notify owning entity。
- 会重复加载 remote images 的示例要限制 cache size。
- missing assets 和 HTTP failures 应通过示例可见状态或日志暴露，不要 panic。

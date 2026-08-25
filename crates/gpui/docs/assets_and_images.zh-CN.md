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
- `BoundedImageCache` 限制保留的 item 数量和 decoded bytes。
- 自定义 `ImageCacheProvider` 可以从 window-local element state 构建 caches。

cache evict image 时，如果有 current window，通过 `cx.drop_image(image,
Some(window))` drop；release cleanup 中使用 `cx.drop_image(image, None)`。

## 动态图片

GIF、APNG 和动画 WebP 共用同一套播放调度。GPUI 保留文件内部的 frame delay，仅在
帧间隔短于配置的播放上限时延长它。默认上限为 90 FPS；应用可在显示设备和工作负载
允许时显式设置更高的有限 `AnimatedImageConfig::max_fps`。零值或无效时序使用配置的
最小帧间隔。

流式动画同时限制预取 frame 数和 bytes。队列为空时允许首个大帧超过 byte limit，确保
大尺寸动画仍可前进；后续预取保持有界。

## Guidelines

- 不要把 decoding 和 cache mutation 放在 render-only code 中。
- 可能阻塞的工作使用 `background_spawn` 或 cache loader tasks。
- 异步 image load 完成后，在下一帧 notify owning entity。
- 会重复加载 remote images 的示例要限制 cache size。
- missing assets 和 HTTP failures 应通过示例可见状态或日志暴露，不要 panic。

# Image Cache

[English](image_cache.md)

GPUI image caches 控制 decoded images 和 GPU image resources 的 scope、retention
和 eviction。

## Built-In Scope

使用 `image_cache(provider).child(...)` element 把 image cache 行为限定到 subtree。
当 view 希望保留所有已加载图片直到 cache clear 或 drop 时，使用
`RetainAllImageCache`。

Remote 和 asset-backed `img(...)` elements 通过 app asset source 与 HTTP client
加载，然后经过 active image cache。

## Custom Providers

当 cache 应存储在 window-local element state 中时，实现 `ImageCacheProvider`。
provider 返回 `AnyImageCache`，并可基于当前 element id 创建或复用 `Entity<T>`
cache。

需要自定义 eviction 时实现 `ImageCache`。evicted images 在有 window 时应通过
`cx.drop_image(image, Some(window))` drop；release cleanup 中使用
`cx.drop_image(image, None)`。

## Async Loads

Image cache loaders 应把慢工作安排到 background executor，并在加载完成后 notify
owning view。当 load completion 从 background task 到达时，在下一帧 notify。

## Metrics

Image cache metrics 会记录 item count、byte count 和 evictions。cache ids 应足够稳
定以便 diagnostics，并在 cache release 时 drop metrics。

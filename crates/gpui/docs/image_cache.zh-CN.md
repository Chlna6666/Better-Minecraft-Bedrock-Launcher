# Image Cache

[English](image_cache.md)

GPUI image caches 控制 decoded images 和 GPU image resources 的 scope、retention
和 eviction。

## Built-In Scope

使用 `image_cache(provider).child(...)` element 把 image cache 行为限定到 subtree。
内置实现是 `BoundedImageCache`，必须通过 `BoundedImageCacheConfig` 明确限制 item
数量和 retained bytes；streaming entry 还计入后续循环所需的 compressed source。
GPUI 不提供无界 retain-all cache。

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

loading entry 同样计入 item limit。实现必须明确取消或保留已经不可达的工作；如果 key
已被 evict，但另一个 future 仍隐式持有相同 load，就不属于有界行为。

## Metrics

Image cache metrics 会记录 item count、byte count 和 evictions。cache ids 应足够稳
定以便 diagnostics，并在 cache release 时 drop metrics。

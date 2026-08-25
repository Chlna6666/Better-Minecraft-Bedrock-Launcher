# Image Cache

[Chinese](image_cache.zh-CN.md)

GPUI image caches control how decoded images and GPU image resources are scoped,
retained, and evicted.

## Built-In Scope

Use the `image_cache(provider).child(...)` element to scope image cache behavior
to a subtree. `BoundedImageCache` is the built-in cache and requires explicit
item and retained-byte limits through `BoundedImageCacheConfig`. Streaming
entries include the compressed source required for subsequent cycles. GPUI does not
provide an unbounded retain-all cache.

Remote and asset-backed `img(...)` elements load through the app asset source
and HTTP client, then pass through the active image cache.

## Custom Providers

Implement `ImageCacheProvider` when a cache should be stored in window-local
element state. A provider returns `AnyImageCache` and may create or reuse an
`Entity<T>` cache based on the current element id.

Implement `ImageCache` when custom eviction is required. Evicted images should
be dropped through `cx.drop_image(image, Some(window))` when a window is
available, or `cx.drop_image(image, None)` during release cleanup.

## Async Loads

Image cache loaders should schedule slow work on the background executor and
notify the owning view when loading completes. Notify on the next frame when the
load completion arrives from a background task.

Loading entries count against the item limit. Implementations must cancel or
detach no longer reachable work deliberately; evicting a key while another
future silently retains the same load is not bounded behavior.

## Metrics

Image cache metrics record item count, byte count, and evictions. Keep cache ids
stable enough for diagnostics, and drop metrics when the cache is released.

# Assets And Images

[Chinese](assets_and_images.zh-CN.md)

GPUI can load local assets, remote images through the configured HTTP client,
and application-owned image resources.

## Asset Sources

Implement `AssetSource` when an application or example needs packaged assets.
`load` returns bytes for a path and `list` returns child names. Register assets
with `Application::with_assets(...)`.

Keep asset paths relative to the asset source. Do not bake application-specific
absolute paths into framework code.

## HTTP Images

Remote `img(...)` sources use the app HTTP client. Set it with
`Application::with_http_client(...)` or `cx.set_http_client(...)`.

Examples should use the HTTP client exported by GPUI. If an example does not
intend to perform network IO, use `gpui::http_client::BlockedHttpClient` so the
dependency surface remains explicit.

## Image Caches

Use image cache elements and providers when image lifetime needs to be scoped:

- `image_cache(provider).child(...)` scopes a provider to an element subtree.
- `RetainAllImageCache` keeps images until it is cleared or dropped.
- Custom `ImageCacheProvider` implementations can build caches from window-local
  element state.

When a cache evicts an image, drop it through `cx.drop_image(image,
Some(window))` when a current window is available, or `cx.drop_image(image,
None)` during release cleanup.

## Guidelines

- Keep decoding and cache mutation out of render-only code.
- Use `background_spawn` or cache loader tasks for work that may block.
- Notify the owning entity on the next frame when an async image load completes.
- Bound cache size in examples that repeatedly load remote images.
- Surface missing assets and HTTP failures through visible example state or
  logs, not panics.

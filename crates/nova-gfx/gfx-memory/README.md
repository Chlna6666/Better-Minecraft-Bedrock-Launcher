# gfx-memory

[中文文档](README.zh-CN.md)

`gfx-memory` provides GPU memory allocation wrappers used by `nova-gfx`
backends.

It wraps backend allocator objects, exposes allocation accounting, and provides
a small upload-ring allocator used by resource upload paths. It does not own
graphics resources such as buffers, images, or textures; backend crates own those
resources and bind allocations to them.

The crate is split into focused modules:

- common allocation descriptors, errors, and statistics.
- upload-ring allocation for staging uploads.
- deferred-free queues keyed by completed GPU fence values.
- backend allocator wrappers selected by Cargo features.

## Backends

`gfx-memory` uses `default = []`. Enable only the backend allocator needed by
the caller:

```toml
gfx-memory = { version = "0.1", default-features = false, features = ["vulkan"] }
```

- `vulkan` enables `ash` and `gpu-allocator/vulkan`.
- `dx12` enables `windows` and `gpu-allocator/d3d12`.
- `metal` enables `objc2`, `objc2-metal`, and `gpu-allocator/metal`.

Non-target constructors return `GfxError::Unavailable` when the platform cannot
support the requested backend.

## Lifetime Rules

`DeferredFreeQueue<T>` stores payloads until the backend observes that the
associated GPU fence has completed. Backend destroy paths should remove the
public handle immediately, retire the native payload behind a fence, and release
native resources only after polling completion.

`UploadRingAllocator` also tracks busy pages by completed fence values. Pages
retired by a submit must not be reused until the backend reports the matching
GPU fence as complete. Synchronous helper upload paths may mark their temporary
fence complete only after waiting for the queue to finish.

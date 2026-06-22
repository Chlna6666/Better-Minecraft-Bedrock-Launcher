# nova-gfx Publishing

This document records the crates.io release shape for the `nova-gfx` crate
family.

## Release Order

Publish crates in dependency order:

1. `gfx-core`
2. `gfx-memory`
3. `gfx-shader`
4. `gfx-vulkan`
5. `gfx-dx12`
6. `gfx-metal`

`gfx-memory`, `gfx-shader`, and backend crates use `version + path`
dependencies for local development. `cargo package` for those crates requires
the referenced version to already exist in the registry, so package dry-runs for
downstream crates should be run after their dependencies have been published.

## Metadata Standard

Each library crate must keep:

- `license`
- `description`
- `rust-version`
- `readme`
- `documentation`
- `keywords`
- `categories`
- `include`
- `README.md`
- `README.zh-CN.md`

Examples are workspace-only smoke tests and must keep `publish = false`.

Do not add a `repository` or `homepage` field until the public repository URL is
the actual canonical project URL. Avoid placeholder URLs in published metadata.

## Validation

Before publishing:

```powershell
rtk cargo fmt --all
rtk cargo test -p gfx-core
rtk cargo test -p gfx-memory --no-default-features
rtk cargo test -p gfx-memory --no-default-features --features vulkan
rtk cargo test -p gfx-memory --no-default-features --features dx12
rtk cargo test -p gfx-shader
rtk cargo check -p gfx-vulkan
rtk cargo check -p gfx-dx12
rtk cargo check -p gfx-metal
rtk cargo check -p gpui --no-default-features
rtk cargo check -p gpui --no-default-features --features nova-gfx-vulkan
rtk cargo check -p gpui --no-default-features --features nova-gfx-dx12
rtk cargo tree -p gfx-vulkan --no-default-features
rtk cargo tree -p gpui --no-default-features --features nova-gfx-vulkan
rtk cargo doc -p gfx-core --no-deps
rtk cargo package -p gfx-core --allow-dirty
```

Run the `gfx-memory --features metal`, `gfx-metal`, and `nova-gfx-metal`
checks on macOS before a release that changes Metal-facing code. Windows can
verify the non-Apple stub path for `gfx-metal`, but it cannot validate the Metal
allocator feature.

The `gfx-vulkan` and GPUI Vulkan dependency trees must not pull `gfx-dx12`,
`gfx-metal`, D3D12, or Metal Objective-C dependencies.

After each dependency is published, run `cargo package --allow-dirty` for the
next crate in the release order.

## Breaking Feature Change

`gfx-memory` has `default = []`, and backend crates enable only the allocator
feature they need. GPUI now requires one of `nova-gfx-vulkan`,
`nova-gfx-dx12`, or `nova-gfx-metal` to select a renderer backend. The old
`nova-gfx` feature does not select a concrete backend and is retained only as an
empty compatibility marker for downstream migration.

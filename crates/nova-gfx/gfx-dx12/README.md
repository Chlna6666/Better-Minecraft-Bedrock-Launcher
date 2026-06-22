# gfx-dx12

[中文文档](README.zh-CN.md)

`gfx-dx12` is the Direct3D 12 implementation of the `gfx-core` device traits.

On Windows it creates native D3D12 devices, resources, pipelines, command
encoders, and DXGI swapchains. On non-Windows targets it still implements the
core traits with a minimal stub that returns `GfxError::Unavailable`, allowing
cross-platform crates to compile their non-target paths.

`raw-window-handle` is used only in this backend crate for native surface
creation. `gfx-core` does not depend on it.

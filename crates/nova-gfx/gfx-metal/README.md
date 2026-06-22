# gfx-metal

[中文文档](README.zh-CN.md)

`gfx-metal` is the Metal implementation of the `gfx-core` device traits.

On Apple targets it creates Metal devices, resources, pipelines, command
encoders, and `CAMetalLayer` swapchains. On non-Apple targets it implements the
core traits with a minimal stub that returns `GfxError::Unavailable`, allowing
non-target builds to compile.

Offscreen `draw_steps_to_texture` is currently reported as
`GfxError::Unavailable`; this crate keeps that behavior explicit until the Metal
offscreen path is implemented.

# Backdrop Blur

[English](backdrop_blur.md)

Backdrop blur 是用于模糊 painted primitive 背后内容的 renderer feature。它属于 GPUI
scene rendering，而不是 application-level visual policy。

## Scene Data

请求 backdrop blur 的 elements 会把 blur primitives 插入 scene。renderer 会把这些
primitives 与 frame 其他内容一起 batch，并为 performance metrics 记录 diagnostic
counts。

## GPU Pipeline

nova-gfx renderer 使用专用 WGSL 实现 backdrop blur。它把 base shader 与 blur-specific
shader modules 分离，让 feature pipelines 可以按需创建，并在 retained resource
trimming 中释放。

Backdrop blur rendering 可能分配 intermediate render targets。可能创建大量重叠
blurred regions 的 UI code 应限制 blur radii 和 primitive counts。

## Guidelines

- blur 行为保持 deterministic 且由 renderer 拥有。
- 不要把 application theme defaults 加入 renderer。
- 诊断 blur-heavy windows 时使用 metrics。
- 只有 intentionally retained diagnostic 或 platform compatibility code 才使用局部
  `#[expect(...)]`。

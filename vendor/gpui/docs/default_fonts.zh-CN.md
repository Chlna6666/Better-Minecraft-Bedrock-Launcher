# 默认字体

[English](default_fonts.md)

GPUI 拥有 font discovery、shaping、fallback 和 rasterization primitives。framework
应暴露 font capabilities，而不硬编码 product-specific font policy。

## Boundaries

应用可以在启动时配置 embedded fonts 或 font preferences。这些决策属于 application
setup，而不是 GPUI framework defaults。

GPUI platform text systems 应提供稳定 defaults、platform fallback 和 metrics，而不
依赖某个应用的 assets。

## Text System

text system 负责：

- font discovery 和 fallback；
- shaping 和 line layout；
- glyph rasterization；
- font features 和 fallback lists；
- platform-specific text integration。

## Guidelines

- framework default fonts 保持 generic。
- application font assets 不进入 GPUI internals。
- 影响 rendering 的 platform text-system differences 要文档化。
- optional fonts 缺失时避免 panic；应 fallback 或报告 diagnostics。

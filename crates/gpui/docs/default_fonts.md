# Default Fonts

[Chinese](default_fonts.zh-CN.md)

GPUI owns font discovery, shaping, fallback, and rasterization primitives. The
framework should expose font capabilities without hard-coding product-specific
font policy.

## Boundaries

Applications may configure embedded fonts or font preferences during startup.
Those decisions belong to application setup, not GPUI framework defaults.

GPUI platform text systems should provide stable defaults, platform fallback,
and metrics without depending on an application's assets.

## Text System

The text system handles:

- font discovery and fallback;
- shaping and line layout;
- glyph rasterization;
- font features and fallback lists;
- platform-specific text integration.

## Guidelines

- Keep framework default fonts generic.
- Keep application font assets outside GPUI internals.
- Document platform text-system differences where they affect rendering.
- Avoid panics for missing optional fonts; fall back or report diagnostics.

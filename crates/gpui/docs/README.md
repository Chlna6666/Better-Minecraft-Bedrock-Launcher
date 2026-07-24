# GPUI Documentation

[Chinese](README.zh-CN.md)

This documentation describes GPUI as a standalone UI framework. English files
are canonical. Chinese translations use matching `.zh-CN.md` files in the same
directory.

## Guides

- [Development guide](development.md): current API style, async work, renderer
  policy, and lint rules.
- [Contexts and entities](contexts.md): `App`, `Context<T>`, `Window`,
  `Entity<T>`, events, and subscriptions.
- [Rendering and elements](rendering.md): `Render`, `RenderOnce`, elements,
  styling, layout, and custom paint hooks.
- [Input and actions](input_and_actions.md): event handlers, listeners,
  actions, key bindings, and key dispatch.
- [Key dispatch](key_dispatch.md): focused key contexts and action dispatch.
- [Assets and images](assets_and_images.md): asset loading, HTTP images, image
  cache providers, and texture lifetime.
- [Image cache](image_cache.md): scoped image cache behavior and custom cache
  providers.
- [Animation engine](animation_engine.md): timing, easing, transition metadata,
  drivers, window scheduling, and scene/nova animation data paths.
- [Renderer backend](renderer_backend.md): backend options, frame scheduling,
  nova-gfx startup, and metrics.
- [Windows renderer backend](windows_renderer_backend.md): Windows nova-gfx
  backend selection and frame delivery.
- [Runtime WGSL shaders](runtime_wgsl_shaders.md): runtime shader validation
  and custom shader modules.
- [Backdrop blur](backdrop_blur.md): backdrop blur pipeline behavior.
- [Default fonts](default_fonts.md): font setup boundaries and platform
  defaults.
- [Performance pipeline](performance_pipeline.md): frame metrics and retained
  resource trimming.
- [Examples](examples.md): example authoring rules and current API patterns.
- [Validation](validation.md): formatting, checks, clippy, examples, and docs
  validation.

## Compatibility Notes

Use the current API names in all new code and examples:

- `App`
- `Context<T>`
- `Window`
- `Entity<T>`
- `WeakEntity<T>`
- `Render`
- `RenderOnce`

Do not write new application code with obsolete names such as `Model<T>`,
`View<T>`, `ModelContext<T>`, `WindowContext`, or `ViewContext<T>`.

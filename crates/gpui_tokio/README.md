# gpui_tokio

[中文](README.zh-CN.md)

`gpui_tokio` integrates Tokio with GPUI: it installs a Tokio runtime handle on the GPUI `App`
and provides `TokioContext` helpers so foreground/background tasks can use Tokio without
blocking the UI thread.

## Public surface

- `init(cx)` creates and installs a runtime;
- `init_from_handle(cx, handle)` installs a caller-owned runtime;
- `Tokio` / `TokioContext` provide the runtime accessor used by task helpers.

## Boundaries

- Never block the render path; long-running work belongs on the Tokio runtime or a background
  executor, with results marshalled back to GPUI.
- Runtime ownership stays with the application (`egpui` hosts it for BMCBL).

## Build and test

```powershell
cargo check -p gpui_tokio
cargo test -p gpui_tokio
```

## Documentation

- [Architecture](../../docs/ARCHITECTURE.en.md)
- [Build and test](../../docs/BUILD_AND_TEST.en.md)

# easytier-bmcbl

[中文](README.zh-CN.md)

`easytier-bmcbl` is the BMCBL wrapper around the upstream EasyTier crate. It re-exports the
core EasyTier modules and owns the application-side instance manager: runtime construction,
worker-thread sizing (bounded to 4–6 threads), lifecycle and configuration bridging.

## Features

`aes-gcm`, `socks5`, `websocket` forward to the upstream `easytier-core` features.

## Boundaries

- Upstream EasyTier sources live in `crates/easytier` and are excluded from the workspace;
  do not patch them here.
- The wrapper owns threading/lifecycle only; the BMCBL UI and room policy stay in `src/core`.

## Build and test

```powershell
cargo check -p easytier-bmcbl
cargo test -p easytier-bmcbl
```

## Documentation

- [Crate catalog](../../docs/CRATES.en.md)

# Build and test

This is the entry point for building, feature selection, testing and troubleshooting the BMCBL
workspace. Commands are PowerShell-first; Linux differences are noted.

## 1. Toolchain

- Stable Rust with edition 2024 (CI pins a toolchain in `.github/workflows/compile-check.yml`).
- Windows: MSVC toolchain and Windows 10/11 SDK; default target `x86_64-pc-windows-msvc`.
- Linux: `build-essential clang cmake libclang-dev llvm-dev ninja-build pkg-config`, plus
  `libasound2-dev libdbus-1-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev`,
  `libgl1-mesa-dev libpipewire-0.3-dev libssl-dev libudev-dev libvulkan-dev`,
  `libwayland-dev libx11-dev libx11-xcb-dev libxcb-*-dev libxcursor-dev libxi-dev`,
  `libxkbcommon-dev libxkbcommon-x11-dev libxrandr-dev nasm yasm`.
- `LIBCLANG_PATH`/`PATH` must resolve `libclang` (`libclang.dll` on Windows, `libclang.so` on
  Linux).
- Plugin development additionally needs `rustup target add wasm32-unknown-unknown`.

## 2. Build and run

```powershell
cargo run --bin BMCBL
cargo build --release --bin BMCBL
```

The dev profile uses `opt-level = 3` with line-tables-only debug info; release uses fat LTO,
`panic = "abort"` and stripping for size.

## 3. Features and backend matrix

Root package default features:

```toml
default = ["gpui-windows-dx12", "gpui-windows-vulkan", "gpui-opengl"]
```

Target-specific gpui dependencies provide the platform baseline: `nova-gfx-dx11` on Windows,
`nova-gfx-vulkan` (plus Wayland/X11) on Linux, and `nova-gfx-metal` on macOS (the application
does not support macOS).

| Platform | Included by default | Default backend | Optional |
| --- | --- | --- | --- |
| Windows | DX11 + DX12 + Vulkan + OpenGL | DX11 | switch to any compiled backend in settings |
| Linux | Vulkan + OpenGL 4.5 | Vulkan | `gpui-opengl` is enabled by default |
| macOS | Metal | — | not supported by the application |

`--no-default-features` keeps only the target-specific backend (Windows: DX11; Linux: Vulkan);
re-enable features as needed:

```powershell
cargo run --bin BMCBL --no-default-features
cargo run --bin BMCBL --no-default-features --features gpui-opengl
cargo run --bin BMCBL --features gpui-windows-dx12
cargo run --bin BMCBL --features gpui-windows-vulkan
```

Backend compile boundary:

```powershell
cargo check -p gfx-dx11
cargo check -p gfx-dx12
cargo check -p gfx-vulkan
cargo check -p gfx-opengl
cargo check -p gfx-metal
cargo check -p gfx-webgl
cargo check -p gpui --no-default-features --features nova-gfx-dx12
```

`gfx-webgl` is a stub and `gfx-metal` still lacks offscreen rendering; DX11/DX12/Vulkan/OpenGL
are the actively maintained backends.

## 4. Test tiers

### 4.1 Fast checks (no GPU)

```powershell
cargo check -p gfx-core -p gfx-dx11 -p gfx-dx12 -p gfx-vulkan -p gfx-opengl -p gfx-metal -p gfx-webgl
cargo check -p gpui --no-default-features --features "nova-gfx-dx11 nova-gfx-dx12 nova-gfx-vulkan nova-gfx-opengl"
cargo check -p gpui-3d
cargo check -p bmcbl
cargo test -p gfx-core
```

### 4.2 GPUI / nova unit tests

```powershell
$env:RUST_MIN_STACK = '67108864'
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib platform::nova::
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib animation::
```

### 4.3 Hardware gates (real GPU/window required)

These tests are `#[ignore]` by default and must be invoked explicitly:

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-dx12 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-vulkan -- --ignored --nocapture --test-threads=1
cargo test -p gfx-opengl -- --ignored --nocapture --test-threads=1
```

Consumer-side glyph/image gate:

```powershell
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib production_glyphs_and_images -- --ignored --nocapture --test-threads=1
```

### 4.4 Benchmarks (explicit feature)

```powershell
cargo bench --features bench-support
cargo bench --features bench-support --bench plugin_runtime
```

Benchmark output must not be committed under `docs/`.

## 5. Formatting, lints and documentation checks

```powershell
# Format only the files touched by this change; historical files may have formatting drift.
rustfmt --edition 2024 <changed-files...>
cargo clippy -p gfx-core -- -D warnings
cargo clippy -p gpui --no-default-features --features nova-gfx-dx12 -- -D warnings

pwsh -File scripts/check-docs.ps1
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
```

Workspace lints: `unsafe_code = "warn"`, Clippy `all`/`pedantic` at warn level; library code
should avoid `unwrap()`, and every `unsafe` block needs a `// SAFETY:` explanation.

## 6. CI

`.github/workflows/compile-check.yml` runs on Windows and Linux:

1. validate the GPUI workspace layout;
2. `cargo check -p bedrock-leveldb -p bedrock-world --all-features`;
3. `cargo build --lib`;
4. `cargo build --bin BMCBL`.

Release and nightly flows are defined in the Actions workflows and the root README's release
section.

## 7. Troubleshooting

- **`libclang` not found**: install LLVM, add `LLVM\bin` to `PATH`, set `LIBCLANG_PATH`.
- **Stale dependency/build artifacts**: run `cargo clean -p <crate>` first; only clear the whole
  `target` directory if necessary. Do not hand-edit `Cargo.lock`.
- **Windows test process stack failure (`0xc0000409`)**: raise the test thread stack
  (`RUST_MIN_STACK`) or split the test run by module; do not record it as a business failure.
- **EasyTier is missing `wintun.dll`**: the build only warns; supply the DLL from the vendored
  directory or an EasyTier release when networking is needed.
- **Hardware tests are skipped**: pass `--ignored` and ensure a window/GPU is available.

## 8. Documentation and artefacts

- Test output, raw benchmark data, screenshots and logs do not belong in `docs/`.
- Commands in documentation must run against the current workspace; update this page when
  features or backends change.

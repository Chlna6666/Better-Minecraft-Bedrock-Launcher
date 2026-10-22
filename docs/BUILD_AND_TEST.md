# 构建与测试

本文是 BMCBL 工作区的构建、feature、测试与排障入口。命令以 Windows PowerShell 为主，
Linux 差异单独标注。

## 1. 工具链

- Rust stable（edition 2024；CI 使用固定版本，见 `.github/workflows/compile-check.yml`）。
- Windows：MSVC 工具链、Windows 10/11 SDK；`cargo` 默认目标 `x86_64-pc-windows-msvc`。
- Linux：`build-essential clang cmake libclang-dev llvm-dev ninja-build pkg-config`，
  以及 `libasound2-dev libdbus-1-dev libegl1-mesa-dev libfontconfig1-dev libfreetype6-dev`、
  `libgl1-mesa-dev libpipewire-0.3-dev libssl-dev libudev-dev libvulkan-dev`、
  `libwayland-dev libx11-dev libx11-xcb-dev libxcb-*-dev libxcursor-dev libxi-dev`、
  `libxkbcommon-dev libxkbcommon-x11-dev libxrandr-dev nasm yasm`。
- `LIBCLANG_PATH`/`PATH` 需要能找到 `libclang.dll`（Windows）或 `libclang.so`（Linux）。
- 插件开发额外需要 `rustup target add wasm32-unknown-unknown`。

## 2. 构建与运行

```powershell
cargo run --bin BMCBL
cargo build --release --bin BMCBL
```

开发 profile 使用 `opt-level = 3`、`line-tables-only` 调试信息；release 使用 fat LTO、
`panic = "abort"`、strip，追求发布体积。

## 3. Feature 与后端矩阵

根 package 的默认 feature：

```toml
default = ["gpui-windows-dx12", "gpui-windows-vulkan", "gpui-opengl"]
```

target 专属 gpui 依赖负责各平台的基础后端：Windows 为 `nova-gfx-dx11`，Linux 为
`nova-gfx-vulkan`（同时启用 Wayland/X11），macOS 为 `nova-gfx-metal`（应用不支持 macOS）。

| 平台 | 默认包含 | 默认首选后端 | 可选 |
| --- | --- | --- | --- |
| Windows | DX11 + DX12 + Vulkan + OpenGL | DX11 | 设置中切换任意已编译后端 |
| Linux | Vulkan + OpenGL 4.5 | Vulkan | `gpui-opengl` 已默认启用 |
| macOS | Metal | — | 应用不提供支持 |

`--no-default-features` 只保留 target 专属后端（Windows：DX11；Linux：Vulkan），
可按需重新打开：

```powershell
cargo run --bin BMCBL --no-default-features
cargo run --bin BMCBL --no-default-features --features gpui-opengl
cargo run --bin BMCBL --features gpui-windows-dx12
cargo run --bin BMCBL --features gpui-windows-vulkan
```

后端编译边界：

```powershell
cargo check -p gfx-dx11
cargo check -p gfx-dx12
cargo check -p gfx-vulkan
cargo check -p gfx-opengl
cargo check -p gfx-metal
cargo check -p gfx-webgl
cargo check -p gpui --no-default-features --features nova-gfx-dx12
```

后端实现状态：`gfx-webgl` 为 stub；`gfx-metal` 的离屏渲染尚未实现；DX11/DX12/Vulkan/OpenGL
为当前主要维护后端。

## 4. 测试分级

### 4.1 快速检查（无 GPU 依赖）

```powershell
cargo check -p gfx-core -p gfx-dx11 -p gfx-dx12 -p gfx-vulkan -p gfx-opengl -p gfx-metal -p gfx-webgl
cargo check -p gpui --no-default-features --features "nova-gfx-dx11 nova-gfx-dx12 nova-gfx-vulkan nova-gfx-opengl"
cargo check -p gpui-3d
cargo check -p bmcbl
cargo test -p gfx-core
```

### 4.2 GPUI / nova 单元测试

```powershell
$env:RUST_MIN_STACK = '67108864'
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib platform::nova::
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib animation::
```

### 4.3 硬件 gate（需要真实 GPU/窗口）

这些测试默认 `#[ignore]`，必须显式运行：

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-dx12 -- --ignored --nocapture --test-threads=1
cargo test -p gfx-vulkan -- --ignored --nocapture --test-threads=1
cargo test -p gfx-opengl -- --ignored --nocapture --test-threads=1
```

GPUI 生产字形/图片路径由消费方 gate 验证（使用各 backend 的 feature）：

```powershell
cargo test -p gpui --no-default-features --features nova-gfx-dx12 --lib production_glyphs_and_images -- --ignored --nocapture --test-threads=1
```

### 4.4 基准（显式 feature）

```powershell
cargo bench --features bench-support
cargo bench --features bench-support --bench plugin_runtime
```

基准入口只在 `bench-support` 下存在，不能把基准输出写入 `docs/`。

## 5. 格式、Lint 与文档检查

```powershell
# 只格式化本次改动的文件；仓库历史文件可能存在格式漂移
rustfmt --edition 2024 <改动文件...>
cargo clippy -p gfx-core -- -D warnings
cargo clippy -p gpui --no-default-features --features nova-gfx-dx12 -- -D warnings

pwsh -File scripts/check-docs.ps1
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
```

仓库范围 lint 规则：`unsafe_code = "warn"`，Clippy `all`/`pedantic` 为 warn；库代码避免
`unwrap()`，所有 `unsafe` 块必须有 `// SAFETY:` 说明。

## 6. CI

`.github/workflows/compile-check.yml` 在 Windows 与 Linux 上：

1. 校验 GPUI workspace layout；
2. `cargo check -p bedrock-leveldb -p bedrock-world --all-features`；
3. `cargo build --lib`；
4. `cargo build --bin BMCBL`。

发布与 nightly 流程见仓库 Actions workflow 与根 README 的“版本与发布”章节。

## 7. 常见问题

- **`libclang` 找不到**：安装 LLVM 并把 `LLVM\bin` 加入 `PATH`，设置 `LIBCLANG_PATH`。
- **依赖/编译缓存异常**：先 `cargo clean -p <crate>`；必要时清理整个 `target`，不要手工修改
  `Cargo.lock`。
- **测试进程栈异常（Windows `0xc0000409`）**：提高测试线程栈（`RUST_MIN_STACK`），或按测试
  模块拆分运行；不要把它记录为业务失败。
- **EasyTier 联机缺失 `wintun.dll`**：构建只会告警；需要联机时从 vendored 目录或 EasyTier
  发布物补齐。
- **硬件测试被跳过**：确认已加 `--ignored`，且窗口/GPU 可用。

## 8. 文档与产物

- 测试输出、benchmark 原始数据、截图和日志不进入 `docs/`。
- 文档中的命令必须能在当前工作区直接执行；修改 feature/后端后同步更新本文。

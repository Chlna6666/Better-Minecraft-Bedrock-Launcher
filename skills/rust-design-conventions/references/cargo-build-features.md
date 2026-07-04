# Cargo、构建系统与 Features

> Cargo 是 Rust 的官方构建系统和包管理器。掌握 Cargo 的高级特性（features、条件编译、workspace、交叉编译、build script）是发布生产级 Rust 项目的前提。

## 1. Cargo.toml 完全规范

### 完整结构

```toml
[package]
name = "my-project"
version = "1.2.3"                    # 语义化版本
edition = "2021"                     # Rust edition（2015/2018/2021/2024）
rust-version = "1.75"                # 最低 Rust 版本（MSRV）
authors = ["Alice <alice@example.com>"]
license = "MIT OR Apache-2.0"        # SPDX 表达式
description = "Brief one-line description"
repository = "https://github.com/org/repo"
homepage = "https://project.example.com"
documentation = "https://docs.rs/project"
readme = "README.md"
keywords = ["api", "client"]         # crates.io 搜索（≤5 个）
categories = ["web-programming"]     # crates.io 分类
exclude = [                           # 打包时排除
    "tests/fixtures/*",
    ".github/",
]
include = [                           # 打包时包含（与 exclude 互斥）
    "src/**",
    "Cargo.toml",
    "README.md",
]

# workspace 成员（仅 workspace root 写）
[workspace]
members = ["crates/*"]
exclude = ["crates/legacy"]
resolver = "2"                       # 推荐 edition 2021+ 用 resolver 2

[dependencies]
serde = { version = "1.0", features = ["derive"] }
tokio = { version = "1", optional = true }  # 可选依赖

[dev-dependencies]                   # 仅测试/bench/example 用
proptest = "1"
mockall = "0.11"

[build-dependencies]                 # build script 用
cc = "1"

[features]
default = ["std"]
std = []
async = ["dep:tokio"]                # dep: 显式引用可选依赖

[[bin]]
name = "my-cli"
path = "src/bin/cli.rs"

[profile.release]
opt-level = 3
lto = "fat"
```

### 版本指定

```toml
[dependencies]
# 精确版本范围（推荐）
serde = "1.0"          # 等价于 ^1.0.0：>=1.0.0, <2.0.0
serde = "^1.0"         # 同上
serde = "1"            # 等价于 ^1：>=1.0.0, <2.0.0
serde = "1.2.3"        # 等价于 ^1.2.3：>=1.2.3, <2.0.0

# 更严格
serde = "=1.0.45"      # 恰好 1.0.45
serde = ">=1.0, <1.2"  # 范围
serde = "~1.2"         # 等价于 >=1.2.0, <1.3.0

# Git
lib = { git = "https://github.com/org/repo" }
lib = { git = "https://github.com/org/repo", branch = "dev" }
lib = { git = "https://github.com/org/repo", tag = "v1.0" }
lib = { git = "https://github.com/org/repo", rev = "abc123" }

# 本地路径（开发时）
lib = { path = "../lib" }
```

## 2. Features 设计

### 基本语法

```toml
[features]
# 1. 简单 feature（开/关编译分支）
default = ["std"]
std = []

# 2. feature 启用其他 features
async-runtime = ["tokio", "async-trait"]

# 3. feature 启用可选依赖（Cargo 1.60+）
#    用 dep: 前缀避免自动 feature
[dependencies]
tokio = { version = "1", optional = true }

[features]
async = ["dep:tokio"]  # 启用 tokio 依赖
# 启用方式：cargo build --features async

# 4. 对依赖启用 feature
[dependencies]
serde = { version = "1", optional = true }

[features]
serde-derive = ["serde/derive"]  # 启用 serde 的 derive feature
```

### Feature 设计原则

```toml
[features]
# ✓ 1. 默认最小化
default = []  # 或只包含最基础功能
# 用户按需启用，避免引入不需要的依赖

# ✓ 2. 命名清晰
# 用功能名而非依赖名
json = ["serde", "serde_json"]  # ✓ 提供功能
# tokio = ["dep:tokio"]         # ❌ 直接用依赖名易混淆

# ✓ 3. 文档化每个 feature
[features]
## 启用 std 库支持（默认）
std = []
## 启用异步运行时（tokio）
async = ["dep:tokio"]
## 启用 TLS 支持
tls = ["rustls", "webpki-roots"]
```

### Feature 组合陷阱

```toml
# ❌ feature 间的隐式依赖关系可能产生意外组合
[features]
backend-a = ["dep:lib-a"]
backend-b = ["dep:lib-b"]

# 如果用户同时启用 backend-a 和 backend-b，行为可能未定义
# 解决：用 mutually exclusive 的 cfg 检查（见下）
```

### Cargo 1.60+ 的 dep: 语法

```toml
# Cargo.toml
[dependencies]
tokio = { version = "1", optional = true }
serde = { version = "1", optional = true }

[features]
# 旧语法：每个可选依赖自动产生同名 feature
# tokio = ["dep:tokio"]  # ❌ 冗余

# 新语法（推荐）：用 dep: 显式
[features]
async = ["dep:tokio"]  # 启用 tokio 依赖
# 没有 dep: 前缀的 tokio 不会自动创建
```

## 3. 条件编译（#[cfg]）

### 基于 feature

```rust
// 根据编译时 feature 切换代码
#[cfg(feature = "async")]
pub async fn fetch(url: &str) -> String { /* tokio 实现 */ }

#[cfg(not(feature = "async"))]
pub fn fetch(url: &str) -> String { /* 同步实现 */ }
```

### 基于目标

```rust
// 操作系统
#[cfg(target_os = "linux")]
fn syscall() { /* Linux 实现 */ }

#[cfg(target_os = "windows")]
fn syscall() { /* Windows 实现 */ }

#[cfg(target_os = "macos")]
fn syscall() { /* macOS 实现 */ }

// 架构
#[cfg(target_arch = "x86_64")]
fn simd_add() { /* AVX2 */ }

#[cfg(target_arch = "aarch64")]
fn simd_add() { /* NEON */ }

// 指针宽度
#[cfg(target_pointer_width = "64")]
type Size = u64;
#[cfg(target_pointer_width = "32")]
type Size = u32;

// Endianness
#[cfg(target_endian = "little")]
fn read_u32(bytes: [u8; 4]) -> u32 { u32::from_le_bytes(bytes) }
```

### cfg_if 宏（清晰的条件）

```rust
// Cargo.toml: cfg-if = "1"
use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(target_os = "linux")] {
        fn os_name() -> &'static str { "Linux" }
    } else if #[cfg(target_os = "windows")] {
        fn os_name() -> &'static str { "Windows" }
    } else if #[cfg(target_os = "macos")] {
        fn os_name() -> &'static str { "macOS" }
    } else {
        fn os_name() -> &'static str { "Unknown" }
    }
}
```

### 常用 cfg 属性

```rust
// Cargo.toml [features] 启用的
#[cfg(feature = "my_feature")]

// Cargo profile
#[cfg(debug_assertions)]          // debug 模式
#[cfg(not(debug_assertions))]     // release 模式
#[cfg(test)]                       // 测试编译

// 目标三元组
#[cfg(target = "x86_64-unknown-linux-gnu")]

// 自定义（用 --cfg 传入）
// rustc --cfg my_custom
#[cfg(my_custom)]
```

### 在 Cargo.toml 中条件依赖

```toml
[target.'cfg(target_os = "linux")'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
winapi = { version = "0.3", features = ["winuser"] }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = "0.2"
```

## 4. Workspace 管理

### 基本结构

```toml
# workspace 根 Cargo.toml
[workspace]
members = [
    "crates/core",
    "crates/cli",
    "crates/server",
]
resolver = "2"

[workspace.dependencies]  # 共享依赖版本（Cargo 1.64+）
serde = "1.0"
tokio = "1"

[workspace.package]  # 共享元信息（Cargo 1.64+）
version = "0.1.0"
edition = "2021"
license = "MIT"
```

```toml
# crates/core/Cargo.toml
[package]
name = "my-core"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
serde.workspace = true  # 引用 workspace 依赖
```

### Workspace 优势

```rust
// 1. 统一版本：所有 crate 用相同依赖版本
// 2. 共享 Cargo.lock：一次锁定，避免版本冲突
// 3. 共享 target/：编译产物复用
// 4. 统一发布：cargo publish 各 crate
// 5. 统一测试：cargo test --all
```

### Workspace 命令

```bash
cargo build                 # 构建所有
cargo test                  # 测试所有
cargo build -p my-core      # 只构建某个 crate
cargo run -p my-cli         # 运行某个 binary
cargo update -p serde       # 更新特定依赖
```

## 5. Profile 配置

### 各 profile 配置

```toml
[profile.dev]
opt-level = 0          # 默认 0（最快编译）
debug = true           # 默认 true
incremental = true     # 默认 true

[profile.release]
opt-level = 3          # 最高优化
debug = false          # 无调试信息
strip = true           # 去符号
lto = "fat"            # 全程序链接时优化
codegen-units = 1      # 单 codegen unit（最佳优化）
panic = "abort"        # abort 而非 unwind

[profile.release.package."*"]  # 依赖单独配置
opt-level = "s"        # 依赖优化体积

# 自定义 profile
[profile.bench]
inherits = "release"
debug = true           # benchmark 需要部分调试信息

# 基于 release 但带调试
[profile.release-debug]
inherits = "release"
debug = true
strip = false

# 使用：cargo build --profile release-debug
```

### Profile 选项说明

| 选项 | 值 | 说明 |
|------|-----|------|
| `opt-level` | 0/1/2/3/s/z | 优化级别（s/z 优化体积） |
| `debug` | true/false/0/1/2 | 调试信息（2 最详细） |
| `lto` | false/true/"thin"/"fat" | 链接时优化 |
| `codegen-units` | 1-256 | 编译单元数（1 最优，256 最快编译） |
| `panic` | "unwind"/"abort" | panic 策略 |
| `strip` | true/false/"symbols"/"debuginfo" | 去除符号 |
| `incremental` | true/false | 增量编译 |

## 6. Build Script（build.rs）

### 何时用 build.rs

```rust
// build.rs 在 Cargo 编译前运行，用于：
// 1. 编译 C/C++ 依赖
// 2. 生成本地代码绑定（bindgen）
// 3. 读取环境变量生成配置
// 4. 检查目标平台特性
```

### 基本示例

```rust
// build.rs
fn main() {
    // 1. 编译 C 库
    cc::Build::new()
        .file("src/native.c")
        .compile("native");

    // 2. 告诉 Cargo 重新运行条件
    println!("cargo:rerun-if-changed=src/native.c");
    println!("cargo:rerun-if-env-changed=CC");

    // 3. 生成 cfg flag
    let target = std::env::var("TARGET").unwrap();
    if target.contains("linux") {
        println!("cargo:rustc-cfg=on_linux");
    }
}
```

### 生成配置供代码使用

```rust
// build.rs
fn main() {
    // 读取版本并设置
    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=APP_VERSION={version}");
}

// src/main.rs
fn main() {
    println!("Version: {}", env!("APP_VERSION"));  // 自动注入
}
```

### 链接系统库

```rust
// build.rs
fn main() {
    println!("cargo:rustc-link-lib=ssl");      // 链接 libssl
    println!("cargo:rustc-link-lib=dylib=crypto");
}
```

## 7. 交叉编译

### 基本用法

```bash
# 安装目标工具链
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-apple-darwin
rustup target add wasm32-unknown-unknown

# 交叉编译
cargo build --target x86_64-unknown-linux-musl
cargo build --target aarch64-apple-darwin

# 配置默认目标
# .cargo/config.toml
[build]
target = "x86_64-unknown-linux-musl"
```

### 常见目标

```
x86_64-unknown-linux-gnu       # Linux 64 位（动态链接）
x86_64-unknown-linux-musl      # Linux 64 位（静态链接）
aarch64-unknown-linux-gnu      # ARM64 Linux（树莓派/服务器）
x86_64-pc-windows-msvc         # Windows MSVC
x86_64-apple-darwin            # Intel macOS
aarch64-apple-darwin           # Apple Silicon macOS
wasm32-unknown-unknown         # WebAssembly
wasm32-wasi                    # WebAssembly + WASI
armv7-unknown-linux-gnueabihf  # ARMv7（嵌入式）
```

### cross 工具（处理系统库）

```bash
# Cargo.toml 交叉编译需要 C 依赖时很复杂
# cross 用 Docker 自动处理交叉编译环境
cargo install cross
cross build --target aarch64-unknown-linux-gnu
cross test --target armv7-unknown-linux-gnueabihf
```

## 8. 发布到 crates.io

### 准备

```toml
[package]
name = "my-crate"           # 名字唯一
version = "0.1.0"           # 语义化版本
license = "MIT"             # 必须（或 license-file）
description = "..."
repository = "..."          # 推荐
```

### 发布

```bash
# 登录（需要 crates.io API token）
cargo login <token>

# 试运行（检查打包内容）
cargo package

# 实际打包
cargo package --list  # 查看将发布的文件

# 发布
cargo publish

# yank（不让新项目用此版本，已用的不受影响）
cargo yank --vers 1.0.0
cargo yank --vers 1.0.0 --undo  # 撤销 yank
```

### 版本发布策略

```bash
# 补丁：bug 修复（兼容）
cargo release patch  # 0.1.0 → 0.1.1

# 次要：新功能（兼容）
cargo release minor  # 0.1.0 → 0.2.0

# 主要：breaking change
cargo release major  # 0.1.0 → 1.0.0

# 用 cargo-release 自动化
cargo install cargo-release
cargo release 0.2.0
```

## 9. 常用 cargo 命令速查

```bash
# 构建
cargo build [--release] [--target X] [-p crate]

# 运行
cargo run [-- args...]

# 测试
cargo test                          # 所有测试
cargo test --test integration       # 集成测试
cargo test -- --nocapture           # 显示 println!
cargo test -- --ignored             # 运行 #[ignore] 测试
cargo bench                         # 基准测试
cargo test --doc                    # 文档测试

# 文档
cargo doc [--open]                  # 生成文档
cargo doc -p serde --open           # 单个 crate 文档

# 依赖管理
cargo update                        # 更新 Cargo.lock
cargo update -p serde               # 更新特定依赖
cargo tree                          # 依赖树
cargo tree -d                       # 重复依赖（多个版本）
cargo add tokio --features full     # 添加依赖
cargo rm tokio                      # 移除依赖
cargo upgrade                       # 升级版本范围（cargo install cargo-upgrade）

# 检查
cargo check                         # 只检查不生成代码（快）
cargo clippy                        # lint
cargo fmt                           # 格式化
cargo fix                           # 自动修复

# 扩展工具
cargo install cargo-expand          # 查看宏展开
cargo install cargo-edit            # add/rm 命令
cargo install cargo-outdated        # 检查过时依赖
cargo install cargo-audit           # 安全漏洞扫描
cargo install cargo-bloat           # 分析二进制体积
cargo install cargo-deny            # 许可证/ Advisory 检查
```

## 10. Cargo/构建检查清单

### Cargo.toml
- [ ] 版本指定合理（`^` 而非精确）
- [ ] MSRV 通过 `rust-version` 声明
- [ ] license 字段存在
- [ ] 关键字/分类合理（便于 crates.io 搜索）
- [ ] exclude/include 配置正确（包大小）
- [ ] 可选依赖用 `dep:` 语法（Cargo 1.60+）

### Features
- [ ] default 最小化
- [ ] 每个 feature 有文档
- [ ] feature 间无意外组合
- [ ] 命名描述功能而非依赖

### 条件编译
- [ ] 平台特定代码用 `#[cfg]`
- [ ] 多分支用 `cfg_if!` 替代嵌套 `#[cfg]`
- [ ] `target.'cfg(...)'.dependencies` 处理平台依赖

### Workspace
- [ ] 共享依赖用 `[workspace.dependencies]`
- [ ] 共享元信息用 `[workspace.package]`
- [ ] resolver = "2"

### Profile
- [ ] release 配置 LTO + codegen-units=1
- [ ] 按需 strip
- [ ] 依赖用 `[profile.release.package."*"]`

### 构建
- [ ] build.rs 有 rerun-if-changed
- [ ] 交叉编译用 cross 工具
- [ ] cargo clippy + fmt 在 CI 运行

### 发布
- [ ] cargo package 检查打包内容
- [ ] cargo audit 检查安全漏洞
- [ ] cargo deny 检查许可证
- [ ] CHANGELOG 维护

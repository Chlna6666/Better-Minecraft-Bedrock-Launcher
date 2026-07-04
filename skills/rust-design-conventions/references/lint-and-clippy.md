# Lint 与 Clippy 配置

> Lint 是 Rust 严谨工程的质量门禁。`rustc` 内置 lint + `clippy` 提供的 700+ 条 lint，加上 `[lints]` 配置，能在编译期拦截绝大多数常见错误和反模式。一个严谨项目的标准是：**`cargo clippy -D warnings` 在 CI 中必须通过**，且 lint 策略按团队风险偏好分级配置。

## 1. Lint 体系全景

```
Rust Lint 体系
├── rustc 内置 lint         # 编译器自带，如 unused_variables
│   ├── allow/warn/deny/forbid
│   └── 通过 [lints.rust] 或 #![warn(...)] 配置
├── clippy lint             # 额外 700+ 条，分类为 6 组
│   ├── correctness         # 正确性（应为 deny）
│   ├── suspicious          # 可疑代码（应为 deny/warn）
│   ├── complexity          # 过度复杂（应为 warn）
│   ├── perf                # 性能（应为 warn）
│   ├── pedantic           # 学究式风格（可选 warn）
│   └── nursery            # 实验性（通常 allow）
└── rustdoc lint            # 文档相关
```

### Lint 级别

| 级别 | 行为 | 用途 |
|------|------|------|
| `allow` | 不报 | 显式关闭某 lint |
| `warn` | 警告，不阻断编译 | 提示但不强制 |
| `deny` | 错误，阻断编译 | 强制遵守 |
| `forbid` | 错误 + 不可被下游 `allow` 覆盖 | 最重要的不变量 |

**`deny` vs `forbid`：**
- `deny`：当前作用域可用 `#[allow(...)]` 临时放宽
- `forbid`：禁止任何下游 `#[allow]` 覆盖，最强约束

```rust
// forbid 比 deny 更强
#![forbid(unsafe_code)]  // 整个 crate 禁止 unsafe，任何模块都无法 allow
#![deny(missing_docs)]
```

## 2. 配置方式

### 2.1 Cargo.toml 的 `[lints]`（推荐，Cargo 1.74+）

集中配置，跨文件统一，CI 友好：

```toml
# Cargo.toml
[lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
rust_2018_idioms = "warn"
rust_2021_compatibility = "warn"
unused_must_use = "deny"
unreachable_pub = "warn"

[lints.clippy]
# 正确性（必须 deny）
all = { level = "deny", priority = -1 }

# 性能
perf = { level = "warn", priority = -1 }

# 风格（pedantic，按需）
pedantic = { level = "warn", priority = -1 }
```

**`priority = -1` 的含义：** 当 `all`/`pedantic` 这类 lint group 和单条 lint 同时配置时，用负优先级让 group 先应用，再被具体 lint 覆盖。

### 2.2 crate 根的 `#![...]` 属性

旧式配置，Cargo 1.74 前唯一方式，仍可用：

```rust
// src/lib.rs 或 src/main.rs 顶部
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(rust_2018_idioms)]
#![warn(clippy::all, clippy::pedantic, clippy::nursery)]
```

### 2.3 单项覆盖

```rust
// 临时放宽某 lint
#[allow(dead_code)]
fn unused_but_intentional() { ... }

// 局部提升级别
#[warn(clippy::too_many_arguments)]
fn complex_handler(/* 10 个参数 */) { ... }
```

## 3. 推荐 lint 配置（严谨项目基线）

### 3.1 Rust 内置 lint

```toml
[lints.rust]
# 安全性（forbid，不可覆盖）
unsafe_code = "forbid"              # 禁止 unsafe（除非确需）

# 文档
missing_docs = "warn"               # pub 项必须有文档

# 现代惯用法
rust_2018_idioms = "warn"           # 2018 edition 惯用法
rust_2021_compatibility = "warn"    # 2021 edition 兼容
future_incompatible = "warn"        # 未来版本会破坏的代码

# 严格性
unused = "warn"                     # 未使用的 import/变量
unused_must_use = "deny"            # 必须处理 Result
unreachable_pub = "warn"            # pub 但实际没被外部用
private_in_public = "deny"          # 公共 API 暴露私有类型
elided_lifetimes_in_paths = "warn"  # 公共路径省略生命周期
missing_debug_implementations = "warn"  # pub 类型应有 Debug
```

### 3.2 Clippy lint（按组分级）

```toml
[lints.clippy]
# === 正确性组：必须 deny ===
correctness = { level = "deny", priority = -1 }
# 捕获逻辑错误：== 比浮点、off-by-one、未处理的 Result

# === 可疑组：deny ===
suspicious = { level = "deny", priority = -1 }
# 可能有 bug 的模式：如 &vec[..-1]、不必要 clone

# === 性能组：warn（热点路径手动 deny）===
perf = { level = "warn", priority = -1 }
# redundant_clone, needless_collect, large_enum_variant

# === 复杂度组：warn ===
complexity = { level = "warn", priority = -1 }
# 过度复杂：嵌套 match、可简化的表达式

# === 风格组：warn ===
style = { level = "warn", priority = -1 }
# 不符合惯用法的写法

# === pedantic 组：可选 warn ===
# pedantic = { level = "warn", priority = -1 }
# 学究式：very strict，可能产生大量警告

# === 单条高价值 lint（即使不用 pedantic 也建议开）===
enum_glob_use = "deny"              # 禁止 use Enum::*（用 Enum::Variant）
expect_used = "warn"                # 警告 expect（生产代码用 ?）
unwrap_used = "warn"                # 警告 unwrap
panic = "warn"                      # 警告直接 panic
todo = "warn"                       # 警告 todo!()（防止忘记实现）
unimplemented = "deny"              # 禁止 unimplemented!()
dbg_macro = "warn"                  # 警告 dbg!（防止提交到生产）
print_stdout = "warn"               # 警告 println!（应用 log）
print_stderr = "warn"               # 警告 eprintln!
must_use_candidate = "warn"         # 提示应加 #[must_use]
return_self_not_must_use = "warn"   # 返回 Self 应标 must_use
```

### 3.3 文档 lint

```toml
[lints.clippy]
missing_errors_doc = "warn"         # Result 函数需 # Errors
missing_panics_doc = "warn"         # 可能 panic 需 # Panics
missing_safety_doc = "deny"         # unsafe 必须 # Safety
doc_markdown = "warn"               # 代码标识用反引号
```

## 4. pedantic / nursery 的取舍

### 4.1 pedantic 组特点

```toml
# pedantic 包含约 300 条 lint，非常严格
pedantic = { level = "warn", priority = -1 }
```

**优点：** 全面，强制高质量。
**缺点：** 警告量大，部分 lint 偏主观（如 `module_name_repetitions` 要求避免 `user::user_service`）。

**策略：**
- 新项目从 pedantic 开始，培养习惯
- 存量项目按需选单条 lint，避免一次性引入大量警告

### 4.2 高价值单条 lint（即使不用 pedantic）

```toml
[lints.clippy]
# 防止常见错误
cast_possible_truncation = "warn"   # as 转换可能截断
cast_sign_loss = "warn"             # 有符号转无符号丢符号
cast_precision_loss = "warn"        # f64 转 f32 丢精度
indexing_slicing = "warn"           # 直接索引（建议 get）

# API 质量
must_use_candidate = "warn"         # 提示加 #[must_use]
missing_const_for_fn = "warn"       # 可标 const 的函数
single_char_pattern = "warn"        # "x".contains('x') 而非 "x"

# 性能
redundant_clone = "warn"            # 不必要 clone
needless_collect = "warn"           # 不必要 collect
large_enum_variant = "warn"         # 枚举大变体该 Box

# 可读性
cognitive_complexity = "warn"       # 函数认知复杂度
too_many_arguments = "warn"         # 参数过多
too_many_lines = "warn"             # 函数过长
```

## 5. CI 门禁

### 5.1 GitHub Actions 标准配置

```yaml
# .github/workflows/ci.yml
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all -- --check        # 格式检查
      - run: cargo clippy --all-targets --all-features -- -D warnings
        # -D warnings 把所有警告变错误，CI 失败
```

**关键标志：**
- `--all-targets`：含 tests/benches/examples
- `--all-features`：所有 feature 组合
- `-D warnings`：任何警告都失败

### 5.2 预提交钩子

```bash
# .git/hooks/pre-commit
#!/bin/sh
cargo fmt --check || { echo "请先 cargo fmt"; exit 1; }
cargo clippy -- -D warnings || { echo "clippy 检查失败"; exit 1; }
```

或用 `cargo-husky` 自动安装：

```toml
# Cargo.toml
[dev-dependencies]
cargo-husky = { version = "1", features = ["precommit-hook", "run-cargo-clippy"] }
```

### 5.3 缓存优化

```yaml
# clippy 慢，加缓存
- uses: Swatinem/rust-cache@v2
- run: cargo clippy --all-targets -- -D warnings
```

## 6. 处理 lint 警告

### 6.1 修复而非压制（首选）

```rust
// ❌ 压制警告
#[allow(clippy::too_many_arguments)]
fn handler(a, b, c, d, e, f, g, h) { ... }

// ✓ 修复：引入参数对象
struct HandlerParams { a, b, c, d, e, f, g, h }
fn handler(params: HandlerParams) { ... }
```

### 6.2 合理压制（必要时）

```rust
// 当确实需要绕过时，说明原因
#[allow(clippy::needless_range_loop)]  // 此处索引更清晰，体现对齐
for i in 0..rows {
    matrix[i][i] = 1;
}

// 模块级压制（慎用）
#[allow(clippy::module_inception)]  // mod parser::parser 是故意的
mod parser;
```

**规则：** 每次 `#[allow]` 都必须有注释说明原因。

### 6.3 修复 clippy 自动建议

```bash
# 自动修复可修复的 lint
cargo clippy --fix --allow-dirty --allow-no-vcs

# 或用 cargo fix
cargo fix --clippy
```

## 7. 自定义 lint 场景

### 7.1 禁止特定 crate

```rust
// 防止团队误用某些 crate
// 用 dylint 或在 CI 检查 Cargo.lock
```

### 7.2 deny-specific 模式

```toml
[lints.clippy]
# 禁止 unwrap/expect 在生产代码（测试代码单独 allow）
unwrap_used = "deny"
expect_used = "deny"
```

测试代码中临时放宽：

```rust
// tests/integration.rs
#![allow(clippy::unwrap_used, clippy::expect_used)]  // 测试中 unwrap 可接受

#[test]
fn test() {
    let result = parse().unwrap();  // 测试中 OK
}
```

## 8. 完整推荐配置模板

```toml
# Cargo.toml — 严谨库项目基线
[lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"
rust_2018_idioms = "warn"
rust_2021_compatibility = "warn"
future_incompatible = "warn"
unused = "warn"
unused_must_use = "deny"
private_in_public = "deny"
missing_debug_implementations = "warn"

[lints.clippy]
# 分组
correctness = { level = "deny", priority = -1 }
suspicious = { level = "deny", priority = -1 }
complexity = { level = "warn", priority = -1 }
perf = { level = "warn", priority = -1 }
style = { level = "warn", priority = -1 }

# 单条强制
enum_glob_use = "deny"
unimplemented = "deny"
missing_safety_doc = "deny"
missing_errors_doc = "warn"
missing_panics_doc = "warn"
unwrap_used = "warn"
expect_used = "warn"
panic = "warn"
todo = "warn"
dbg_macro = "warn"
print_stdout = "warn"
print_stderr = "warn"
must_use_candidate = "warn"
redundant_clone = "warn"
needless_collect = "warn"
large_enum_variant = "warn"
cast_possible_truncation = "warn"
cast_sign_loss = "warn"
```

## 9. Lint 检查清单

### 配置
- [ ] 用 `[lints]`（Cargo 1.74+）集中配置，而非散落的 `#![...]`
- [ ] `unsafe_code = "forbid"`（除非确需 unsafe）
- [ ] `correctness`/`suspicious` 组 deny
- [ ] `perf`/`complexity`/`style` 组 warn
- [ ] 文档 lint（`missing_errors_doc`/`missing_panics_doc`/`missing_safety_doc`）开启

### 生产代码强制
- [ ] `unwrap_used`/`expect_used` 至少 warn（生产代码用 `?`）
- [ ] `unused_must_use = "deny"`（必须处理 Result）
- [ ] `private_in_public = "deny"`（公共 API 不暴露私有）
- [ ] `missing_docs`（pub 项必须有文档）

### CI
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` 通过
- [ ] `cargo fmt --all -- --check` 通过
- [ ] 缓存优化（clippy 慢）
- [ ] 预提交钩子（cargo-husky 或自定义）

### 处理
- [ ] 修复优先于 `#[allow]` 压制
- [ ] 每次 `#[allow]` 有注释说明原因
- [ ] 测试代码可批量 allow（`unwrap`/`expect`）
- [ ] 定期 `cargo clippy --fix` 清理可自动修复项

### 进阶
- [ ] `cast_*` 系列 lint 防止数值转换错误
- [ ] `large_enum_variant` 防止枚举内存浪费
- [ ] `must_use_candidate` 提示加 `#[must_use]`
- [ ] 评估是否启用 `pedantic`（新项目推荐，存量按需）

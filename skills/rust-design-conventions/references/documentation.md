# 文档注释规范（rustdoc）

> Rust 的文档注释不只是给人看的说明，它是**可测试的、可编译的、生态契约**。一个严谨的 Rust 库，每一项公共 API 都必须有 rustdoc 注释，且必须声明 `# Panics` / `# Errors` / `# Safety` 等约定章节。缺失这些等于把未定义行为和 panic 风险推给调用者。

## 1. 两种注释的区别

```rust
// 普通注释：编译时被丢弃，不出现在文档
// 这是行注释
/* 这是块注释 */

// 文档注释：被 rustdoc 收集，生成 API 文档，可包含可运行代码
/// 文档注释（紧贴下面的项）
/** 块文档注释 */

//! 模块/crate 级文档注释（放在文件顶部，描述整个模块）
/*! 块模块文档 */
```

**规则：**
- 每个 `pub` 项**必须**有 `///` 文档注释（用 `missing_docs` lint 强制）
- 非 `pub` 项通常不需要文档注释（除非逻辑复杂）
- `//!` 用于 crate 级（`lib.rs`/`main.rs` 顶部）和模块级（`mod.rs` 顶部）

## 2. crate 级文档（lib.rs / main.rs 顶部）

```rust
//! # MyLibrary
//!
//! 一句话描述这个 crate 做什么。
//!
//! 详细介绍：解决什么问题、核心特性、设计理念。
//!
//! ## 特性
//!
//! - 零拷贝解析
//! - 异步支持
//! - 无 unsafe
//!
//! ## 快速开始
//!
//! ```
//! use my_library::Parser;
//!
//! let parser = Parser::new();
//! let result = parser.parse("input").unwrap();
//! println!("{result}");
//! ```
//!
//! ## 设计决策
//!
//! 为什么选择 X 而非 Y...

// 或者用 #[doc] 属性（适合多行）
#[doc = "等价于 //! 的内容"]
```

**规则：**
- 第一行是简短摘要（一句话，以句号结尾），crates.io 和搜索结果显示这行
- 空一行后写详细描述
- 含快速示例，让用户立刻能用
- 列出关键特性

## 3. 项级文档规范

### 3.1 完整文档结构

```rust
/// 解析输入字符串为 `Config`。
///
/// 这是一段简短的描述：函数做什么、返回什么。
///
/// # 参数
///
/// - `input`: TOML 格式的配置字符串
/// - `strict`: 是否启用严格模式（未知字段报错）
///
/// # 返回
///
/// 成功返回 `Config`，失败返回 `ConfigError`。
///
/// # 错误
///
/// 以下情况返回错误：
/// - 输入不是合法 TOML 语法 → `ConfigError::Parse`
/// - 包含未知字段（strict=true）→ `ConfigError::UnknownField`
/// - 包含无效值（如端口超出范围）→ `ConfigError::InvalidValue`
///
/// # Panics
///
/// 如果 `input` 包含 NUL 字节则 panic。调用者应预先过滤。
///
/// # 示例
///
/// 基本用法：
/// ```
/// use my_library::parse_config;
///
/// let config = parse_config(r#"
///     host = "localhost"
///     port = 8080
/// "#, false)?;
/// assert_eq!(config.port, 8080);
/// # Ok::<(), my_library::ConfigError>(())
/// ```
///
/// 严格模式拒绝未知字段：
/// ```
/// use my_library::parse_config;
///
/// let result = parse_config("unknown = 1", true);
/// assert!(result.is_err());
/// ```
///
/// # 另见
///
/// - [`Config`]
/// - [`ConfigError`]
pub fn parse_config(input: &str, strict: bool) -> Result<Config, ConfigError> {
    // ...
}
```

### 3.2 必备章节（按 API 类型）

| API 类型 | `# Examples` | `# Errors` | `# Panics` | `# Safety` |
|---------|:---:|:---:|:---:|:---:|
| 返回 `Result` 的函数 | ✓ 必备 | ✓ **必备** | 视情况 | — |
| 可能 panic 的函数 | ✓ 必备 | — | ✓ **必备** | — |
| `unsafe fn` | ✓ 必备 | 视情况 | 视情况 | ✓ **必备** |
| 普通 `pub fn` | ✓ 必备 | — | 视情况 | — |
| `pub struct`/`enum` | ✓ 建议 | — | — | — |
| `pub trait` | ✓ 建议 | — | — | — |

**核心约定：**
- **返回 `Result` 的函数必须写 `# Errors`**——列出所有错误变体及触发条件
- **可能 panic 的函数必须写 `# Panics`**——列出所有 panic 条件
- **`unsafe fn`/`unsafe impl` 必须写 `# Safety`**——列出调用者必须保证的不变量

### 3.3 `# Errors` 章节（C-FAIL-DOC）

```rust
/// 从文件加载用户配置。
///
/// # Errors
///
/// 返回 [`ConfigError`] 当：
/// - 文件不存在或不可读 → `Io` 变体
/// - 文件内容非合法 TOML → `Parse` 变体
/// - 必填字段缺失 → `MissingField`
pub fn load_config(path: &Path) -> Result<Config, ConfigError> { ... }
```

**规则：**
- 枚举每一个错误变体及其触发条件
- 不要只写"返回错误"——调用者需要知道哪些情况会失败
- 优先用 `Result` 而非 panic，除非是不可恢复的程序错误

### 3.4 `# Panics` 章节

```rust
/// 计算两个日期间的天数。
///
/// # Panics
///
/// 如果 `end < start` 则 panic。调用者应预先检查顺序。
pub fn days_between(start: NaiveDate, end: NaiveDate) -> u32 {
    assert!(end >= start, "end must be >= start");
    // ...
}
```

**何时该 panic（而非 Result）：**
- 前置条件违反是程序 bug（如索引越界、空切片取首元素）
- 不变量被破坏（理论上不可能的状态）
- `unwrap` 已知正确的 `Option`/`Result`（如静态数据解析）

**规则：** 即使是"合理的 panic"也要在文档声明，让调用者知道边界。

### 3.5 `# Safety` 章节（unsafe 必备）

```rust
/// 读取未对齐的 `u32`。
///
/// # Safety
///
/// 调用者必须保证：
/// - `ptr` 非空
/// - `ptr` 指向的 4 字节内存已初始化
/// - 该内存在此函数返回前不会被其他线程并发修改
pub unsafe fn read_unaligned_u32(ptr: *const u8) -> u32 {
    std::ptr::read_unaligned(ptr as *const u32)
}
```

**规则：**
- 逐条列出调用者必须保证的不变量
- 用 SAFETY 注释风格（便于审查和工具识别）
- 详见 [unsafe-rust.md](unsafe-rust.md)

## 4. rustdoc 链接语法

### 4.1 引用其他项

```rust
/// 解析输入为 [`Config`]。
///             ^^^^^^^^ 自动链接到 Config 类型

/// 返回 [`Vec`] of [`Token`]。
///     ^^^^        ^^^^^^ 标准库和本 crate 项都可链接

/// 用 [`Config::new`] 创建默认配置。
///     ^^^^^^^^^^^ 方法引用

/// 错误类型见 [`enum@ConfigError`]。
///              ^^^^^^  显式标注 enum，避免歧义

/// 实现了 [`Read`] trait。
/// ^^^^^  trait 链接
```

### 4.2 链接形式

```rust
/// [`Config`]            — 简写，自动推断
/// [`Config::parse`]     — 方法
/// [`crate::Config`]     — 完整路径
/// [`String`][std::string::String] — 显式路径
/// [`text`][Config]      — 显示文字 text，链接到 Config
/// <https://doc.rust-lang.org>  — URL 自动链接
```

### 4.3 intra-doc 链接的优势

```rust
/// 返回 [`Token`] 列表。
///
/// 用 [`Parser::tokenize`] 解析。
///
/// 错误见 [`ConfigError::Parse`]。
///
/// [std 的 Vec]: std::vec::Vec
/// 使用 [std 的 Vec]。
pub fn tokens(&self) -> Vec<Token> { ... }
```

**规则：** 用 intra-doc 链接（`[`项名`]`）而非手写 URL——重构改名时编译器会报错提醒更新。

## 5. 文档测试（Doc Tests）

### 5.1 文档中的代码块自动成为测试

```rust
/// # 示例
/// ```
/// use my_library::Parser;
///
/// let parser = Parser::new();
/// let tokens = parser.tokenize("fn main");
/// assert_eq!(tokens.len(), 2);
/// ```
pub fn tokenize(&self, input: &str) -> Vec<Token> { ... }
```

运行：`cargo test --doc`（或 `cargo test` 自动包含）。

### 5.2 隐藏辅助行（`#`）

```rust
/// ```
/// # use my_library::Parser;       // # 开头的行不显示在文档，但参与编译
/// # let parser = Parser::new();
/// let tokens = parser.tokenize("fn");
/// assert!(!tokens.is_empty());
/// ```
```

用于隐藏 `use`、变量初始化等样板，让文档聚焦核心用法。

### 5.3 处理 `Result` 返回的示例

```rust
/// ```
/// use my_library::parse_config;
///
/// let config = parse_config("host = localhost")?;
/// assert_eq!(config.host, "localhost");
/// # Ok::<(), my_library::ConfigError>(())
/// ```
```

末尾 `# Ok::<(), _>(())` 让示例函数返回 `Result`，使 `?` 可用。

### 5.4 跳过编译的示例

```rust
/// 仅展示，不编译不测试：
/// ```no_run
/// let result = server.listen();  // 会阻塞，测试时不能真跑
/// ```
///
/// 不编译（用于伪代码）：
/// ```compile_fail
/// let x: u32 = "hello";  // 故意写错，确保编译失败
/// ```
///
/// 忽略特定行输出：
/// ```rust,ignore
/// let x = some_platform_specific_thing();
/// ```
```

| 标记 | 行为 |
|------|------|
| （无） | 编译 + 运行 + 断言 |
| `no_run` | 编译但不运行（阻塞/死循环场景） |
| `ignore` | 不编译不运行（条件编译/平台特定） |
| `compile_fail` | 编译必须失败（测试错误信息） |
| `should_panic` | 运行必须 panic |
| `edition2018`/`edition2021` | 指定 edition |

### 5.5 文档测试的取舍

**优点：** 文档即测试，示例永远新鲜。
**缺点：**
- 编译慢（每个代码块独立编译为可执行文件）
- 复杂示例维护成本高

**策略：**
- 简单 API 用文档测试
- 复杂流程放 `examples/` 目录（单独编译，更快）

## 6. 其他 rustdoc 特性

### 6.1 警告/提示框

```rust
/// > ⚠️ **Warning**: 此函数会清空整个缓存，谨慎使用。
///
/// > **Note**: 此函数在 1.0 版本后标记为 deprecated。

/// 用属性控制提示样式：
#[doc = "支持 Markdown 所有语法，包括表格、列表、代码。"]
```

### 6.2 `#[doc(hidden)]` 隐藏项

```rust
// 实现细节需要 pub 但不想暴露给文档
#[doc(hidden)]
pub mod internal {  // 不出现在文档，但仍可被代码引用
    pub fn helper() {}
}
```

### 6.3 `#[doc(alias = "...")]` 搜索别名

```rust
/// 排序集合。
#[doc(alias = "sort")]
#[doc(alias = "order")]
pub fn arrange<T: Ord>(v: &mut [T]) { ... }
// 搜索 "sort" 或 "order" 都能找到这个函数
```

### 6.4 `#[doc(cfg)]` 平台/特性标注

```rust
/// 仅在 Linux 上可用。
#[cfg(target_os = "linux")]
#[doc(cfg(target_os = "linux"))]
pub fn epoll_create() -> i32 { ... }
// 文档会显示 "Linux" 标签

/// 需要 "async" feature。
#[cfg(feature = "async")]
#[doc(cfg(feature = "async"))]
pub async fn fetch(url: &str) -> String { ... }
```

### 6.5 `#[deprecated]` 弃用标注

```rust
#[deprecated(
    since = "1.2.0",
    note = "改用 `new_with_config`，支持更多选项"
)]
pub fn new() -> Self {
    Self::new_with_config(Config::default())
}
// 文档显示弃用警告 + 替代方案
```

## 7. 文档质量 lint

### 7.1 强制文档存在

```toml
# Cargo.toml
[lints.rust]
missing_docs = "warn"   # 所有 pub 项必须有文档
```

```rust
// lib.rs 顶部加，对整个 crate 生效
#![warn(missing_docs)]
```

### 7.2 clippy 文档 lint

```toml
[lints.clippy]
# 文档相关
missing_docs_in_private_items = "warn"  # 私有项也要文档
missing_errors_doc = "warn"             # Result 函数必须有 # Errors
missing_panics_doc = "warn"             # 可能 panic 的函数必须有 # Panics
missing_safety_doc = "warn"             # unsafe 必须有 # Safety
doc_markdown = "warn"                   # 代码标识用反引号
needless_doctest_main = "warn"          # 不需要 fn main 包装
```

### 7.3 文档检查命令

```bash
cargo doc --no-deps --open      # 生成并查看文档
cargo doc --document-private-items  # 含私有项（自审用）
cargo test --doc                 # 运行文档测试
cargo clippy -- -W clippy::doc_markdown  # 文档 lint
cargo +nightly rustdoc -- -Z unstable-options --check  # 检查 intra-doc 链接有效性
```

## 8. 文档注释检查清单

### 必备性
- [ ] 每个 `pub` 项都有 `///` 文档（`missing_docs` lint 强制）
- [ ] crate 级 `//!` 文档（lib.rs 顶部，含摘要 + 特性 + 快速开始）
- [ ] 模块级 `//!` 文档（每个 mod.rs 顶部）

### 章节完整性
- [ ] 返回 `Result` 的函数有 `# Errors`（列出所有错误变体）
- [ ] 可能 panic 的函数有 `# Panics`（列出所有 panic 条件）
- [ ] `unsafe` 项有 `# Safety`（列出所有不变量）
- [ ] 每个 API 有 `# Examples`（可运行）

### 链接
- [ ] 用 intra-doc 链接（`[`项名`]`）而非手写 URL
- [ ] 链接到的项实际存在（重构改名时编译器会报错）
- [ ] 引用标准库用 `[`Vec`]` 等

### 文档测试
- [ ] 示例代码可编译可运行（`cargo test --doc` 通过）
- [ ] 辅助行用 `#` 隐藏（不污染文档）
- [ ] 阻塞示例用 `no_run`
- [ ] 复杂流程移到 `examples/`

### 语法与质量
- [ ] 代码标识用反引号（`Config` 而非 Config）
- [ ] 平台/特性相关项用 `#[doc(cfg)]` 标注
- [ ] 弃用项用 `#[deprecated]` + note 说明替代
- [ ] 实现细节用 `#[doc(hidden)]` 隐藏

### 命令检查
- [ ] `cargo doc` 无警告
- [ ] `cargo test --doc` 全通过
- [ ] clippy 文档 lint 无警告

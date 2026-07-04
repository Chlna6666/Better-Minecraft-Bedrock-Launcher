---
name: rust-design-conventions
description: >
  Rust 全栈设计、函数实现、模块边界、API 语义、性能和工程规范指南。
  当用户涉及任何 Rust 任务时触发：编写或修改 Rust 函数、方法、trait
  impl、模块、crate、Cargo.toml、测试、文档、错误处理、生命周期、所有权、
  async/Tokio、并发、unsafe/FFI、零拷贝、性能优化、lint/clippy、API 设计、
  SemVer、依赖管理、MSRV、no_std、feature matrix、跨平台/交叉编译、
  项目结构、文件布局、重构和代码审查。默认要求代码严谨、idiomatic、
  边界清晰、借用优先、少 clone、少分配、跨平台、可测试、可维护。
---

# Rust 设计规范

将本技能作为 Rust 代码变更的执行协议。先确定语义和边界，再写代码；
先用类型系统表达不变量，再考虑运行时检查；先测量和验证，再声称优化。

## 使用顺序

如果同时触发多个 Rust 技能，按这个顺序使用：

1. 用本技能确定架构边界、API 语义、所有权模型、模块位置和验证范围。
2. 用 `rust-best-practices` 收紧 idiom、错误处理、测试、lint 和代码细节。
3. 只读取当前任务需要的 `references/` 文件，不一次性加载所有参考资料。

## Rust 变更协议

在修改 Rust 代码前，先完成这些判断：

1. 定位边界：目标 crate、模块、公开 API、调用方、现有测试和错误类型。
2. 定义语义：函数职责、输入所有权、输出含义、失败模式、不变量、副作用。
3. 选择抽象：普通函数、方法、trait、泛型、枚举、newtype、builder 或类型状态。
4. 评估性能：是否在热路径，是否涉及分配、clone、锁、I/O、async 或大数据。
5. 检查构建约束：MSRV、edition、feature、no_std、目标平台和依赖策略。
6. 安排验证：需要哪些单元测试、集成测试、doc test、clippy 或 benchmark。

默认最小变更：只改当前任务必须修改的模块。不要顺手重构、升级依赖、
改配置、改公共 API 或扩大可见性，除非这是完成任务的必要条件。

## 函数实现语义门禁

写每个 Rust 函数、方法或 trait impl 时，逐项检查：

- 名称表达行为；`is_`/`has_`/`can_`/`should_` 只返回 `bool`。
- 参数默认借用：`&T`、`&mut T`、`&str`、`&[T]`、`&Path`。
- 只有在存储、消费、跨线程移动或明确转移所有权时才接收 owned 类型。
- 不用 `.clone()` 修复 borrow checker；先重新审视所有权边界。
- 缺失值用 `Option<T>`，可恢复失败用 `Result<T, E>`，不变量破坏才 panic。
- 生产代码不使用 `unwrap()`；`expect()` 必须说明无法失败的不变量。
- 避免 bool mode 参数；用 enum 或拆成语义清晰的两个函数。
- 返回集合前考虑是否能返回 slice、iterator 或 caller-owned buffer。
- 热路径避免不必要分配、中间 `collect()`、动态分发和锁持有。
- async 函数不在持锁状态下 `.await`；阻塞 I/O 使用 `spawn_blocking` 或异步 API。
- unsafe 只能用于明确无法用安全 Rust 表达的边界，并写完整 `SAFETY` 依据。

## 跨平台与构建门禁

- 文件系统、进程、环境变量边界使用 `Path`/`PathBuf`/`OsStr`/`OsString`。
- 不假设路径分隔符、换行符、大小写敏感性、可执行后缀、Unix 权限或信号语义。
- 平台差异用 `#[cfg(...)]`、目标特定模块或 `target.'cfg(...)'.dependencies` 隔离。
- 字节协议、网络协议和持久化格式必须显式处理大小端。
- 使用 SIMD、原子、指针宽度、FFI 或系统 API 前先检查 `target_arch`、`target_feature`、`target_pointer_width` 和 ABI。
- 维护已有 `no_std` 支持；新增 `std` 依赖必须经过 `std` feature 或明确说明不再支持。
- 新 Rust API 必须符合 `rust-version`；提高 MSRV 视为影响下游的兼容性变更。
- feature 应该小而可组合；互斥 feature 要在编译期 `compile_error!`，不要让行为运行时才失败。
- 新依赖先评估必要性、许可证、维护状态、MSRV、体积、编译时间、安全公告和传递依赖。

## 类型与 API 设计门禁

- 用 newtype 区分语义相同的原始类型，例如 `UserId(u64)` 和 `OrderId(u64)`。
- 用 enum 表示互斥状态，不用多个 bool 拼出状态机。
- 让无效状态在类型层不可表示；必要时使用 builder 或类型状态模式。
- 公共结构体字段默认私有，通过方法暴露稳定语义。
- 公共 API 避免泄漏内部集合、锁、数据库连接、runtime 或第三方实现细节。
- 公共类型通常 derive `Debug`，敏感字段手写 `Debug`。
- 公共返回值若不应忽略，添加 `#[must_use]`。
- 库使用结构化错误；应用层可用上下文丰富的 anyhow 错误。
- breaking change 需要明确说明影响面，必要时使用 `#[non_exhaustive]` 预留扩展。

## 模块与文件边界

- `main.rs` 保持薄入口；业务逻辑放入 `lib.rs` 或内部模块。
- `lib.rs` / `mod.rs` 只编排和 re-export，不承载大量实现。
- 模块名使用 `snake_case` 单数，职责聚焦。
- 可见性最小化：优先私有，其次 `pub(crate)`，最后才是 `pub`。
- 不为了测试把私有 API 改成 `pub`；用模块内测试或 `#[path]` 外置测试文件。
- 单函数超过约 50 行要拆分；单实现文件超过约 500 行要重新评估职责。
- 避免一函数一文件的过度碎片化；按类型族或变更理由组织文件。

## 性能默认规则

- 借用优于 clone，迭代器优于中间集合，预分配优于反复扩容。
- 算法和数据结构优先于微优化。
- 先定义资源目标：吞吐、延迟、峰值内存、分配次数、二进制体积或编译时间。
- 大输入默认流式处理；不要把文件、网络 body、日志、CSV、JSONL 或数据库结果无界读入内存。
- API 优先表达数据流：能返回 iterator/slice/Cow/Bytes 就不要强制分配 `Vec`/`String`。
- 热路径复用缓冲区或让调用方传入 buffer，避免循环内反复分配和释放。
- 关注峰值内存：避免过大预分配、长期持有临时集合、无界 cache、无界 channel。
- 大 struct/enum 检查布局和 variant 大小；必要时 boxing 冷路径大字段，避免放大每个实例。
- 读多写少考虑 `RwLock`；初始化后不变考虑 `OnceLock` / `LazyLock`。
- 高并发 map 可考虑 `DashMap`，但新增依赖必须有明确收益。
- 并发设计必须有背压：限制任务数、队列长度、连接数和批量大小。
- async 热路径避免 `Box<dyn Future>`、阻塞调用、锁跨 await 和无界 spawn。
- 大文件和流式数据优先 `BufReader` / `BufWriter` / `io::copy`。
- 网络/文件转发优先评估 `io::copy`、vectored I/O 或平台合适的零拷贝机制。
- `#[inline(always)]`、`target-cpu=native`、自定义 allocator、SIMD 和 unsafe 优化必须有 benchmark/profile 支撑。
- 低资源目标下同时评估运行时内存、CPU、二进制体积、冷启动、编译时间和依赖数量。
- 只有 benchmark、profile 或明确复杂度变化能支撑“性能优化”结论。

## 测试与文档门禁

- 单元测试覆盖边界条件、错误路径、不变量和回归。
- 集成测试只通过公共 API 验证跨模块行为。
- 测试名称表达场景和期望结果，例如 `parse_empty_input_returns_error`。
- 算法、解析器、编码器、序列化和状态机优先考虑属性测试。
- 并发或 async 调度相关逻辑要考虑压力测试、确定性调度测试或项目已有的 `loom` 模型测试。
- unsafe 或 FFI 变更要考虑 `miri`、sanitizer、fuzz 或 ABI/布局断言。
- 公共 API 写 `///` 文档；返回 `Result` 的公共函数写 `# Errors`。
- 可能 panic 的公共函数写 `# Panics`；unsafe API 写 `# Safety`。
- 修复 bug 时优先加入能失败的回归测试。

## 验证命令

优先使用项目已有脚本或 CI 命令。没有约定时，选择最小但有意义的集合：

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features
cargo test --doc
```

如存在 feature matrix、workspace、MSRV、benchmark、unsafe/FFI 或 semver 风险，且对应工具可用，按任务风险补充：

```bash
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo bench
cargo semver-checks
cargo audit
cargo deny check
cargo tree -d
cargo test --no-default-features
cargo +nightly miri test
```

## 参考路由

根据任务场景读取最相关的 1-2 个参考文件：

| 场景 | 读取 |
| --- | --- |
| 内存布局、stack/heap、repr、allocator | [references/memory-and-layout.md](references/memory-and-layout.md) |
| 生命周期、`'static`、HRTB、型变、自引用 | [references/lifetimes.md](references/lifetimes.md) |
| 多线程、`Send`/`Sync`、锁、channel、atomic | [references/concurrency.md](references/concurrency.md) |
| async、Future、Pin、Tokio、select、阻塞问题 | [references/async-programming.md](references/async-programming.md) |
| 零拷贝、`Bytes`、`Cow`、协议解析、sendfile | [references/zero-copy.md](references/zero-copy.md) |
| 泛型、trait object、vtable、inline、const fn | [references/zero-cost-abstractions.md](references/zero-cost-abstractions.md) |
| unsafe、FFI、裸指针、unsafe impl | [references/unsafe-rust.md](references/unsafe-rust.md) |
| 性能优化流程、benchmark、分配和热路径 | [references/performance-optimization.md](references/performance-optimization.md) |
| 性能陷阱、隐藏 clone、低效集合、锁粒度 | [references/performance-pitfalls.md](references/performance-pitfalls.md) |
| 错误处理、类型设计、函数设计、可读性 | [references/code-robustness.md](references/code-robustness.md) |
| 命名规范、RFC 430、getter、转换前缀 | [references/naming-conventions.md](references/naming-conventions.md) |
| 测试放置、封装边界、mock、proptest | [references/testing-standards.md](references/testing-standards.md) |
| 文件布局、模块化、`mod.rs`、拆分阈值 | [references/file-layout.md](references/file-layout.md) |
| Cargo、features、workspace、cfg、MSRV、no_std、交叉编译 | [references/cargo-build-features.md](references/cargo-build-features.md) |
| rustdoc、doc test、`# Errors`、`# Safety` | [references/documentation.md](references/documentation.md) |
| lint、clippy、CI 质量门禁 | [references/lint-and-clippy.md](references/lint-and-clippy.md) |
| 公共 API、SemVer、trait 边界、breaking change | [references/api-design.md](references/api-design.md) |
| 依赖、漏洞、许可证、MSRV、Cargo.lock | [references/dependency-management.md](references/dependency-management.md) |
| 宏、derive、属性宏、DSL | [references/macros.md](references/macros.md) |

## 快速自检

完成 Rust 修改前，确认：

- 语义：函数名、参数、返回值和错误类型表达真实意图。
- 所有权：没有为通过编译而加入的 clone、owned 参数或过宽生命周期。
- 边界：没有扩大模块职责、公共 API、依赖或配置范围。
- 平台：没有隐藏的 Windows/Linux/macOS、大小端、路径、feature 或 MSRV 假设。
- 健壮性：无效状态尽量由类型系统排除，错误路径可测试。
- 性能：资源目标明确，无明显多余分配、全量缓冲、重复计算、锁跨 await、无界并发或热路径动态分发。
- 文档：公共 API 和 unsafe 边界说明完整。
- 验证：格式化、lint、测试或基准命令已运行，未运行项说明原因。

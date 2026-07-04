# 统一测试规范

> 测试是 Rust 项目质量与可维护性的基石。本规范解决测试放置的**双向难题**：既不能把所有测试塞进根 `tests/`（为测私有逻辑被迫把内部 API 改成 `pub`/`pub(crate)`，破坏封装边界），也不能让所有测试堆在源码 `.rs` 文件里（源码臃肿、可读性下降）。核心是**按测试类型选择放置位置，并配合文件拆分阈值控制源码体积**。

## 1. 三层测试体系

```
       ╱ E2E ╲          ← 5%：完整流程，慢，脆弱
      ╱ 集成  ╲         ← 25%：验证模块协作，通过公共 API
     ╱ 单元测试 ╲       ← 70%：单个函数/类型，快，确定，可测私有逻辑
```

| 层级 | 目标 | 位置 | 访问权限 | 速度 |
|------|------|------|---------|------|
| 单元测试 | 单个函数/类型的内部正确性 | 贴近被测模块（`src/` 内的 `mod tests`） | 可访问私有项 | 极快 |
| 集成测试 | 模块协作 + 公共 API 契约 | `tests/*.rs`（crate 根目录） | 只能访问 `pub` API | 中 |
| E2E | 端到端业务流程 | `tests/e2e/` 或独立 crate | 通过 binary 入口 | 慢 |

## 2. 测试放置：双向平衡（核心）

测试放置要同时避免两个极端：

```
   ❌ 极端 A：所有测试塞进 tests/        ❌ 极端 B：所有测试堆在源码 .rs 里
   ├─ 为测私有改 pub(crate)/pub          ├─ 源码文件几百上千行
   ├─ 破坏封装边界                        ├─ 实现/测试混杂，阅读要翻屏
   ├─ 冻结内部实现                        ├─ 改实现要在大文件里定位
   └─ API 膨胀、文档污染                   └─ IDE 跳转、搜索被测试干扰

   ✓ 正解：按类型分流 + 文件拆分阈值
   ├─ 单元测试 → 贴近被测模块（访问私有，不破坏封装）
   │   └─ 用 #[path] 外置到独立文件，避免源码臃肿
   ├─ 集成测试 → tests/（只测公共 API）
   └─ 大型模块先拆分实现文件，再决定测试外置
```

### 2.1 三条放置规则

| 测试类型 | 放置位置 | 理由 |
|---------|---------|------|
| **私有逻辑的单元测试** | 被测模块内（`#[cfg(test)] mod tests`） | 必须访问私有项，外置到 `tests/` 会被迫改 `pub` |
| **公共 API 的单元测试** | 被测模块内（同上，保持一致） | 单元测试集中管理，便于定位 |
| **集成测试 / E2E** | `tests/*.rs` | 通过公共 API 验证协作，不应访问内部 |

### 2.2 控制源码体积：`#[path]` 外置测试

**关键洞察：** `#[cfg(test)] mod tests` 仍属被测模块（能访问私有），但可以用 `#[path]` 把测试代码物理放到独立文件——既保护封装，又不让源码臃肿。

```rust
// src/parser/lexer.rs  ← 只放实现
pub fn tokenize(input: &str) -> Vec<Token> { /* 实现 */ }
fn is_keyword(s: &str) -> bool { /* 私有函数 */ }

#[cfg(test)]
#[path = "lexer_tests.rs"]  // 测试物理外置，逻辑仍属本模块
mod tests;
```

```rust
// src/parser/lexer_tests.rs  ← 测试独立文件
use super::*;  // ✓ 访问父模块的私有项（is_keyword）

#[test]
fn tokenize_handles_keywords() {
    assert_eq!(tokenize("fn let").len(), 2);
}

#[test]
fn is_keyword_recognizes_fn() {
    assert!(is_keyword("fn"));  // ✓ 测私有，无需改 pub
}
```

**编译期保证：** `#[cfg(test)]` 让测试代码只在 `cargo test` 时编译，发布二进制完全不包含测试。

### 2.3 拆分阈值：何时内联 vs 外置

按**测试行数 + 源码行数**判断：

| 场景 | 实现+测试总量 | 策略 |
|------|--------------|------|
| 小模块（工具函数、简单类型） | 实现 <100 行，测试 <50 行 | **内联**：`#[cfg(test)] mod tests { ... }` 放源码底部 |
| 中等模块 | 实现 100-300 行，测试 50-200 行 | **外置**：用 `#[path]` 把测试放到 `_tests.rs` |
| 大模块 | 实现 >300 行 或 测试 >200 行 | **先拆实现**：把实现本身拆成多个文件，再为每个子文件配测试 |

```rust
// ❌ 反模式：让单文件膨胀到 1000+ 行
// src/parser.rs  ← 实现 600 行 + 内联测试 400 行 = 1000 行
// 阅读困难，改任何一处都要在大文件里翻

// ✓ 正解：按职责拆分实现，每个子文件配外置测试
// src/parser/
//   mod.rs              ← re-export，<50 行
//   lexer.rs            ← 实现 <300 行
//   lexer_tests.rs      ← 测试（#[path] 引入）
//   parser_state.rs     ← 实现 <200 行
//   parser_state_tests.rs
```

> 实现文件拆分的详细规范见 [file-layout.md](file-layout.md)。

### 2.4 内联测试（小模块适用）

测试很少时，内联在源码底部可接受，避免为几个测试单独建文件：

```rust
// src/utils/id.rs  ← 小工具模块
pub fn next_id() -> u64 { /* 5 行实现 */ }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_id_is_monotonic() {
        let a = next_id();
        let b = next_id();
        assert!(b > a);
    }
}
```

**判断标准：** 实现与测试加起来 <150 行，且测试 <50 行 → 内联可接受。

### 2.5 集成测试放 `tests/`

集成测试只通过公共 API 验证模块协作，**不访问私有项**：

```
my-crate/
├── src/
│   └── lib.rs
└── tests/
    ├── parser_integration.rs    # 测 parser 公共 API
    ├── full_pipeline.rs         # 端到端流程
    └── common/
        └── mod.rs               # 集成测试共享代码
```

```rust
// tests/parser_integration.rs
use my_crate::Parser;  // 只 import pub API

#[test]
fn parser_handles_full_program() {
    let parser = Parser::new();
    let ast = parser.parse("fn main() { 42 }").unwrap();
    assert_eq!(ast.functions().len(), 1);  // ✓ 测公共 API
}
```

### 2.6 反模式：为测试把私有 API 改成 pub

```rust
// ❌ 反模式：为了在 tests/ 测私有逻辑，把内部 API 改成 pub
// src/parser/lexer.rs
pub fn is_keyword(s: &str) -> bool { /* ... */ }
//  ^^^^ 本应是私有，仅为测试暴露

// tests/lexer_test.rs
use my_crate::parser::lexer::is_keyword;
#[test]
fn test_is_keyword() {
    assert!(is_keyword("fn"));
}
```

**危害：**
1. **破坏封装**：`is_keyword` 本应是内部实现细节，被迫暴露为公共 API
2. **冻结实现**：一旦暴露，外部用户可能依赖它，未来重构受限
3. **API 膨胀**：公共 API 被测试细节污染，文档混乱
4. **错误信号**：私有逻辑的测试应通过单元测试（贴近模块）验证，而非塞进 `tests/`

**正确做法：** 私有逻辑的测试放在被测模块的 `#[cfg(test)] mod tests`（可外置到 `_tests.rs` 避免源码臃肿）；只有公共 API 契约测试才进 `tests/`。

### 2.7 反模式：让源码文件无节制臃肿

```rust
// ❌ 反模式：测试全堆在源码里，文件膨胀
// src/service/user_service.rs
pub fn create_user(...) { ... }      // 实现 50 行
pub fn update_user(...) { ... }      // 实现 60 行
fn validate_email(...) { ... }       // 实现 40 行
// ... 更多实现共 400 行 ...

#[cfg(test)]
mod tests {
    // 800 行测试全堆在这里
    // 源码文件总共 1200+ 行，阅读和导航困难
}
```

**危害：**
1. **可读性下降**：实现与测试混杂，改实现要在巨文件里翻屏
2. **导航困难**：IDE 符号列表、搜索被测试淹没
3. **合并冲突**：多人改同一巨文件易冲突
4. **认知负担**：读者无法快速区分"哪些是产品代码"

**正确做法：** 测试 >200 行时用 `#[path]` 外置；实现 >300 行时先拆分实现文件（详见 [file-layout.md](file-layout.md)）。

## 3. 集成测试共享代码

`tests/` 下每个文件是独立 crate，不能互相 `use`。共享代码用 `mod`：

```
tests/
├── common/
│   └── mod.rs          # 共享辅助函数
├── parser_integration.rs
└── end_to_end.rs
```

```rust
// tests/common/mod.rs
pub fn setup_test_db() -> TestDb { /* ... */ }
pub fn sample_input() -> &'static str { "fn main() {}" }
```

```rust
// tests/parser_integration.rs
mod common;  // 引入共享模块
use common::sample_input;

#[test]
fn parser_handles_sample() {
    let input = sample_input();
    // ...
}
```

**注意：** `tests/common/` 不会被当作独立测试文件（Cargo 只把 `tests/*.rs` 顶层文件当测试目标，目录内文件被 `mod` 引入）。

## 4. 文档测试（Doc Tests）

文档测试既是文档又是测试，验证公共 API 用法：

```rust
/// 计算两个日期之间的工作日数（排除周末和节假日）。
///
/// # 示例
/// ```
/// use my_crate::business_days;
/// use chrono::NaiveDate;
///
/// let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
/// let end = NaiveDate::from_ymd_opt(2024, 1, 8).unwrap();
/// assert_eq!(business_days(start, end, &[]), 5);
/// ```
pub fn business_days(start: NaiveDate, end: NaiveDate, holidays: &[NaiveDate]) -> u32 {
    // ...
}
```

```bash
cargo test --doc          # 只运行文档测试
cargo test                # 运行所有测试（含文档测试）
```

**原则：** 文档测试只展示公共 API 的正确用法，不测内部实现。

## 5. 测试命名规范

### 5.1 命名表达意图

```rust
// ❌ 含糊命名
fn test_user() { /* ... */ }
fn test1() { /* ... */ }
fn test_parser() { /* ... */ }

// ✓ 命名表达：被测对象_场景_期望结果
#[test]
fn user_create_with_valid_email_succeeds() { /* ... */ }

#[test]
fn user_create_with_invalid_email_returns_error() { /* ... */ }

#[test]
fn transfer_with_insufficient_balance_returns_error() { /* ... */ }

#[test]
fn parser_handles_empty_input_without_panic() { /* ... */ }
```

### 5.2 命名模式

```
<被测对象>_<前置条件/场景>_<期望结果>

示例：
cart_add_item_increments_total
parse_url_with_invalid_scheme_returns_error
connection_pool_acquire_when_exhausted_blocks_then_times_out
sort_stable_preserves_equal_element_order
```

### 5.3 命名禁用词

```rust
// ❌ 这些词不传达信息
fn test_success() {}           // 成功是什么意思？
fn test_basic() {}              // basic 没有信息
fn test_edge_case() {}         // 哪个 edge case？
fn test_happy_path() {}         // 哪个 happy path？

// ✓ 具体描述
fn parse_returns_ok_for_valid_utf8() {}
fn parse_returns_error_for_invalid_byte_0xff() {}
```

## 6. 测试结构：Given-When-Then

```rust
#[test]
fn cart_add_item_increments_total() {
    // Given（准备）
    let mut cart = ShoppingCart::new();
    let item = Item::new("书", Money::from_dollars(20));

    // When（执行）
    cart.add(item);

    // Then（断言）
    assert_eq!(cart.total(), Money::from_dollars(20));
    assert_eq!(cart.item_count(), 1);
}
```

**原则：** 每个测试只验证一个行为。多个断言可以，但必须围绕同一行为。

## 7. 断言规范

### 7.1 断言选择

```rust
// 相等
assert_eq!(result, expected);
assert_ne!(result, unexpected);

// 布尔
assert!(is_valid);
assert!(!has_errors);

// 恐慌测试（应测 panic 的场景）
#[test]
#[should_panic(expected = "index out of bounds")]
fn slice_index_out_of_bounds_panics() {
    let v = [1, 2, 3];
    let _ = v[10];
}

// Result 返回的测试（无需 unwrap）
#[test]
fn parse_valid_url_returns_ok() -> Result<(), UrlError> {
    let url = Url::parse("https://example.com")?;
    assert_eq!(url.host(), "example.com");
    Ok(())
}

// 自定义失败消息
assert_eq!(status, 200, "expected 200 OK, got {status}");
```

### 7.2 错误信息可读

```rust
// ❌ 失败信息不清晰
assert!(result.is_ok());

// ✓ 失败时显示上下文
assert!(result.is_ok(), "expected Ok, got Err: {:?}", result.err());

// ✓ 用 debug 格式输出差异
assert_eq!(actual, expected);  // assert_eq 自动显示两者
```

## 8. 测试不变量而非实现细节

```rust
// ❌ 测试实现细节（重构就坏）
#[test]
fn test_sort_uses_quickselect() {
    let comparisons = count_comparisons(&mut data);
    assert!(comparisons < 100);  // 依赖具体算法
}

// ✓ 测试行为（重构不坏）
#[test]
fn sort_result_is_sorted() {
    let mut data = vec![3, 1, 4, 1, 5, 9, 2, 6];
    sort(&mut data);
    assert_eq!(data, vec![1, 1, 2, 3, 4, 5, 6, 9]);  // 测试结果
}

// ✓ 测试不变量
#[test]
fn sort_preserves_length() {
    let mut data = vec![3, 1, 4];
    sort(&mut data);
    assert_eq!(data.len(), 3);  // 长度不变
}

#[test]
fn sort_is_idempotent() {
    let mut data = vec![3, 1, 2];
    sort(&mut data);
    let sorted = data.clone();
    sort(&mut data);  // 再排一次
    assert_eq!(data, sorted);  // 已排序的再排不变
}
```

## 9. 测试组织实践

### 9.1 完整项目结构

```
my-crate/
├── src/
│   ├── lib.rs
│   ├── parser/
│   │   ├── mod.rs
│   │   ├── lexer.rs              # 实现
│   │   ├── lexer_tests.rs        # 单元测试（方案 A）
│   │   ├── parser.rs
│   │   └── parser_tests.rs
│   └── ast/
│       ├── mod.rs
│       ├── node.rs
│       └── node_tests.rs
├── tests/                        # 集成测试
│   ├── common/
│   │   └── mod.rs                # 共享辅助
│   ├── parser_integration.rs
│   └── full_pipeline.rs
├── benches/                      # 基准测试
│   └── parser_bench.rs
└── examples/                     # 示例（也是可运行测试）
    └── basic_usage.rs
```

### 9.2 测试辅助函数

```rust
// src/parser/lexer_tests.rs
use super::*;

fn make_lexer(input: &str) -> Lexer {
    Lexer::new(input)
}

#[test]
fn lexer_skips_whitespace() {
    let mut lexer = make_lexer("  fn  ");
    let token = lexer.next_token().unwrap();
    assert_eq!(token.kind, TokenKind::Fn);
}

// 测试夹具（test fixture）
struct TestInput {
    source: String,
    expected_tokens: Vec<TokenKind>,
}

fn parse_fixtures() -> Vec<TestInput> {
    vec![
        TestInput { source: "fn".into(), expected_tokens: vec![TokenKind::Fn] },
        TestInput { source: "let x".into(), expected_tokens: vec![TokenKind::Let, TokenKind::Ident] },
    ]
}

#[test]
fn lexer_handles_fixtures() {
    for fixture in parse_fixtures() {
        let mut lexer = Lexer::new(&fixture.source);
        let tokens: Vec<_> = lexer.collect_tokens().unwrap();
        assert_eq!(
            tokens.iter().map(|t| t.kind).collect::<Vec<_>>(),
            fixture.expected_tokens,
            "failed for input: {}", fixture.source
        );
    }
}
```

## 10. 测试执行控制

### 10.1 忽略慢测试

```rust
#[test]
#[ignore = "需要网络连接，慢测试"]
fn fetch_from_real_api() {
    // 默认不运行
}

// 运行被忽略的测试
// cargo test -- --ignored
// cargo test -- --include-ignored
```

### 10.2 控制输出

```bash
cargo test                           # 所有测试
cargo test -- --nocapture            # 显示 println! 输出
cargo test -- --test-threads=1      # 单线程运行（调试用）
cargo test parser                    # 只运行名字含 parser 的测试
cargo test --test parser_integration # 只运行某集成测试文件
cargo test --doc                     # 只运行文档测试
cargo test --lib                     # 只运行单元测试
```

## 11. 测试隔离

### 11.1 避免测试间依赖

```rust
// ❌ 测试间有状态依赖
static mut COUNTER: u32 = 0;

#[test]
fn test1() {
    unsafe { COUNTER += 1; }
    assert_eq!(unsafe { COUNTER }, 1);  // 依赖执行顺序
}

#[test]
fn test2() {
    unsafe { COUNTER += 1; }
    assert_eq!(unsafe { COUNTER }, 2);  // 依赖 test1 先跑
}

// ✓ 每个测试独立
#[test]
fn counter_increments() {
    let mut counter = Counter::new();  // 每次新建
    counter.increment();
    assert_eq!(counter.value(), 1);
}
```

### 11.2 避免共享可变状态

```rust
// ❌ 全局可变状态污染测试
use std::sync::Mutex;
static SHARED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

#[test]
fn test1() {
    SHARED.lock().unwrap().push(1);
    assert_eq!(SHARED.lock().unwrap().len(), 1);  // 依赖 SHARED 为空
}

// ✓ 测试内局部状态
#[test]
fn test1() {
    let mut vec = Vec::new();  // 局部
    vec.push(1);
    assert_eq!(vec.len(), 1);
}
```

## 12. 测试金字塔比例

```
单元测试 70%      ─ 快、独立、可测私有逻辑、覆盖分支
集成测试 25%      ─ 验证模块协作、公共 API 契约
E2E       5%      ─ 验证完整业务流程
```

**判断准则：**
- 单元测试：测单个函数/类型，不依赖外部（文件、网络、数据库）
- 集成测试：测多个模块协作，可能依赖文件系统但不用真实网络
- E2E：测真实用户场景，可能依赖真实服务

## 13. 属性测试（Property-Based Testing）

用 `proptest` 生成大量随机输入验证不变量：

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn sort_preserves_length(ref input in prop::collection::vec(1..1000, 0..100)) {
        let mut data = input.clone();
        sort(&mut data);
        prop_assert_eq!(data.len(), input.len());
    }

    #[test]
    fn sort_result_is_sorted(ref input in prop::collection::vec(1..1000, 0..100)) {
        let mut data = input.clone();
        sort(&mut data);
        for i in 1..data.len() {
            prop_assert!(data[i-1] <= data[i]);
        }
    }

    #[test]
    fn parse_inverse_of_format(input in r"[a-z]{1,10}") {
        let parsed = parse(&input).unwrap();
        prop_assert_eq!(format(parsed), input);  // 不变量：parse(format(x)) == x
    }
}
```

**适用：** 算法不变量、序列化/反序列化、解析器、数学运算。

## 14. Mock 与依赖注入

### 14.1 用 trait 实现可测试性

```rust
// 定义 trait
pub trait UserRepository: Send + Sync {
    fn find_by_id(&self, id: u64) -> Result<User>;
    fn save(&self, user: &User) -> Result<()>;
}

// 生产实现
pub struct DbUserRepository { db: Db }
impl UserRepository for DbUserRepository { /* ... */ }

// 测试 Mock
pub struct MockUserRepository {
    pub users: RefCell<HashMap<u64, User>>,
}
impl UserRepository for MockUserRepository {
    fn find_by_id(&self, id: u64) -> Result<User> {
        self.users.borrow().get(&id).cloned()
            .ok_or(Error::NotFound)
    }
    fn save(&self, user: &User) -> Result<()> {
        self.users.borrow_mut().insert(user.id, user.clone());
        Ok(())
    }
}

// 测试时注入 Mock
#[test]
fn user_service_create_user_saves_to_repo() {
    let repo = MockUserRepository { users: RefCell::new(HashMap::new()) };
    let service = UserService::new(Arc::new(repo));

    service.create_user("Alice").unwrap();

    let repo = service.repo_as_mock();
    assert_eq!(repo.users.borrow().len(), 1);
}
```

### 14.2 mockall crate（自动生成 Mock）

```rust
use mockall::*;

#[automock]
trait UserRepository {
    fn find_by_id(&self, id: u64) -> Result<User>;
    fn save(&self, user: &User) -> Result<()>;
}

#[test]
fn user_service_returns_error_when_repo_fails() {
    let mut mock_repo = MockUserRepository::new();
    mock_repo.expect_find_by_id()
        .with(eq(42))
        .returning(|_| Err(Error::NotFound));

    let service = UserService::new(Arc::new(mock_repo));
    let result = service.get_user(42);
    assert!(matches!(result, Err(Error::NotFound)));
}
```

## 15. 测试规范检查清单

### 测试放置与封装边界（核心）
- [ ] 源码文件只放实现，单元测试迁移到独立测试文件（`#[path]`）或源码底部 `#[cfg(test)] mod tests`
- [ ] 单元测试留在被测模块内（可访问私有项），不被迫改 `pub`
- [ ] 集成测试放 `tests/`，只通过公共 API 测试
- [ ] 不把私有逻辑测试塞进 `tests/`（会破坏封装）
- [ ] 集成测试共享代码放 `tests/common/mod.rs`

### 测试组织
- [ ] 单元测试占 70%+，集成 25%，E2E 5%
- [ ] 每个测试只验证一个行为
- [ ] 测试间无状态依赖
- [ ] 慢测试用 `#[ignore]` 标注

### 命名
- [ ] 命名表达：被测对象_场景_期望结果
- [ ] 避免含糊词（test_success/test_basic/test_edge_case）
- [ ] panic 测试用 `#[should_panic(expected = "...")]`

### 断言
- [ ] 用 `assert_eq!` 而非 `assert!(a == b)`（差异显示更好）
- [ ] 失败信息含上下文
- [ ] 测试不变量而非实现细节

### 可测试性
- [ ] 外部依赖通过 trait 抽象（数据库、HTTP、文件系统）
- [ ] 测试用 Mock 替代真实依赖
- [ ] 时间用注入参数或 `mockall::mock_clock`
- [ ] 随机数用固定种子

### 高级
- [ ] 算法/解析器用 `proptest` 做属性测试
- [ ] 公共 API 有文档测试
- [ ] Bug 修复先写失败测试再修复

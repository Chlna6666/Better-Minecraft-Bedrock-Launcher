# 代码健壮性与可读性

> "程序首先是写给人读的，其次才是让机器执行。" —— Harold Abelson

性能和正确性是底线，但**可读性决定维护成本**。Rust 的类型系统是强大的可读性工具——好的 Rust 代码让错误状态在编译期不可能出现。

## 1. 错误处理健壮性

### 1.1 不要 unwrap / expect 在非测试代码

```rust
// ❌ 生产代码中的定时炸弹
fn process(data: &str) -> u64 {
    let n: u64 = data.parse().unwrap();  // 解析失败就 panic！
    n * 2
}

// ✓ 用 Result 显式处理
fn process(data: &str) -> Result<u64, ParseError> {
    let n: u64 = data.parse()?;
    Ok(n * 2)
}

// ✓ 真的能保证不会失败时用 expect + 说明原因
fn get_env(name: &str) -> String {
    std::env::var(name)
        .expect(&format!("必须设置 {name} 环境变量"))
        // expect 的消息帮助调试时快速定位
}

// 在 Cargo.toml 用 lint 强制
// [lints.clippy]
// unwrap_used = "deny"
// expect_used = "deny"
```

### 1.2 错误类型分层

```rust
// ❌ 用字符串作错误（丢失类型信息）
fn process() -> Result<(), String> {
    if bad { return Err("失败".into()); }
    Ok(())
}
// 调用者只能用字符串匹配，无法穷尽处理

// ✓ 用 thiserror 定义结构化错误
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("用户 {user_id} 不存在")]
    UserNotFound { user_id: u64 },
    #[error("权限不足：需要 {required:?}")]
    PermissionDenied { required: Vec<Permission> },
    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

// 调用者可以 match 穷尽处理
match process() {
    Ok(_) => (),
    Err(AppError::UserNotFound { user_id }) => { /* ... */ },
    Err(AppError::PermissionDenied { required }) => { /* ... */ },
    Err(e) => log::error!("其他错误: {e}"),
}
```

### 1.3 错误传播添加上下文

```rust
// ❌ 直接 ? 传播，丢失上下文
fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)?;  // 失败时只说 IO 错误
    Ok(serde_json::from_str(&content)?)
}

// ✓ 用 map_err / anyhow::Context 添加上下文
fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::ConfigRead(path.to_path_buf(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| AppError::ConfigParse(path.to_path_buf(), e))
}

// 或用 anyhow（应用程序层）
use anyhow::{Context, Result};
fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("解析配置文件失败: {}", path.display()))
}
```

### 1.4 不要用 panic 做错误处理

```rust
// ❌ 用 panic 控制流
fn divide(a: f64, b: f64) -> f64 {
    if b == 0.0 { panic!("除零错误"); }
    a / b
}

// ✓ 用 Result
fn divide(a: f64, b: f64) -> Result<f64, DivError> {
    if b == 0.0 { return Err(DivError::ZeroDivision); }
    Ok(a / b)
}

// panic 只用于：
// - 不变量违反（程序 bug，不可恢复）
// - 资源耗尽（OOM）
// - 测试断言
// - 解析静态字符串字面量（如 parse::<i32>("42").unwrap()，已知正确）
```

## 2. 类型驱动的健壮性

### 2.1 用 Newtype 防止混淆

```rust
// ❌ 原始类型混淆
fn transfer(from: u64, to: u64, amount: u64) { /* ... */ }
transfer(100, 200, 50);  // 哪个是 from？哪个是 amount？

// ✓ Newtype
#[derive(Debug, Clone, Copy)]
struct AccountId(u64);
#[derive(Debug, Clone, Copy)]
struct Amount(u64);

fn transfer(from: AccountId, to: AccountId, amount: Amount) { /* ... */ }
transfer(AccountId(100), AccountId(200), Amount(50));  // 类型保证不会传错
```

### 2.2 让无效状态不可表示

```rust
// ❌ 用 Option<bool> 表示三态，容易出错
struct User {
    is_active: Option<bool>,  // None 是什么意思？没设置？还是未确认？
}

// ✓ 用 enum 明确语义
enum AccountStatus {
    Pending,    // 待确认
    Active,     // 已激活
    Suspended,  // 已暂停
    Deleted,    // 已删除
}
struct User {
    status: AccountStatus,  // 状态明确
}

// ❌ 用两个独立 bool，可能产生无效组合
struct Subscription {
    is_paid: bool,
    is_active: bool,
    // 无效组合：is_paid=false, is_active=true（未付费却激活？）
}

// ✓ 用 enum 表示互斥状态
enum Subscription {
    Trial { expires_at: DateTime },
    Active { until: DateTime },
    Cancelled,
    Expired,
}
```

### 2.3 Builder 模式处理复杂构造

```rust
// ❌ 多参数构造，易错
fn new_server(host: &str, port: u16, tls: bool, cert: Option<&Path>,
              timeout: u64, max_conn: u32, log_level: &str) -> Server { /* ... */ }

// ✓ Builder 模式
struct ServerBuilder {
    host: String,
    port: u16,
    tls: bool,
    cert: Option<PathBuf>,
    timeout: Duration,
    max_conn: u32,
}

impl ServerBuilder {
    fn new(host: impl Into<String>, port: u16) -> Self {
        Self { host: host.into(), port, tls: false, cert: None,
               timeout: Duration::from_secs(30), max_conn: 100 }
    }
    fn with_tls(mut self, cert: impl AsRef<Path>) -> Self {
        self.tls = true; self.cert = Some(cert.as_ref().to_path_buf()); self
    }
    fn with_timeout(mut self, t: Duration) -> Self { self.timeout = t; self }
    fn build(self) -> Result<Server> { /* 验证 + 构造 */ }
}

let server = ServerBuilder::new("0.0.0.0", 443)
    .with_tls("/etc/cert.pem")
    .with_timeout(Duration::from_secs(10))
    .build()?;
```

### 2.4 类型状态模式（编译期保证流程）

```rust
// 让编译器保证调用顺序
struct Draft { content: String }
struct Reviewed { content: String, reviewer: String }
struct Published { content: String, reviewer: String, at: DateTime }

impl Draft {
    fn new(content: String) -> Self { Self { content } }
    fn review(self, reviewer: String) -> Reviewed {
        Reviewed { content: self.content, reviewer }
    }
}
impl Reviewed {
    fn publish(self) -> Published {
        Published { content: self.content, reviewer: self.reviewer,
                    at: Utc::now() }
    }
}

// 编译器保证：Draft 必须先 review 才能 publish
let draft = Draft::new("...".into());
// draft.publish();  // ❌ 编译错误：Draft 没有 publish
let reviewed = draft.review("Alice".into());
let published = reviewed.publish();  // ✓
```

## 3. 命名规范

> 完整命名规范见 [naming-conventions.md](naming-conventions.md)（基于 RFC 430 + API Guidelines）。本节只列与健壮性强相关的要点。

### 3.1 函数命名表达意图

```rust
// ❌ 含糊命名
fn process(data: &Data) -> Output { /* ... */ }
fn handle(x: &str) { /* ... */ }
fn check(item: &Item) -> bool { /* ... */ }

// ✓ 动词明确行为
fn validate_input(data: &Data) -> Output { /* ... */ }
fn send_notification(message: &str) { /* ... */ }
fn is_valid_email(item: &Item) -> bool { /* ... */ }

// 命名约定（详见 naming-conventions.md §2）：
// is_xxx / has_xxx / can_xxx → 返回 bool
// as_xxx                    → 无损耗转换（&self → &T）
// to_xxx                    → 有损耗转换（可能分配）
// into_xxx                  → 消费 self 转换
// from_xxx                  → 构造（关联函数）
// with_xxx / set_xxx        → Builder 链式
```

### 3.2 命名与返回类型一致

```rust
// ❌ 误导性命名
fn get_user(&self) -> () { ... }              // get_ 但不返回值
fn is_valid(&self) -> Result<()> { ... }      // is_ 但返回 Result 而非 bool
fn to_string(&self) -> &str { ... }            // to_ 但返回引用（应为 as_）

// ✓ 命名与返回类型一致
fn delete_user(&self) -> () { ... }            // void 操作
fn is_valid(&self) -> bool { ... }             // 布尔
fn validate(&self) -> Result<(), Error> { ... } // Result
fn as_str(&self) -> &str { ... }               // 借用
```

### 3.3 布尔参数用枚举

```rust
// ❌ 调用处不知所云
file.write(data, true);   // true 是什么意思？
file.write(data, false);  // 同步还是异步？追加还是覆盖？

// ✓ 用枚举明确语义
enum WriteMode { Overwrite, Append }
file.write(data, WriteMode::Append);  // 一目了然

// ✓ 或拆成两个函数
file.overwrite(data);
file.append(data);
```

## 4. 函数设计

### 4.1 函数长度

```rust
// 原则：函数做一件事，名字能完全描述其行为
// 超过 30 行的函数考虑拆分
// 超过 50 行几乎一定要拆

// ❌ 一个函数做太多事
fn process_order(order: &Order) -> Result<Receipt> {
    // 验证
    if order.items.is_empty() { return Err(...); }
    for item in &order.items {
        if !inventory.check(item) { return Err(...); }
    }
    // 计算价格
    let mut total = 0;
    for item in &order.items { total += item.price; }
    let tax = total * 0.1;
    // 应用折扣
    if let Some(coupon) = &order.coupon {
        total -= coupon.discount;
    }
    // 保存
    db.save(order)?;
    // 发邮件
    email.send(order.user_email)?;
    Ok(Receipt { total, tax })
}

// ✓ 拆分为小函数
fn process_order(order: &Order) -> Result<Receipt> {
    validate_order(order)?;
    let (subtotal, tax) = calculate_price(order);
    let total = apply_discount(subtotal, order.coupon.as_ref());
    persist_order(order)?;
    send_confirmation(order)?;
    Ok(Receipt { total, tax })
}
```

### 4.2 参数数量

```rust
// ❌ 参数过多
fn create_user(name: String, email: String, age: u32,
               role: String, dept: String, salary: u64,
               start: Date, manager: Option<String>) -> User { /* ... */ }

// ✓ 参数对象
struct UserInput {
    name: String,
    email: String,
    age: u32,
    role: Role,
    dept: Department,
    salary: Salary,
    start: Date,
    manager: Option<String>,
}
fn create_user(input: UserInput) -> User { /* ... */ }
```

### 4.3 避免输出参数

```rust
// ❌ 用参数返回结果
fn compute_stats(data: &[f64], mean: &mut f64, stddev: &mut f64) { /* ... */ }
let mut m = 0.0; let mut s = 0.0;
compute_stats(&data, &mut m, &mut s);

// ✓ 返回元组或结构体
fn compute_stats(data: &[f64]) -> (f64, f64) { /* ... */ }
let (mean, stddev) = compute_stats(&data);

// ✓ 复杂返回用结构体
struct Stats { mean: f64, stddev: f64, median: f64 }
fn compute_stats(data: &[f64]) -> Stats { /* ... */ }
```

## 5. 注释规范

### 5.1 注释解释"为什么"，不解释"是什么"

```rust
// ❌ 复述代码（无价值）
// 增加计数器
counter += 1;

// ❌ 复述代码
// 返回用户列表
fn get_users() -> Vec<User> { /* ... */ }

// ✓ 解释意图/原因
// 用 5 次重试是因为下游服务有 0.1% 的随机失败率
// 参见 https://issues.example.com/TICKET-1234
const MAX_RETRIES: u32 = 5;

// ✓ 解释非显然的决策
// 这里用 i64 而非 u64，因为要表示负的错误码
type Result<T> = std::result::Result<T, i64>;

// ✓ 解释 workaround
// TODO: 移除此 hack，待 issue #42 修复
// 当前的 serde 版本不支持 tagged enum with flatten
fn serialize_with_workaround(/* ... */) { /* ... */ }
```

### 5.2 文档注释（doc comment）

```rust
/// 计算两个日期之间的工作日数（排除周末和节假日）。
///
/// # 参数
/// - `start`: 开始日期（包含）
/// - `end`: 结束日期（不包含）
/// - `holidays`: 节假日列表
///
/// # 返回
/// 工作日数量，若 `start > end` 返回 0。
///
/// # 示例
/// ```
/// use chrono::NaiveDate;
/// let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
/// let end = NaiveDate::from_ymd_opt(2024, 1, 8).unwrap();
/// assert_eq!(business_days(start, end, &[]), 5);  // 1-5 是工作日
/// ```
///
/// # Panics
/// 不会 panic。
pub fn business_days(
    start: NaiveDate,
    end: NaiveDate,
    holidays: &[NaiveDate],
) -> u32 { /* ... */ }
```

### 5.3 unsafe 注释（强制）

```rust
// 每个 unsafe 块必须有 SAFETY 注释
fn example(buf: &mut [u8]) {
    // SAFETY: idx 已验证 < buf.len()（由调用者保证）
    unsafe {
        std::ptr::write_bytes(buf.as_mut_ptr(), 0, buf.len());
    }
}
```

## 6. 模块组织与可读性

### 6.1 模块大小

```
// 原则：
// - 单文件不超过 500 行（超出考虑拆分）
// - 单模块功能聚焦
// - 公共 API 在 lib.rs 集中 re-export

// 文件组织示例
src/
├── lib.rs              # 公共 API 入口
├── error.rs            # 错误类型
├── model/              # 数据模型
│   ├── mod.rs          # re-export
│   ├── user.rs         # < 300 行
│   └── order.rs
└── service/            # 业务逻辑
    ├── mod.rs
    ├── auth.rs
    └── payment.rs
```

### 6.2 可见性最小化

```rust
// ❌ 所有内容 pub
pub struct User {
    pub name: String,
    pub email: String,
    pub password_hash: String,  // 不应该暴露！
}

// ✓ 内部字段私有，通过方法访问
pub struct User {
    name: String,
    email: String,
    password_hash: String,  // 模块外不可访问
}

impl User {
    pub fn name(&self) -> &str { &self.name }
    pub fn email(&self) -> &str { &self.email }
    // 不暴露 password_hash
    pub fn verify_password(&self, input: &str) -> bool {
        verify(self.password_hash.as_bytes(), input.as_bytes())
    }
}
```

## 7. 测试驱动健壮性

> 完整测试规范见 [testing-standards.md](testing-standards.md)。本节只列与健壮性强相关的要点。

### 7.1 测试放置与封装边界（核心原则）

**源码文件只放实现，测试代码迁移到独立测试文件。** 单元测试贴近被测模块（可访问私有项），集成测试才放根 `tests/`。

```rust
// src/parser/lexer.rs
pub fn tokenize(input: &str) -> Vec<Token> { /* 实现 */ }
fn is_keyword(s: &str) -> bool { /* 私有函数 */ }

#[cfg(test)]
#[path = "lexer_tests.rs"]  // 测试迁移到独立文件
mod tests;
```

```rust
// src/parser/lexer_tests.rs
use super::*;

#[test]
fn is_keyword_recognizes_fn() {
    assert!(is_keyword("fn"));  // ✓ 可测私有，无需改 pub
}
```

**不要为测试把私有 API 改成 `pub`/`pub(crate)`——那会破坏封装边界。** 详见 testing-standards.md §2。

### 7.2 测试金字塔

```
       ╱ E2E ╲          ← 5%：完整流程，慢，脆弱
      ╱ 集成  ╲         ← 25%：模块协作
     ╱ 单元测试 ╲       ← 70%：单个函数/类型，快，确定
```

### 7.3 测试命名表达意图

```rust
// ❌ 含糊命名
fn test_user() { /* ... */ }
fn test1() { /* ... */ }

// ✓ 命名表达：被测对象_场景_期望结果
#[test]
fn user_create_with_valid_email_succeeds() { /* ... */ }

#[test]
fn transfer_with_insufficient_balance_returns_error() { /* ... */ }
```

### 7.4 测试不变量而非实现细节

```rust
// ❌ 测试实现细节（重构就坏）
#[test]
fn test_sort_uses_quickselect() {
    let comparisons = count_comparisons(&mut data);
    assert!(comparisons < 100);  // 依赖具体算法
}

// ✓ 测试行为（重构不坏）
#[test]
fn test_sort_result_is_sorted() {
    let mut data = vec![3, 1, 4, 1, 5, 9, 2, 6];
    sort(&mut data);
    assert_eq!(data, vec![1, 1, 2, 3, 4, 5, 6, 9]);  // 测试结果
}
```

## 8. 性能与可读性的平衡

### 8.1 不要过早优化

```rust
// ❌ 第一次写就过度优化
fn sum(data: &[i32]) -> i32 {
    // 用 SIMD 内联函数手写优化
    unsafe {
        let mut sum = _mm256_setzero_si256();
        for chunk in data.chunks_exact(8) {
            let v = _mm256_loadu_si256(chunk.as_ptr() as *const _);
            sum = _mm256_add_epi32(sum, v);
        }
        // ...
    }
}

// ✓ 先写清晰的代码，profile 后再优化
fn sum(data: &[i32]) -> i32 {
    data.iter().sum()  // 简单清晰，编译器会自动向量化
}
```

### 8.2 优化时加注释

```rust
// ✓ 优化代码必须注释为何这样写
fn process(data: &[u8]) -> Vec<u8> {
    // 用 unsafe 的 ptr::copy 替代 slice::copy_within
    // 因为 benchmark 显示这里占总时间 30%
    // 安全性：src/dst 都在 data 范围内，无重叠
    unsafe {
        let mut result = Vec::with_capacity(data.len());
        std::ptr::copy_nonoverlapping(data.as_ptr(), result.as_mut_ptr(), data.len());
        result.set_len(data.len());
        result
    }
}
```

### 8.3 性能关键路径与普通路径分离

```rust
// 普通路径：可读性优先
pub fn parse_simple(input: &str) -> Result<Config> {
    // 清晰的逐步解析
}

// 性能路径：性能优先，但有充分注释
#[cfg(feature = "fast-parser")]
pub fn parse_fast(input: &str) -> Result<Config> {
    // 优化版本，benchmark 显示比 parse_fast 快 5x
    // 详见 benches/parse_bench.rs
}
```

## 9. 健壮性检查清单

### 错误处理
- [ ] 生产代码无 unwrap/expect（除非有 expect 说明原因）
- [ ] 错误类型用 thiserror 结构化定义
- [ ] 错误传播添加上下文（map_err / context）
- [ ] panic 只用于不变量违反，不做控制流

### 类型系统
- [ ] 用 Newtype 区分语义相同的原始类型
- [ ] 让无效状态在类型层面不可表示
- [ ] 复杂构造用 Builder 模式
- [ ] 关键流程用类型状态模式

### 可读性
- [ ] 函数名表达行为（动词 + 对象）
- [ ] 函数长度 < 30 行，参数 < 4 个
- [ ] 注释解释"为什么"，不解释"是什么"
- [ ] 公共 API 有文档注释（含示例、参数、返回、错误）

### 模块
- [ ] 单文件 < 500 行
- [ ] 字段私有，方法访问
- [ ] 公共 API 在 lib.rs 集中导出

### 测试
- [ ] 单元测试占 70%+，覆盖关键路径
- [ ] 测试命名表达意图（场景 + 期望）
- [ ] 测试行为而非实现细节
- [ ] 每个 bug 修复先写失败测试

### 性能
- [ ] 先写清晰代码，profile 后再优化
- [ ] 优化代码有注释说明原因和 benchmark
- [ ] 不在普通路径过早优化

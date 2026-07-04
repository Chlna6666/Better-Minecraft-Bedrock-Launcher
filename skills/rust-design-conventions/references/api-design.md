# API 设计与 SemVer

> 公共 API 设计是库作者的核心职责。好的 API 让错误用法在编译期不可能、让调用者无需读文档就能正确使用、让版本升级不会破坏依赖者。本规范覆盖 trait/类型边界设计、derive 策略、`Send`+`Sync` 决策、动态分发取舍，以及 SemVer 兼容性规则——这些是 Rust 生态约定（API Guidelines）的硬性部分。

## 1. 公共 API 的核心原则

- **难以用错（hard to misuse）**：类型系统让错误状态不可表示
- **最小惊讶**：与标准库和生态惯例一致
- **可演进**：未来扩展不破坏现有调用者
- **可发现**：方法名、类型名让用法显而易见

## 2. Trait 边界设计

### 2.1 Trait 定义原则

```rust
// ✓ 小而专的 trait（接口隔离）
trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
}
trait Write {
    fn write(&mut self, buf: &[u8]) -> Result<usize>;
}
trait Seek {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64>;
}

// ❌ 上帝 trait（违反接口隔离）
trait FileSystem {
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    fn list(&self, dir: &str) -> Result<Vec<String>>;
    fn delete(&self, path: &str) -> Result<()>;
    fn metadata(&self, path: &str) -> Result<Metadata>;
    // 实现者必须实现全部，即使用不到
}
```

### 2.2 关联类型 vs 泛型参数

```rust
// 关联类型：一个 impl 只有一种 Item（更精确）
trait Iterator {
    type Item;                    // 关联类型
    fn next(&mut self) -> Option<Self::Item>;
}
// impl Iterator for MyIter { type Item = u32; ... }  // 一个 impl 一种 Item

// 泛型参数：一个类型可多次 impl（灵活）
trait From<T> {
    fn from(t: T) -> Self;
}
// impl From<i32> for MyType / impl From<String> for MyType  // 多次 impl OK
```

**决策：**
- 如果"每个类型只有一种合理实现"→ 关联类型（如 `Iterator::Item`）
- 如果"需要多种转换"→ 泛型参数（如 `From<T>`）

### 2.3 Trait object 友好性

```rust
// ✗ dyn-unfriendly：泛型方法不能做成 trait object
trait Bad {
    fn process<T: Debug>(&self, x: T);  // 泛型方法 → trait 不能 dyn
}

// ✓ dyn-friendly：用关联类型替代泛型
trait Good {
    type Output: Debug;
    fn process(&self) -> Self::Output;  // 可 dyn（但要 object-safe）
}

// object-safe 的条件：
// - 无泛型方法
// - 无 Self where Self: Sized（除非是默认实现）
// - 所有方法签名满足：&self/&mut self/self 且参数/返回非泛型
```

### 2.4 默认方法

```rust
trait Repository<T> {
    fn find_by_id(&self, id: u64) -> Option<T>;
    fn find_all(&self) -> Vec<T> {
        // 默认实现，实现者可覆盖
        vec![]
    }
}
```

**原则：** 提供合理默认实现，减少实现者负担；但默认实现必须对所有实现都正确。

## 3. 类型设计

### 3.1 Newtype 模式

```rust
// 区分语义相同的原始类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub u64);

fn transfer(from: UserId, to: UserId, amount: u64) { ... }
// 调用方不可能把 OrderId 误传为 UserId

// 零开销
#[repr(transparent)]
pub struct UserId(pub u64);  // sizeof == sizeof(u64)
```

### 3.2 让无效状态不可表示

```rust
// ❌ Option<bool> 语义模糊（None 是什么？）
struct User { is_active: Option<bool> }

// ✓ 用枚举明确状态
enum AccountStatus {
    Pending,    // 待激活
    Active,     // 已激活
    Suspended,  // 暂停
    Deleted,    // 已删除
}

// ❌ 两个 bool 可能产生无效组合
struct Subscription {
    is_paid: bool,
    is_active: bool,
    // 无效：is_paid=false, is_active=true
}

// ✓ 枚举表示互斥状态
enum Subscription {
    Trial { expires_at: DateTime },
    Active { until: DateTime },
    Cancelled,
    Expired,
}
```

### 3.3 Builder 模式

```rust
pub struct Server {
    host: String,
    port: u16,
    timeout: Duration,
}

impl Server {
    pub fn builder() -> ServerBuilder {
        ServerBuilder::default()
    }
}

#[derive(Default)]
pub struct ServerBuilder {
    host: Option<String>,
    port: Option<u16>,
    timeout: Option<Duration>,
}

impl ServerBuilder {
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }
    pub fn build(self) -> Result<Server, BuildError> {
        Ok(Server {
            host: self.host.ok_or(BuildError::MissingHost)?,
            port: self.port.unwrap_or(80),
            timeout: self.timeout.unwrap_or_else(|| Duration::from_secs(30)),
        })
    }
}
```

### 3.4 类型状态模式

```rust
// 编译期保证调用流程
pub struct Unconfigured;
pub struct Configured { host: String, port: u16 }

pub struct Client<State> { state: State }

impl Client<Unconfigured> {
    pub fn new() -> Self { Client { state: Unconfigured } }
    pub fn configure(self, host: &str, port: u16) -> Client<Configured> {
        Client { state: Configured { host: host.into(), port } }
    }
}
impl Client<Configured> {
    pub fn connect(self) -> Connection { ... }
}

// Client::new().connect()           // ❌ 编译错误：Unconfigured 无 connect
// Client::new().configure(...).connect()  // ✓
```

## 4. Trait derive 策略（C-COMMON-TRAITS）

### 4.1 应该 derive 的 trait

公共类型应对以下 trait derive：

```rust
#[derive(
    Debug,       // 必备：调试、错误信息、日志
    Clone,       // 常用：让调用者能复制
    PartialEq,   // 常用：比较、测试断言
    Eq,          // 若 PartialEq 且所有字段 Eq
    Hash,        // 若可能用作 HashMap key
)]
pub struct User {
    id: u64,
    name: String,
}
```

### 4.2 derive 决策表

| Trait | 何时 derive | 注意 |
|-------|------------|------|
| `Debug` | 几乎所有 pub 类型 | 不要 derive 在含敏感数据的类型（密码） |
| `Clone` | 大多数 pub 类型 | 大类型（Vec 大数据）考虑不 derive |
| `Copy` | 小且 trivial 类型（≤24 字节） | 实现后无法自定义 Drop |
| `PartialEq`/`Eq` | 可比较的类型 | 浮点字段不能 Eq |
| `Hash` | 用作 HashMap key 的类型 | 所有字段必须 Hash |
| `Default` | 有合理默认值的类型 | 无合理默认则不 derive |
| `PartialOrd`/`Ord` | 需要排序的类型 | 不总是需要 |

### 4.3 不应 derive 的场景

```rust
// 含敏感数据：Debug 会泄露
pub struct Credentials {
    password: String,  // Debug 会打印密码！
}
// 应手写 Debug
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Credentials").finish_non_exhaustive()
    }
}

// Clone 成本高：大文件句柄、连接池
pub struct LargeBuffer([u8; 1_000_000]);  // 不 derive Clone（1MB 拷贝）

// Eq 语义不明确：浮点
pub struct PointF64 { x: f64, y: f64 }  // 不 derive Eq（f64 不 Eq）
```

## 5. `Send` + `Sync` 决策

### 5.1 自动推导

大多数类型自动是 `Send` + `Sync`（所有字段都是 Send/Sync）：

```rust
struct User { id: u64, name: String }  // 自动 Send + Sync
struct Data { rc: Rc<u32> }            // 不是 Sync（Rc 非 Sync）
```

### 5.2 公共类型的 Send/Sync 考量

```rust
// ✓ 显式标注并发能力（在文档中）
/// 线程安全的计数器。
///
/// 实现了 `Send` 和 `Sync`，可跨线程共享。
pub struct Counter {
    value: AtomicU64,  // 自动 Send + Sync
}

// ⚠️ 内部可变性需要手动保证 Sync
pub struct MyRwLock<T> {
    inner: UnsafeCell<T>,  // UnsafeCell 非 Sync
}
// SAFETY: 通过 lock/unlock 协议保证线程安全
unsafe impl<T: Send> Sync for MyRwLock<T> {}
```

### 5.3 设计时的并发友好性

```rust
// ❌ dyn-unfriendly + 非 Send
pub struct Cache {
    data: Rc<RefCell<HashMap<K, V>>>,  // Rc 非 Send
}
// tokio::spawn 无法用此 Cache

// ✓ 用 Arc + 锁或 DashMap
pub struct Cache {
    data: Arc<RwLock<HashMap<K, V>>>,  // Send + Sync
}
// 或
pub struct Cache {
    data: DashMap<K, V>,  // 无锁，Send + Sync
}
```

**原则：** 公共类型默认设计为 `Send` + `Sync`，除非有强理由（如 `Rc` 的单线程优化）。

## 6. `#[must_use]` 标注

```rust
// 返回值不能忽略的类型
#[must_use = "Result 必须处理错误"]
pub enum Result<T, E> { ... }

// 返回 Self 的 builder 方法
impl Builder {
    #[must_use]
    pub fn host(self, host: &str) -> Self { ... }  // 忽略返回值 = 配置丢失
}

// 普通函数返回值
#[must_use]
pub fn compute_hash(data: &[u8]) -> u64 { ... }
// 忽略返回值 = 计算白做
```

**规则：**
- `Result`/`Option` 自动 `#[must_use]`
- 返回 `Self` 的 builder 方法应加 `#[must_use]`
- 返回新值的纯函数应加 `#[must_use]`
- 用 `must_use_candidate` clippy lint 自动识别候选

## 7. 动态分发取舍（dyn vs 泛型）

### 7.1 决策矩阵

| 场景 | 选择 | 原因 |
|------|------|------|
| 编译期类型已知 | 泛型 | 零开销、可内联 |
| 类型集合有限且已知 | 枚举分发 | 无堆分配、缓存友好 |
| 类型集合开放（插件） | `dyn Trait` | 灵活性优先 |
| 存储异构集合 | `Box<dyn Trait>` | 必须 dyn |
| 热点循环调用 | 泛型/枚举 | 避免间接调用开销 |
| 跨模块边界传递 | `dyn` 可减少单态化 | 二进制体积权衡 |

### 7.2 枚举分发替代 dyn

```rust
// 类型有限时，枚举比 Box<dyn> 快
pub enum Shape {
    Circle(Circle),
    Rect(Rect),
    Triangle(Triangle),
}

impl Shape {
    pub fn area(&self) -> f64 {
        match self {
            Shape::Circle(c) => c.area(),
            Shape::Rect(r) => r.area(),
            Shape::Triangle(t) => t.area(),
        }
    }
}

// 比 Vec<Box<dyn Shape>> 快：无堆分配、无 vtable、缓存友好
let shapes: Vec<Shape> = vec![...];
let total: f64 = shapes.iter().map(|s| s.area()).sum();
```

## 8. SemVer 兼容性

### 8.1 SemVer 规则

```
MAJOR.MINOR.PATCH
  1.0.0

- MAJOR: 不兼容变更（breaking change）→ 1.0.0 → 2.0.0
- MINOR: 向后兼容的新功能 → 1.0.0 → 1.1.0
- PATCH: 向后兼容的 bug 修复 → 1.0.0 → 1.0.1

0.x.y 阶段：任何变更都可能 breaking（0.x 不保证稳定）
1.0.0+：MINOR/PATCH 必须向后兼容
```

### 8.2 Breaking change 清单（MAJOR 才能做）

**绝对 breaking：**
- 删除/重命名 pub 项（函数、类型、方法、字段）
- 改变函数签名（参数类型、参数顺序、返回类型）
- 给结构体加字段（除非实现 `#[non_exhaustive]`）
- 给枚举加变体（除非 `#[non_exhaustive]`）
- 实现新 trait 可能冲突
- 改 trait 方法签名
- 提高 MSRV

**非 breaking（MINOR/PATCH 可做）：**
- 加新的 pub 项
- 加新的 trait 实现
- 加默认方法到 trait
- 改进内部实现（不改 API）
- 修复 bug

### 8.3 `#[non_exhaustive]` 预留扩展空间

```rust
// 结构体：未来可加字段而不 breaking
#[non_exhaustive]
pub struct Config {
    pub host: String,
    pub port: u16,
}
// 调用者不能这样构造（必须用 ..Default::default() 或 builder）：
// Config { host: "x".into(), port: 80 }  // ❌ 编译错误
// 未来加 timeout 字段不会破坏调用者

// 枚举：未来可加变体
#[non_exhaustive]
pub enum Error {
    Io(std::io::Error),
    Parse(String),
}
// 调用者 match 必须有 _ 通配：
// match error {
//     Error::Io(_) => ...,
//     Error::Parse(_) => ...,
//     _ => ...,  // 必须有，否则未来加变体编译失败
// }
```

**原则：** 公共结构体和枚举默认加 `#[non_exhaustive]`，预留扩展空间。

### 8.4 trait 演进的兼容性

```rust
// 加默认方法：兼容（MINOR）
trait MyTrait {
    fn existing(&self);
    fn new_method(&self) {  // 有默认实现 → 不破坏现有实现者
        // ...
    }
}

// 加必须实现的方法：breaking（MAJOR）
trait MyTrait {
    fn existing(&self);
    fn new_required(&self);  // ❌ 所有实现者必须加 → breaking
}
```

### 8.5 cargo-semver-checks

```bash
# 自动检测 breaking change
cargo install cargo-semver-checks
cargo semver-checks  # 对比 git 历史，报告 breaking change

# 在 CI 中
cargo semver-checks check-release
```

## 9. API 演进策略

### 9.1 弃用而非删除

```rust
#[deprecated(
    since = "1.5.0",
    note = "改用 `new_with_config`，`new` 将在 2.0 移除"
)]
pub fn new() -> Self {
    Self::new_with_config(Config::default())
}

pub fn new_with_config(config: Config) -> Self { ... }
```

### 9.2 版本迁移路径

```
1.0:   旧 API
1.5:   新 API + 旧 API 标 deprecated
2.0:   删除旧 API
```

### 9.3 trait 扩展的安全方式

```rust
// 用 extension trait 模式，不破坏现有 trait
trait MyTraitExt: MyTrait {
    fn new_feature(&self) { ... }
}
impl<T: MyTrait> MyTraitExt for T {}
```

## 10. API 设计检查清单

### Trait
- [ ] trait 小而专（接口隔离），非上帝 trait
- [ ] 用关联类型而非泛型参数（当一对一关系时）
- [ ] 提供合理默认方法
- [ ] 评估 object-safety（如需 dyn）

### 类型
- [ ] Newtype 区分语义相同的原始类型
- [ ] 让无效状态在类型层不可表示
- [ ] 复杂构造用 Builder
- [ ] 关键流程用类型状态模式

### derive
- [ ] 公共类型 derive `Debug`/`Clone`
- [ ] 用作 key 的类型 derive `Hash`/`Eq`
- [ ] 有合理默认值的 derive `Default`
- [ ] 敏感数据手写 `Debug`（不泄露）
- [ ] 小 trivial 类型才 derive `Copy`

### 并发
- [ ] 公共类型默认 `Send` + `Sync`
- [ ] 内部可变性手动 impl Sync 时有 SAFETY 注释
- [ ] 用 Arc 替代 Rc（如需跨线程）

### API 完整性
- [ ] 返回 `Result`/`Option`/`Self` 的项加 `#[must_use]`
- [ ] 公共结构体/枚举加 `#[non_exhaustive]`
- [ ] 弃用项用 `#[deprecated]` + note 说明替代

### SemVer
- [ ] 0.x 阶段允许 breaking，1.0+ 严格 SemVer
- [ ] 加字段/变体前确保 `#[non_exhaustive]`
- [ ] CI 跑 `cargo semver-checks` 防 breaking
- [ ] 弃用流程：deprecated → 下个 MAJOR 删除

### 分发取舍
- [ ] 编译期已知类型用泛型
- [ ] 类型有限用枚举分发
- [ ] 热点循环避免 dyn
- [ ] 异构集合用 Box<dyn>，评估 Arc<dyn> 减少单态化

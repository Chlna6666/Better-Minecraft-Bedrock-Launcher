# Rust 性能优化深度指南

## 核心原则

> **不要猜测，先测量。** 永远用数据驱动优化决策。

优化优先级：算法 > 数据结构 > 内存布局 > 微优化。

---

## 1. 避免不必要的内存复制

### 1.1 所有权与借用（默认行为）

编写 Rust 代码时，**默认使用引用而非所有权**，仅在确实需要时才 clone。

```rust
// ✗ 差：不必要地获取所有权
fn process(name: String) -> String {
    name.to_uppercase()  // String 本身是 owned，可以 to_uppercase
}

// ✓ 好：借用优先
fn process(name: &str) -> String {
    name.to_uppercase()
}

// ✓ 好：必要时使用 Cow 延迟决定是否克隆
use std::borrow::Cow;
fn process(name: Cow<'_, str>) -> String {
    name.into_owned().to_uppercase()
}
```

### 1.2 参数传递规则

| 场景 | 推荐方式 |
|------|----------|
| 只读访问 | `&T` 或 `&str` / `&[T]` |
| 需要修改但不获取所有权 | `&mut T` |
| 小类型（≤24 bytes，impl Copy） | 按值传递 `T` |
| 大类型需要所有权 | `T`，由调用者决定是否 clone |
| 可能需要也可能不需要 owned | `Cow<'_, T>` |
| 跨线程共享只读数据 | `Arc<T>` |
| 构建并返回集合 | 直接返回 `Vec<T>` / `String`（RVO 优化） |

### 1.3 常见 clone 陷阱

```rust
// ✗ 差：循环中不必要的 clone
for item in items.iter() {
    list.push(item.clone());  // 如果 item 实现了 Copy，用 .copied()
}
// ✓ 好：使用迭代器适配器
let list: Vec<_> = items.iter().copied().collect();
// 或更好：如果可能，直接 into_iter 消费所有权
let list: Vec<_> = items.into_iter().collect();

// ✗ 差：函数内 clone 参数（应由调用者传入 owned）
fn take_borrow(thing: &Thing) {
    let owned = thing.clone();  // 调用者应该直接传入 owned
}

// ✓ 好：函数签名明确所有权需求
fn take_owned(thing: Thing) { ... }

// ✗ 差：返回前克隆整个数据结构
fn get_all_users(&self) -> Vec<User> {
    self.users.clone()  // 如果只需迭代，返回迭代器或切片
}

// ✓ 好：返回引用或迭代器
fn iter_users(&self) -> impl Iterator<Item = &User> + '_ {
    self.users.iter()
}
```

### 1.4 结构体字段复制的控制

```rust
// ✗ 差：整个结构体只有一个字段需要 clone
#[derive(Clone)]
struct UserSession {
    user_id: u64,       // Copy，不需要 clone
    username: String,    // 需要拷贝
    permissions: Vec<Permission>,  // 可能很大，不一定要 clone
}

// ✓ 好：只 clone 需要的部分
fn get_username(&self) -> String {
    self.username.clone()  // 只 clone String
}

// ✓ 更好：返回引用避免 clone
fn username(&self) -> &str {
    &self.username
}
```

### 1.5 String 与 &str 的选择

```rust
// 函数签名中：&str 接受 &String 和 &str
fn greet(name: &str) { println!("Hello, {name}"); }

// 结构体字段中：如果生命周期可绑定，用 &str
struct Request<'a> {
    method: &'a str,
    path: &'a str,
    headers: Vec<(&'a str, &'a str)>,
}

// 如果必须 owned（如存入集合、跨线程），用 String
struct StoredUser {
    name: String,  // 需要生命周期独立于原始数据
}
```

### 1.6 Vec 与切片的选择

```rust
// ✗ 差：函数参数接受 Vec
fn process(data: Vec<i32>) { ... }

// ✓ 好：函数参数接受切片
fn process(data: &[i32]) { ... }
// 这样调用者可以传 &vec, &[T], &array，无需 clone

// ✗ 差：中间分配
let filtered: Vec<_> = data.iter().filter(|x| **x > 0).collect();
process(&filtered);

// ✓ 好：传递迭代器
fn process_iter(data: impl Iterator<Item = i32>) { ... }
process_iter(data.iter().filter(|x| **x > 0));
```

---

## 2. 减少重复判断与冗余计算

### 2.1 提前返回，减少嵌套

```rust
// ✗ 差：深层嵌套，每次都要判断到最内层
fn validate(data: &Data) -> Result<Output> {
    if data.is_present() {
        if data.is_valid() {
            if data.has_permission() {
                return Ok(process(data));
            } else {
                return Err(Error::NoPermission);
            }
        } else {
            return Err(Error::Invalid);
        }
    } else {
        return Err(Error::Missing);
    }
}

// ✓ 好：卫语句（guard clause），提前返回，减少嵌套层级
fn validate(data: &Data) -> Result<Output> {
    if !data.is_present() { return Err(Error::Missing); }
    if !data.is_valid() { return Err(Error::Invalid); }
    if !data.has_permission() { return Err(Error::NoPermission); }

    Ok(process(data))
}
```

### 2.2 缓存重复计算结果

```rust
// ✗ 差：循环中重复计算
for item in &items {
    let config = load_config();  // 每次迭代都加载
    process(item, &config);
}

// ✓ 好：提到循环外
let config = load_config();
for item in &items {
    process(item, &config);
}

// ✓ 更好：如果 config 加载成本高，使用 Lazy
use std::sync::LazyLock;
static CONFIG: LazyLock<Config> = LazyLock::new(|| load_config());
```

### 2.3 使用 match 替代重复 if-else

```rust
// ✗ 差：重复的 if-else 链，每个分支都重新判断
fn classify(value: i32) -> &'static str {
    if value == 0 { "zero" }
    else if value > 0 && value < 10 { "small positive" }
    else if value >= 10 && value < 100 { "medium positive" }
    else if value >= 100 { "large positive" }
    else if value < 0 && value > -10 { "small negative" }
    else if value <= -10 { "large negative" }
    else { unreachable!() }
}

// ✓ 好：match 一次判断，清晰且编译器可能优化为跳转表
fn classify(value: i32) -> &'static str {
    match value {
        0 => "zero",
        1..=9 => "small positive",
        10..=99 => "medium positive",
        100.. => "large positive",
        -9..=-1 => "small negative",
        ..=-10 => "large negative",
    }
}
```

### 2.4 使用 HashSet 替代线性查找

```rust
// ✗ 差：O(n) 查找，且每次调用都重新判断
fn is_admin(user: &User) -> bool {
    let admins = ["alice", "bob", "charlie"];
    admins.contains(&user.name.as_str())  // 每次 O(n) 线性搜索
}

// ✓ 好：O(1) 查找
use std::collections::HashSet;
fn is_admin(user: &User) -> bool {
    static ADMINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
        ["alice", "bob", "charlie"].into_iter().collect()
    });
    ADMINS.contains(user.name.as_str())
}
```

### 2.5 使用 Lookup Table 替代分支

```rust
// ✗ 差：大量 match 分支
fn char_to_digit(c: char) -> Option<u8> {
    match c {
        '0' => Some(0), '1' => Some(1), '2' => Some(2),
        '3' => Some(3), '4' => Some(4), '5' => Some(5),
        '6' => Some(6), '7' => Some(7), '8' => Some(8),
        '9' => Some(9),
        _ => None,
    }
}

// ✓ 好：利用已有的方法
fn char_to_digit(c: char) -> Option<u8> {
    c.to_digit(10).map(|d| d as u8)
}
// 编译器会将连续 match 优化为跳转表，但使用标准库方法更简洁
```

### 2.6 使用 bool 短路避免多余计算

```rust
// ✗ 差：两个条件都计算
if expensive_check_a() || expensive_check_b() {
    // 两个都执行了，即使 a 已经是 true
}

// ✓ 好：短路求值（Rust 默认行为）
// && 和 || 都是短路运算符，左侧为 true/false 后右侧不执行
if cheap_check() && expensive_check() { ... }
```

---

## 3. 内存布局优化

### 3.1 结构体字段重排

Rust 默认不做字段重排（出于引用稳定性）。手动重排减少 padding 浪费。

```rust
// ✗ 差：大量 padding 浪费（对齐到 8 字节）
// 内存布局：| u8(1) | pad(7) | String(24) | pad(4) | u64(8) | u32(4) | pad(4) |
// 总计 52 字节，有效数据 45 字节，浪费 7 字节
struct BadLayout {
    flag: u8,
    name: String,
    id: u64,
    count: u32,
}

// ✓ 好：按对齐大小降序排列
// 内存布局：| String(24) | u64(8) | u32(4) | u8(1) | pad(3) |
// 总计 40 字节，节省 12 字节
struct GoodLayout {
    name: String,    // 24 bytes (usize + usize + usize)
    id: u64,          // 8 bytes
    count: u32,       // 4 bytes
    flag: u8,         // 1 byte
}

// 使用 #[repr(C)] 可以确保 C 兼容布局（FFI 需要）
#[repr(C)]
struct FfiLayout {
    name: String,
    id: u64,
    count: u32,
    flag: u8,
}
```

**规则：** 结构体字段按对齐要求从大到小排列，可以最小化 padding。

### 3.2 使用 #[repr(transparent)] 消除 wrapper 开销

```rust
// ✗ 差：Newtype wrapper 有额外的对齐 padding
#[derive(Clone)]
struct UserId(String);

// ✓ 好：transparent repr，与内部类型内存布局完全一致
#[repr(transparent)]
#[derive(Clone)]
struct UserId(String);
// sizeof(UserId) == sizeof(String)，零开销抽象
```

### 3.3 使用枚举的 Tagged Pointer 优化

```rust
// ✗ 差：Option<Box<T>> 占用 16 字节（discriminant + pointer）
let x: Option<Box<User>> = None;  // 16 bytes

// ✓ 好：Rust 对 Option<Box<T>> 有 niched pointer 优化
// None 表示为 null pointer，占用也是 8 字节（与 Box<T> 相同）
// 以下类型自动享受 niched optimization：
// - Option<Box<T>>
// - Option<&T>
// - Option<&mut T>
// - Option<NonNull<T>>
// - Option<fn(...) -> ...>

// ✗ 差：Option<Vec<T>> 没有自动优化（Vec 有内部 capacity 不为零的保证）
// 需要 24 + 8 = 32 bytes
// ✓ 替代方案：自己定义 niched 类型
```

### 3.4 使用小集合替代大集合

```rust
// 当集合通常很小（1-4 个元素）时，使用小向量避免堆分配
use smallvec::SmallVec;

// ✗ 差：即使只有 1 个元素也在堆上分配
let tags: Vec<String> = vec![tag];

// ✓ 好：栈上存储小量，溢出到堆
let tags: SmallVec<[String; 4]> = SmallVec::new();  // ≤4 个元素在栈上
tags.push("rust".into());

// 类似的还有：
// - tinyvec::ArrayVec  // 固定大小，不溢出
// - arrayvec::ArrayVec  // 固定大小，不溢出
// - smallvec::SmallVec  // 小量栈上，溢出堆
// - heapless::Vec       // 嵌入式 no_std，固定大小
```

### 3.5 字符串内联

```rust
use smartstring::SmartString;

// ✗ 差：短字符串也在堆上分配
let name: String = "Bob".to_string();  // 堆分配！

// ✓ 好：短字符串内联存储（≤22 bytes 在栈上）
let name: SmartString<smartstring::LazyCompact> = SmartString::from("Bob");
// 短字符串零堆分配，长字符串自动退化为 String
```

---

## 4. 迭代器与零开销抽象

### 4.1 避免中间集合分配

```rust
// ✗ 差：每个 .collect() 都创建中间 Vec
let step1: Vec<_> = data.iter().map(|x| x * 2).collect();
let step2: Vec<_> = step1.iter().filter(|x| **x > 10).collect();
let result: Vec<_> = step2.iter().map(|x| x + 1).collect();

// ✓ 好：链式迭代器，零中间分配
let result: Vec<_> = data.iter()
    .map(|x| x * 2)
    .filter(|x| *x > 10)
    .map(|x| x + 1)
    .collect();
// 只在最终的 .collect() 时分配一次
```

### 4.2 避免不必要的 collect

```rust
// ✗ 差：collect 后只做一次操作
let sum: i64 = items.iter().map(|x| x.value).collect::<Vec<_>>().iter().sum();

// ✓ 好：直接在迭代器上 sum
let sum: i64 = items.iter().map(|x| x.value).sum();

// ✗ 差：collect 后只检查是否为空
let filtered: Vec<_> = items.iter().filter(|x| x.is_active()).collect();
if !filtered.is_empty() { ... }

// ✓ 好：any() 短路求值
if items.iter().any(|x| x.is_active()) { ... }
```

### 4.3 预分配集合容量

```rust
// ✗ 差：Vec 动态增长导致多次重新分配
let mut result = Vec::new();
for item in &items {
    if item.is_valid() {
        result.push(process(item));  // 可能多次 realloc
    }
}

// ✓ 好：预知大小时预分配
let mut result = Vec::with_capacity(items.len());
for item in &items {
    if item.is_valid() {
        result.push(process(item));
    }
}

// ✓ 更好：使用 estimate_size 或精确计算
let estimated = items.iter().filter(|i| i.is_valid()).count(); // 注意：这次遍历了
let mut result = Vec::with_capacity(estimated);
// 如果精确计算需要遍历，直接在收集时分配：
let result: Vec<_> = items.iter()
    .filter_map(|item| if item.is_valid() { Some(process(item)) } else { None })
    .collect();
```

### 4.4 使用 extend / splice 批量操作

```rust
// ✗ 差：逐个 push
for item in other_list {
    result.push(item);
}

// ✓ 好：批量 extend（可能一次性 memcopy）
result.extend(other_list);
```

---

## 5. 并发与并行优化

### 5.1 选择合适的并发原语

```rust
// 读取远多于写入的场景
// ✗ 差：Mutex，即使是读也需要互斥锁
let data = Arc::new(Mutex::new(big_data));

// ✓ 好：RwLock，允许多个读者同时访问
let data = Arc::new(RwLock::new(big_data));

// ✓ 更好：如果数据初始化后不变，用 OnceLock
let data: Arc<OnceLock<BigData>> = Arc::new(OnceLock::new());
data.get_or_init(|| load_big_data());
// 读取完全无锁！
```

### 5.2 避免锁竞争

```rust
// ✗ 差：细粒度锁导致频繁锁获取
fn update(item: &Item, db: &Mutex<HashMap<Id, Item>>) {
    let mut db = db.lock().unwrap();
    db.insert(item.id(), item.clone());
}

// ✓ 好：分片减少竞争（sharding）
use dashmap::DashMap;  // 无锁并发 HashMap
let db: DashMap<Id, Item> = DashMap::new();
db.insert(item.id(), item.clone());

// ✓ 或者使用 crossbeam 的无锁队列
use crossbeam::queue::ArrayQueue;
let queue = ArrayQueue::new(1024);
```

### 5.3 使用 rayon 进行数据并行

```rust
use rayon::prelude::*;

// ✗ 差：单线程顺序处理
let results: Vec<_> = items.iter().map(|x| expensive_compute(x)).collect();

// ✓ 好：rayon 自动并行
let results: Vec<_> = items.par_iter().map(|x| expensive_compute(x)).collect();

// 注意：任务粒度太小（<1μs）时并行开销超过收益
// 使用 par_iter().with_min_len(1024) 控制最小批次
```

### 5.4 异步 I/O 优化

```rust
// ✗ 差：阻塞 I/O 阻塞整个 tokio 运行时
let file = std::fs::read_to_string(path)?;  // 阻塞！

// ✓ 好：使用 tokio 异步 I/O
use tokio::fs;
let file = fs::read_to_string(path).await?;

// ✓ 运行 CPU 密集任务时使用 spawn_blocking
let result = tokio::task::spawn_blocking(|| cpu_intensive_work()).await?;

// ✗ 差：在 async 函数中持有跨 await 点的锁
async fn bad() {
    let guard = mutex.lock().await;
    some_async_fn().await;  // 锁持有时间过长！
    drop(guard);
}

// ✓ 好：缩小锁的持有范围
async fn good() {
    let data = {
        let guard = mutex.lock().await;
        guard.data.clone()  // 只 clone 需要的数据
    }; // guard 在此处 drop
    some_async_fn(data).await;
}
```

---

## 6. 字符串处理优化

### 6.1 避免重复格式化

```rust
// ✗ 差：多次格式化
for user in &users {
    println!("User: {} ({})", user.name, user.email);
    log::info!("Processing user: {}", user.name);
}

// ✓ 好：只在需要时格式化
for user in &users {
    let display = format!("{} ({})", user.name, user.email);
    println!("User: {display}");
    log::info!("Processing user: {}", user.name);  // 单字段直接用
}
```

### 6.2 使用 String::with_capacity

```rust
// ✗ 差：字符串增长导致多次重新分配
let mut result = String::new();
for chunk in chunks {
    result.push_str(chunk);  // 可能多次 realloc
}

// ✓ 好：预估容量
let total_len: usize = chunks.iter().map(|c| c.len()).sum();
let mut result = String::with_capacity(total_len);
for chunk in chunks {
    result.push_str(chunk);
}

// ✓ 更好：使用 join（内部已优化）
let result = chunks.join("");
```

### 6.3 使用 bytes 处理替代字符串处理

```rust
// ✗ 差：字符串操作需要处理 UTF-8 边界
fn count_a(s: &str) -> usize {
    s.chars().filter(|&c| c == 'a').count()  // 处理 Unicode
}

// ✓ 好：如果确定是 ASCII，直接操作字节（快 5-10x）
fn count_a(s: &str) -> usize {
    s.as_bytes().iter().filter(|&&b| b == b'a').count()
}

// ✓ 更好：使用 memchr crate（SIMD 加速）
fn count_a(s: &str) -> usize {
    memchr::memchr_iter(b'a', s.as_bytes()).count()
}
```

---

## 7. 编译期优化

### 7.1 使用 const 和泛型将计算推到编译期

```rust
// ✗ 差：运行时计算
const TABLE_SIZE: usize = 1024;
let size = TABLE_SIZE * std::mem::size_of::<Entry>();

// ✓ 好：const fn 在编译期计算
const TABLE_SIZE: usize = 1024;
const ENTRY_SIZE: usize = std::mem::size_of::<Entry>();
const TOTAL_SIZE: usize = TABLE_SIZE * ENTRY_SIZE;  // 编译期常量

// ✓ 好：泛型单态化消除分支
trait Strategy {
    fn execute(&self, data: &mut [u8]);
}
fn process<S: Strategy>(data: &mut [u8], strategy: &S) {
    strategy.execute(data);  // 编译后直接内联，无虚函数调用
}
```

### 7.2 使用 #[inline] 的正确时机

```rust
// 不要随意使用 #[inline]，Rust 编译器已经够智能
// 仅在以下场景考虑：

// 1. 极小的函数（1-3 行），确定内联有益
#[inline(always)]
fn clamp(v: f32, min: f32, max: f32) -> f32 {
    v.max(min).min(max)
}

// 2. 跨 crate 边界的小函数（LLVM 看不到内部实现时）
#[inline]
pub fn utility_fn(x: u32) -> u32 { ... }

// 3. 特定的性能关键路径，且有 benchmark 验证
```

### 7.3 Profile 配置优化

```toml
# Cargo.toml
[profile.release]
opt-level = 3           # 最高优化级别
lto = "fat"             # 全程序链接时优化（更慢编译，更快运行）
codegen-units = 1       # 单 codegen unit（更好的优化，更慢编译）
panic = "abort"         # abort 比 unwind 更小更快（但不支持 catch_unwind）
strip = true            # 去除符号表
overflow-checks = false # 生产环境关闭溢出检查（需权衡安全）

# 对于 CPU 密型计算
[profile.release]
opt-level = 3
lto = "fat"

# 对于需要调试的生产构建
[profile.release-debug]
inherits = "release"
debug = true
strip = false
```

### 7.4 Cargo feature 精细化控制

```toml
[features]
default = ["default-backend"]
default-backend = ["rusqlite"]
pg-backend = ["postgres"]
benchmark = []  # 仅用于 benchmark 的 feature

# 避免在 feature 之间引入不必要的依赖
```

---

## 8. I/O 与网络优化

### 8.1 缓冲 I/O

```rust
// ✗ 差：每次写入都系统调用
use std::io::Write;
file.write_all(b"line1\n")?;
file.write_all(b"line2\n")?;
file.write_all(b"line3\n")?;

// ✓ 好：使用 BufWriter 批量刷新
use std::io::{BufWriter, Write};
let mut writer = BufWriter::with_capacity(8192, file);
writer.write_all(b"line1\n")?;
writer.write_all(b"line2\n")?;
writer.write_all(b"line3\n")?;
writer.flush()?;  // 一次性写出
```

### 8.2 零拷贝传输

```rust
// ✗ 差：读入内存再写出，两次拷贝
let data = std::fs::read("input.txt")?;
std::fs::write("output.txt", &data)?;

// ✓ 好：使用 copy 在内核空间传输（零拷贝）
use std::io;
let mut reader = io::BufReader::new(File::open("input.txt")?);
let mut writer = io::BufWriter::new(File::create("output.txt")?);
io::copy(&mut reader, &mut writer)?;  // 使用 copy_file_range 系统调用

// ✓ 网络 sendfile
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
let mut file = File::open("big_file.bin").await?;
let mut out = tokio::net::TcpStream::connect(addr).await?;
tokio::io::copy(&mut file, &mut out).await?;
```

### 8.3 批量数据库操作

```rust
// ✗ 差：逐条插入
for user in &users {
    db.execute("INSERT INTO users (name) VALUES ($1)", &[&user.name])?;
}

// ✓ 好：批量插入
let batch = users.iter().map(|u| (u.name.as_str())).collect::<Vec<_>>();
db.execute_batch(&users.iter().map(|u| {
    format!("INSERT INTO users (name) VALUES ('{}')", u.name)
}).collect::<String>())?;

// ✓ 更好：使用事务 + prepared statement
let tx = db.transaction()?;
let mut stmt = tx.prepare("INSERT INTO users (name) VALUES ($1)")?;
for user in &users {
    stmt.execute([&user.name])?;
}
tx.commit()?;
```

---

## 9. 性能测量工具

### 9.1 基准测试

```rust
// benches/my_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_process(c: &mut Criterion) {
    let data = vec![1u64; 10_000];
    c.bench_function("process_data", |b| {
        b.iter(|| process_data(&data))
    });
}

criterion_group!(benches, bench_process);
criterion_main!(benches);
```

```bash
cargo bench                    # 运行基准测试
cargo flamegraph --bench=my_bench  # 生成火焰图
```

### 9.2 Clippy 性能 lint

```bash
# 必须运行的性能检查
cargo clippy --all-targets --all-features --locked -- -D warnings -W clippy::perf

# 关注的 lint：
# clippy::redundant_clone        — 不必要的克隆
# clippy::needless_collect       — 不必要的中间集合
# clippy::iter_over_hash         — 随机遍历 HashMap
# clippy::single_match_else      — 冗余的 match 分支
# clippy::large_enum_variant     — 大枚举变体应 Box
```

---

## 10. 优化检查清单

在编写或审查 Rust 代码时，自动检查以下项目：

### 内存与分配
- [ ] 函数参数是否使用了 `&T` / `&str` / `&[T]` 而非 owned 类型？
- [ ] 是否有不必要的 `.clone()` 调用？
- [ ] 循环内是否有重复的堆分配？
- [ ] 集合是否预分配了容量（`Vec::with_capacity`、`String::with_capacity`）？
- [ ] 是否有不必要的中间 `.collect()`？
- [ ] 结构体字段是否按对齐大小排列以减少 padding？

### 计算与逻辑
- [ ] 是否有重复计算可以提到循环外或缓存？
- [ ] 是否有嵌套的 if-else 可以用 guard clause 简化？
- [ ] 线性查找是否可以用 HashSet/HashMap 替代？
- [ ] 是否使用了短路求值（&&/||）？

### 并发
- [ ] 是否使用了 RwLock 替代 Mutex（读多写少场景）？
- [ ] 是否使用了 OnceLock/LazyLock 替代 Mutex（初始化后不变的场景）？
- [ ] 异步代码中锁的持有时间是否最短？
- [ ] CPU 密集任务是否使用了 spawn_blocking？

### 编译期
- [ ] 可在编译期计算的值是否使用了 const？
- [ ] 跨 crate 边界的小函数是否有 #[inline]？
- [ ] release profile 是否配置了 LTO 和 codegen-units=1？

### I/O
- [ ] 文件写入是否使用了 BufWriter？
- [ ] 大文件传输是否使用了 io::copy（零拷贝）？
- [ ] 数据库操作是否使用了批量/事务？

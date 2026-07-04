# Rust 性能陷阱与反模式

> Rust 给你零成本抽象的承诺，但"零成本"是有前提的。以下陷阱会让你的 Rust 代码比等价的 C 还慢。本章是反向参考——告诉你在热点路径上**不要**这样写。

## 1. 不必要的内存分配

### 1.1 隐藏的 clone

```rust
// ❌ 在循环中 clone
for user in &users {
    db.save(user.clone());  // 每次 clone 整个 User
}

// ✓ 让 save 接受引用
fn save(&self, user: &User) { /* ... */ }
for user in &users {
    db.save(user);
}

// ✓ 或消费所有权（如果之后不再需要）
for user in users {  // into_iter
    db.save(user);
}
```

### 1.2 字符串拼接分配

```rust
// ❌ 每次拼接都分配
let mut url = String::new();
url += "https://";
url += host;
url += "/api/v1/users/";
url += &id.to_string();  // 额外的 String 分配！

// ✓ 用 format!（一次分配）
let url = format!("https://{host}/api/v1/users/{id}");

// ✓ 高频场景用 write! 到复用缓冲
use std::fmt::Write;
let mut buf = String::with_capacity(64);
write!(buf, "https://{host}/api/v1/users/{id}").unwrap();
let url = &buf;
// buf 可复用，避免重复分配
```

### 1.3 to_string() / to_owned() 在热路径

```rust
// ❌ 频繁的 to_string
for key in &keys {
    map.get(&key.to_string());  // 每次分配 String
}

// ✓ 用 &str 查询
for key in keys {
    map.get(key.as_str());  // 零分配
}
```

### 1.4 collect 的滥用

```rust
// ❌ 中间 collect 多次分配
let v1: Vec<_> = data.iter().map(f1).collect();
let v2: Vec<_> = v1.iter().filter(p).collect();
let v3: Vec<_> = v2.iter().map(f2).collect();

// ✓ 链式迭代器，一次 collect
let v3: Vec<_> = data.iter().map(f1).filter(p).map(f2).collect();

// ❌ 只为判断非空就 collect
let found: Vec<_> = items.iter().filter(|x| x.matches()).collect();
if !found.is_empty() { ... }

// ✓ 用 any 短路
if items.iter().any(|x| x.matches()) { ... }
```

## 2. 低效的集合操作

### 2.1 线性查找替代哈希查找

```rust
// ❌ Vec 线性查找（O(n)）
let admins = vec!["alice", "bob", "charlie"];
if admins.contains(&name) { ... }

// ✓ HashSet（O(1)）
use std::collections::HashSet;
let admins: HashSet<&str> = ["alice", "bob", "charlie"].into_iter().collect();
if admins.contains(&name) { ... }
```

### 2.2 频繁的 Vec 增长

```rust
// ❌ 未预分配，多次 realloc
let mut result = Vec::new();
for i in 0..10_000 {
    result.push(i * 2);
}
// realloc 在 capacity 1,2,4,8,...,16384 时触发，约 14 次拷贝

// ✓ 预分配
let mut result = Vec::with_capacity(10_000);
for i in 0..10_000 { result.push(i * 2); }

// ✓ collect 自带 size_hint 预分配
let result: Vec<_> = (0..10_000).map(|i| i * 2).collect();
```

### 2.3 HashMap 迭代顺序随机

```rust
// ❌ 依赖 HashMap 顺序（每次运行可能不同）
let map = HashMap::from([("a", 1), ("b", 2), ("c", 3)]);
for (k, v) in &map {  // 顺序未定义！
    println!("{k}={v}");
}

// ✓ 需要顺序用 BTreeMap（按 key 排序）
use std::collections::BTreeMap;
let map = BTreeMap::from([("a", 1), ("b", 2), ("c", 3)]);
for (k, v) in &map {  // 一定是 a, b, c 顺序
    println!("{k}={v}");
}

// ✓ 需要插入顺序用 indexmap
use indexmap::IndexMap;
```

### 2.4 不必要的保留容量

```rust
// ❌ 内存浪费
let mut v: Vec<u8> = Vec::with_capacity(1024 * 1024);  // 1MB
v.extend_from_slice(&[1, 2, 3]);  // 只用了 3 字节
// v 仍占用 1MB

// ✓ 用后收缩
v.shrink_to_fit();  // 释放多余容量

// ❌ 反复创建大 Vec
fn process(data: &[u8]) {
    let mut buf = vec![0u8; 1024 * 1024];  // 每次调用分配 1MB
    // 用 buf 处理...
}

// ✓ 复用缓冲（thread_local 或传入）
thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 1024 * 1024]);
}
```

## 3. 动态分发陷阱

### 3.1 热点循环的 trait object

```rust
// ❌ 热点循环用 dyn，无法内联
fn total_area(shapes: &[Box<dyn Shape>]) -> f64 {
    shapes.iter().map(|s| s.area()).sum()  // 每次 vtable 调用
}

// ✓ 用泛型（静态分发）
fn total_area<S: Shape>(shapes: &[S]) -> f64 {
    shapes.iter().map(|s| s.area()).sum()  // 内联，可能向量化
}

// ✓ 或用枚举
enum Shape { Circle(Circle), Rect(Rect) }
fn total_area(shapes: &[Shape]) -> f64 {
    shapes.iter().map(|s| match s {
        Shape::Circle(c) => c.area(),
        Shape::Rect(r) => r.area(),
    }).sum()
}
```

### 3.2 不必要的 Box

```rust
// ❌ 不必要的堆分配
fn make_iter() -> Box<dyn Iterator<Item = i32>> {
    Box::new((0..10).map(|x| x * 2))  // 堆分配
}

// ✓ 用 impl Trait（栈上）
fn make_iter() -> impl Iterator<Item = i32> {
    (0..10).map(|x| x * 2)  // 零分配
}
```

## 4. 字符串陷阱

### 4.1 chars() vs bytes()

```rust
// ❌ 用 chars() 做字节级操作
fn is_ascii_alphanumeric(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric())  // 解码 UTF-8，慢
}

// ✓ 用 bytes()
fn is_ascii_alphanumeric(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_alphanumeric())  // 直接字节，快 3-5x
}

// ✓ 更快：用 SIMD 加速的 memchr
```

### 4.2 字符串分割的隐藏分配

```rust
// ❌ 每次分割分配 Vec
let parts: Vec<&str> = line.split(',').collect();  // 分配
for p in &parts { process(p); }

// ✓ 直接迭代，零分配
for p in line.split(',') {
    process(p);
}
```

### 4.3 不必要的 String 字段

```rust
// ❌ 用 String 存储只读的配置键
struct Config {
    env: String,  // 总是 "production" 或 "staging"
}

// ✓ 用 enum（零分配，可比较）
enum Env { Production, Staging, Development }
struct Config {
    env: Env,
}
```

## 5. 迭代器陷阱

### 5.1 用 iter() 当不需要索引

```rust
// ❌ 用索引访问（边界检查开销）
for i in 0..vec.len() {
    process(vec[i]);  // 每次边界检查
}

// ✓ 直接迭代
for item in &vec {
    process(item);  // 无边界检查
}

// ✓ 需要索引用 enumerate
for (i, item) in vec.iter().enumerate() {
    process(i, item);
}
```

### 5.2 嵌套循环的低效

```rust
// ❌ O(n*m) 嵌套查找
for user in &users {
    for order in &orders {  // 全表扫描
        if order.user_id == user.id {
            // ...
        }
    }
}

// ✓ 用 HashMap 索引（O(n+m)）
use std::collections::HashMap;
let orders_by_user: HashMap<UserId, Vec<&Order>> =
    orders.iter().fold(HashMap::new(), |mut acc, o| {
        acc.entry(o.user_id).or_default().push(o);
        acc
    });
for user in &users {
    if let Some(orders) = orders_by_user.get(&user.id) {
        for order in orders { /* ... */ }
    }
}
```

### 5.3 不必要的排序

```rust
// ❌ 找最大值用排序
let mut sorted = data.clone();  // clone！
sorted.sort();
let max = sorted.last().unwrap();

// ✓ 直接求最大值
let max = data.iter().max();  // O(n) vs O(n log n)
```

## 6. 并发陷阱

### 6.1 锁粒度过大

```rust
// ❌ 整个数据结构一把锁，所有操作互斥
struct Service {
    data: Mutex<BigData>,
}
fn op1(&self) {
    let guard = self.data.lock().unwrap();
    // 大量计算 + I/O，期间其他线程全阻塞
}

// ✓ 分片锁或细粒度锁
struct Service {
    shards: [Mutex<Shard>; 16],  // 16 个分片，减少竞争
}
fn op1(&self, key: u64) {
    let shard = &self.shards[(key % 16) as usize];
    let guard = shard.lock().unwrap();  // 只锁一个分片
}
```

### 6.2 锁内做 I/O

```rust
// ❌ 锁内做异步或阻塞 I/O
async fn bad(state: &Mutex<State>) {
    let guard = state.lock().await;
    let data = fetch_remote().await;  // 持锁期间网络等待！
    guard.update(data);
}

// ✓ 锁外做 I/O
async fn good(state: &Mutex<State>) {
    let data = fetch_remote().await;  // 先获取数据
    let mut guard = state.lock().await;
    guard.update(data);  // 锁内只做内存更新
}
```

### 6.3 Arc<Mutex<T>> 的滥用

```rust
// ❌ 高并发读用 Mutex（读也要互斥）
let cache: Arc<Mutex<HashMap<K, V>>> = Arc::new(Mutex::new(HashMap::new()));

// ✓ 读多写少用 RwLock
let cache: Arc<RwLock<HashMap<K, V>>> = Arc::new(RwLock::new(HashMap::new()));

// ✓ 更好：高并发用 DashMap（无锁）
let cache: DashMap<K, V> = DashMap::new();
```

### 6.4 单线程用 Arc

```rust
// ❌ 单线程不需要 Arc
let data = Arc::new(vec![1, 2, 3]);  // 原子操作开销
let d2 = Arc::clone(&data);  // 原子递增

// ✓ 单线程用 Rc
let data = Rc::new(vec![1, 2, 3]);  // 无原子开销
```

## 7. 异步陷阱

### 7.1 await 持有非 Send 数据

```rust
// ❌ 跨 await 持有 Rc，Future 不 Send
async fn bad() {
    let data = Rc::new(vec![1, 2, 3]);
    process(&data).await;  // Rc 跨 await 点 → 不 Send
}
tokio::spawn(bad());  // ❌ 编译失败

// ✓ 用 Arc
async fn good() {
    let data = Arc::new(vec![1, 2, 3]);
    process(&data).await;
}
```

### 7.2 阻塞调用阻塞运行时

```rust
// ❌ 在 async 中阻塞
async fn bad() {
    std::thread::sleep(Duration::from_secs(1));  // 阻塞整个 worker！
    let data = std::fs::read("file").unwrap();   // 阻塞！
}

// ✓ 用异步版本
async fn good() {
    tokio::time::sleep(Duration::from_secs(1)).await;
    let data = tokio::fs::read("file").await.unwrap();
}

// ✓ CPU 密集用 spawn_blocking
async fn heavy() {
    tokio::task::spawn_blocking(|| {
        expensive_cpu_work()
    }).await.unwrap();
}
```

### 7.3 不必要的 Box<dyn Future>

```rust
// ❌ 用 Box 包装 Future（堆分配）
fn fetch() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async { /* ... */ })
}

// ✓ 用 impl Future（栈上，零分配）
fn fetch() -> impl Future<Output = ()> + Send {
    async { /* ... */ }
}
// 注意：递归 Future 仍需 Box（大小未知）
```

## 8. 编译优化陷阱

### 8.1 没开 LTO

```toml
# ❌ 默认 release profile 不开 LTO
[profile.release]
opt-level = 3

# ✓ 开启 LTO + 单 codegen unit
[profile.release]
opt-level = 3
lto = "fat"           # 全程序优化
codegen-units = 1     # 更好的优化（更慢编译）
panic = "abort"       # 更小更快
strip = true          # 去符号
```

### 8.2 debug 模式跑生产

```rust
// ❌ debug 模式（默认）慢 10-100x
cargo run  # 没开优化

// ✓ 用 release
cargo run --release

// ✓ 临时开发用 dev-opt
[profile.dev]
opt-level = 1  # 至少开 O1
```

### 8.3 没用 CPU 特性

```toml
# Cargo.toml
# ✓ 针对当前 CPU 编译（用 AVX2/AVX-512 等）
[profile.release]
# 或在 .cargo/config.toml:
# [build]
# rustflags = ["-C", "target-cpu=native"]

# ⚠️ 注意：会失去跨机器兼容性，部署到其他 CPU 可能崩溃
```

## 9. 算法陷阱

### 9.1 用 O(n²) 算法当 O(n) 可行

```rust
// ❌ O(n²) 去重
fn dedup(mut v: Vec<i32>) -> Vec<i32> {
    let mut result = Vec::new();
    for x in v {
        if !result.contains(&x) {  // O(n) 查找
            result.push(x);
        }
    }
    result
}  // 总复杂度 O(n²)

// ✓ O(n) 用 HashSet
fn dedup(v: Vec<i32>) -> Vec<i32> {
    let set: HashSet<i32> = v.into_iter().collect();
    set.into_iter().collect()
}

// ✓ 保留顺序 + O(n)
fn dedup_ordered(v: Vec<i32>) -> Vec<i32> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|x| seen.insert(*x)).collect()
}
```

### 9.2 反复 clone+sort

```rust
// ❌ 每次查询都 sort
fn top_n(mut data: Vec<i32>, n: usize) -> Vec<i32> {
    data.sort();
    data.into_iter().rev().take(n).collect()
}
// 每次 O(n log n)

// ✓ 用 partial sort / heap
use std::collections::BinaryHeap;
fn top_n(data: Vec<i32>, n: usize) -> Vec<i32> {
    let heap: BinaryHeap<_> = data.into_iter().collect();
    heap.into_iter().take(n).collect()
}
// 构建 heap O(n)，取 top n O(n log k)
```

## 10. I/O 陷阱

### 10.1 小写入无缓冲

```rust
// ❌ 每行一次 write 系统调用
let mut file = File::create("log.txt")?;
for line in &lines {
    file.write_all(line.as_bytes())?;  // 每次系统调用
}

// ✓ 用 BufWriter 批量
use std::io::BufWriter;
let mut writer = BufWriter::with_capacity(8192, File::create("log.txt")?);
for line in &lines {
    writer.write_all(line.as_bytes())?;
}
writer.flush()?;
```

### 10.2 读整个文件到内存

```rust
// ❌ 大文件读入内存
let content = std::fs::read_to_string("10GB.log")?;  // OOM！

// ✓ 流式处理
use std::io::{BufReader, BufRead};
let file = File::open("10GB.log")?;
let reader = BufReader::new(file);
for line in reader.lines() {
    let line = line?;
    process(&line);  // 每次只有一行在内存
}
```

## 11. 工具辅助发现陷阱

```bash
# Clippy 性能 lint
cargo clippy -- -W clippy::perf -W clippy::pedantic | grep perf

# 关键 lint：
# clippy::redundant_clone          # 冗余 clone
# clippy::needless_collect         # 不必要的 collect
# clippy::useless_vec             # 数组可用时用 Vec
# clippy::iter_overeager_cloned   # 过早 clone 迭代器
# clippy::large_enum_variant      # 大枚举变体该 Box
# clippy::trivial_regex           # 编译期正则

# 堆分析
cargo install dhat
cargo build --features dhat
./target/release/myapp  # 输出堆分析报告

# 火焰图
cargo install flamegraph
cargo flamegraph --bin myapp

# 基准测试
cargo install criterion
cargo bench
```

## 12. 优化决策树

```
发现性能问题
    ↓
是否在热点路径？──否──→ 不优化（过早优化是万恶之源）
    │是
    ↓
有 benchmark 量化吗？──否──→ 先写 benchmark
    │有
    ↓
profile 找瓶颈
    ↓
瓶颈类型？
├─ 分配 ──→ 减 clone / 预分配 / 复用缓冲
├─ 查找 ──→ 换数据结构（HashSet/BTreeMap）
├─ 循环 ──→ 减少嵌套 / 算法优化 / 并行（rayon）
├─ I/O ──→ 缓冲 / 批量 / 异步
├─ 锁 ──→ 减小粒度 / 分片 / 无锁结构
└─ 分发 ──→ 静态分发 / 内联 / 枚举
    ↓
优化后再 benchmark 验证
    ↓
改进显著？──否──→ 回滚（复杂度增加不值）
    │是
    ↓
合并，加注释说明为何这样优化
```

## 13. 性能陷阱检查清单

- [ ] 无循环内 clone / to_string
- [ ] 集合预分配容量
- [ ] 无不必要的中间 collect
- [ ] 线性查找替换为 HashSet/HashMap
- [ ] 热点路径无 trait object（用泛型/枚举）
- [ ] 索引访问替换为直接迭代
- [ ] 锁粒度最小，锁内无 I/O
- [ ] 读多写少用 RwLock 而非 Mutex
- [ ] async 中无阻塞调用
- [ ] 字符串拼接用 format! 或 write!
- [ ] release profile 开启 LTO
- [ ] 大文件流式处理，不读入内存
- [ ] 用 clippy perf lint 自动检查

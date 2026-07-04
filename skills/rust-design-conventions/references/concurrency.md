# 并发编程（Fearless Concurrency）

> Rust 的"无畏并发"核心：编译器在编译期保证线程安全，让你在享受多线程性能的同时不担心数据竞争。

## 1. Send 与 Sync

这是 Rust 并发安全的基石——两个 marker trait。

### 定义

| Trait | 含义 | 自动实现条件 |
|-------|------|-------------|
| `Send` | **类型的所有权**可以安全地跨线程转移 | 所有字段都是 Send |
| `Sync` | **类型的引用 `&T`** 可以安全地跨线程共享 | `&T` 是 Send，即 `T` 可被多线程同时读 |

### 常见类型的 Send/Sync

```
类型                  Send   Sync   说明
─────────────────────────────────────────────────────────
i32, f64, bool       ✓     ✓     基本类型
String, Vec<T>       ✓(若T:Send) ✓(若T:Sync)
Box<T>               ✓(若T:Send) ✓(若T:Sync)
Rc<T>                ✗     ✗     非原子计数，单线程专用
Arc<T>               ✓(若T:Send+Sync) ✓(若T:Send+Sync)
Cell<T>              ✓     ✗     内部可变，非线程安全
RefCell<T>           ✓     ✗     运行时借用检查，非线程安全
Mutex<T>             ✓     ✓     互斥锁
RwLock<T>            ✓     ✓     读写锁
*const T / *mut T    ✗     ✗     裸指针
```

### 关键直觉

```rust
// Rc 不是 Send，因此下面的代码编译失败
let rc = Rc::new(42);
std::thread::spawn(move || {
    println!("{rc}");  // ❌ Rc<i32> 不满足 Send
});

// Arc 是 Send，可以
let arc = Arc::new(42);
std::thread::spawn(move || {
    println!("{arc}");  // ✓
});

// RefCell 不是 Sync，不能跨线程共享引用
let cell = RefCell::new(42);
let r = &cell;
std::thread::spawn(move || {
    r.borrow();  // ❌ &RefCell 不满足 Send（因 RefCell 不是 Sync）
});
```

## 2. 线程创建与基础

### std::thread

```rust
use std::thread;
use std::time::Duration;

// 基本线程
let handle = thread::spawn(|| {
    for i in 0..5 {
        println!("spawned: {i}");
        thread::sleep(Duration::from_millis(1));
    }
});

for i in 0..3 {
    println!("main: {i}");
    thread::sleep(Duration::from_millis(1));
}

handle.join().unwrap();  // 等待子线程结束
```

### 线程间数据共享

```rust
// 1. move 转移所有权（单消费者）
let data = vec![1, 2, 3];
let handle = thread::spawn(move || {
    println!("{data:?}");  // data 所有权转移到子线程
});

// 2. Arc 共享只读
let data = Arc::new(vec![1, 2, 3]);
let data_clone = Arc::clone(&data);
let handle = thread::spawn(move || {
    println!("{data_clone:?}");  // 共享只读访问
});
drop(data);  // 引用计数减 1，但子线程仍持有

// 3. Arc<Mutex<T>> 共享可变
use std::sync::Mutex;
let counter = Arc::new(Mutex::new(0));
let mut handles = vec![];
for _ in 0..10 {
    let counter = Arc::clone(&counter);
    handles.push(thread::spawn(move || {
        let mut num = counter.lock().unwrap();
        *num += 1;
    }));
}
for h in handles { h.join().unwrap(); }
println!("Result: {}", *counter.lock().unwrap());  // 10
```

## 3. 同步原语选择

### 互斥锁（Mutex）

```rust
use std::sync::Mutex;

// 适合：读写都频繁，或写多于读
let data = Mutex::new(Vec::new());

// 锁的范围要尽量小
{
    let mut guard = data.lock().unwrap();
    guard.push(42);          // 临界区内做最小工作
}  // guard 在此处 drop，释放锁

// ❌ 反模式：锁持有期间做耗时操作
let guard = data.lock().unwrap();
do_expensive_io().await;     // 阻塞其他线程！
guard.push(42);

// ✓ 正确：先 clone 出数据再释放锁
let item = {
    let guard = data.lock().unwrap();
    guard.last().cloned()
};  // 锁释放
if let Some(x) = item {
    process(x).await;  // 锁外做耗时操作
}
```

### 读写锁（RwLock）

```rust
use std::sync::RwLock;

// 适合：读远多于写
let config = RwLock::new(Config::default());

// 多个读者可并发
let r1 = config.read().unwrap();
let r2 = config.read().unwrap();  // 可并发读
println!("{r1:?} {r2:?}");
drop(r1); drop(r2);

// 写者独占
let mut w = config.write().unwrap();
w.timeout = 30;  // 阻塞所有其他读写
```

**注意：** Rust 标准库的 RwLock 在 Linux 上用 pthread 实现，可能写者优先导致读者饥饿。对性能敏感场景考虑 `parking_lot::RwLock`（更公平、更快）。

### parking_lot 的优势

```rust
// Cargo.toml: parking_lot = "0.12"
use parking_lot::{Mutex, RwLock};

// 1. 不会 poison（标准库 Mutex lock 返回 Result，因可能 panic 污染）
let guard = mutex.lock();  // 直接返回 guard，不是 Result
// 2. 更快（不使用系统 futex 的快速路径）
// 3. 更小的内存占用（标准库 Mutex 占 1 个字，parking_lot 同样）
```

### 原子类型（Atomic）

```rust
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

// 无锁计数器
let counter = AtomicUsize::new(0);

// 原子操作
counter.fetch_add(1, Ordering::Relaxed);    // counter += 1
counter.compare_exchange(
    5,                      // 期望值
    10,                     // 新值
    Ordering::SeqCst,       // 成功时的内存序
    Ordering::SeqCst,       // 失败时的内存序
).ok();
```

### Ordering（内存序）选择

| Ordering | 含义 | 性能 | 适用 |
|----------|------|------|------|
| `Relaxed` | 只保证操作原子性，无内存序约束 | 最快 | 计数器、统计 |
| `Acquire` | 后续读写不能重排到此操作前 | 快 | 读锁、加载 flag |
| `Release` | 前序读写不能重排到此操作后 | 快 | 写锁、存储 flag |
| `AcqRel` | Acquire + Release | 中 | RMW 操作（fetch_add） |
| `SeqCst` | 全局顺序一致 | 最慢 | 不确定时的默认选择 |

```rust
// 经典模式：用 Acquire/Release 同步 flag
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static READY: AtomicBool = AtomicBool::new(false);
static mut DATA: i32 = 0;

// 线程 A：写数据，然后设置 flag
unsafe { DATA = 42; }
READY.store(true, Ordering::Release);  // 保证 DATA=42 对其他线程可见

// 线程 B：等 flag，然后读数据
while !READY.load(Ordering::Acquire) {
    thread::yield_now();
}
assert_eq!(unsafe { DATA }, 42);  // 一定能看到 42
```

## 4. 通道（Channel）

### 单生产者单消费者（SPSC）

```rust
use std::sync::mpsc;
use std::thread;

let (tx, rx) = mpsc::channel();

thread::spawn(move || {
    tx.send(42).unwrap();
});

println!("{}", rx.recv().unwrap());  // 阻塞接收
```

### 多生产者（MPSC）

```rust
let (tx, rx) = mpsc::channel();
let tx2 = tx.clone();  // clone tx 实现多生产者

thread::spawn(move || { tx.send("from t1").unwrap(); });
thread::spawn(move || { tx2.send("from t2").unwrap(); });

// rx 接收两个消息
for _ in 0..2 {
    println!("{}", rx.recv().unwrap());
}
```

### crossbeam 通道（更高性能）

```rust
// Cargo.toml: crossbeam = "0.8"
use crossbeam::channel::{bounded, unbounded};

// 有界通道：背压控制
let (s, r) = bounded(100);  // 缓冲 100 条
// 当缓冲满时，send 阻塞（生产者背压）

// 无界通道
let (s, r) = unbounded();

// 多生产者多消费者（标准库 mpsc 不支持多消费者）
let (s, r) = bounded(100);
let r2 = r.clone();  // crossbeam 支持多消费者
thread::spawn(move || { while let Ok(msg) = r.recv() { process(msg); } });
thread::spawn(move || { while let Ok(msg) = r2.recv() { process(msg); } });
```

### select! 多路复用

```rust
use crossbeam::select;

loop {
    select! {
        recv(r1) -> msg => println!("from r1: {msg:?}"),
        recv(r2) -> msg => println!("from r2: {msg:?}"),
        recv(tick) -> _ => println!("tick"),
        default(Duration::from_secs(1)) => println!("timeout"),
    }
}
```

## 5. 数据并行（Rayon）

```rust
use rayon::prelude::*;

// 顺序迭代
let sum: i64 = items.iter().map(|x| x * 2).sum();

// 并行迭代（自动分片到多核）
let sum: i64 = items.par_iter().map(|x| x * 2).sum();

// 适用场景：
// - 数据量大（至少几千个元素）
// - 每个元素的计算开销较大（避免任务调度开销占主导）
// - 计算独立无依赖

// 控制并行度
let pool = rayon::ThreadPoolBuilder::new()
    .num_threads(4)
    .build()
    .unwrap();

// 并行 sort（比单线程快 N 倍）
let mut data = vec![5, 2, 8, 1, 9, 3];
data.par_sort();
```

### 使用 scope 共享引用

```rust
// rayon::scope 允许子线程借用栈上数据
let mut data = vec![1, 2, 3];
rayon::scope(|s| {
    s.spawn(|_| {
        data.push(4);  // 借用主线程的 data
    });
    s.spawn(|_| {
        data.push(5);
    });
});
// scope 结束时所有子线程必然已 join，data 借用安全结束
```

## 5b. 高并发数据结构

### DashMap（无锁并发 HashMap）

```rust
// Cargo.toml: dashmap = "5"
use dashmap::DashMap;

let map: DashMap<String, u32> = DashMap::new();

// 多线程并发读写
for i in 0..10 {
    let map = map.clone();  // Arc 内部
    thread::spawn(move || {
        map.insert(format!("key{i}"), i);
    });
}

// 比 Arc<Mutex<HashMap>> 快得多：内部分片（sharding）
// 每个 shard 独立锁，不同 key 的访问互不阻塞
```

### concurrent-queue / crossbeam-queue

```rust
use concurrent_queue::ConcurrentQueue;

// 无锁队列，适合生产者-消费者模式
let queue: ConcurrentQueue<Task> = ConcurrentQueue::unbounded();
queue.push(task).unwrap();
let task = queue.pop().unwrap();
```

## 6. 线程池

### 线程池对比

| 库 | 类型 | 适用 |
|----|------|------|
| `std::thread::spawn` | 一任务一线程 | 短期、少量任务 |
| `rayon::ThreadPool` | 工作窃取线程池 | CPU 密集数据并行 |
| `tokio` runtime | 异步任务池 | I/O 密集、高并发 |
| `threadpool` crate | 固定大小线程池 | 阻塞任务队列 |

### 固定线程池处理阻塞任务

```rust
use threadpool::ThreadPool;
use std::sync::mpsc::channel;

let n_workers = 4;
let pool = ThreadPool::new(n_workers);

let (tx, rx) = channel();
for i in 0..n_workers * 2 {
    let tx = tx.clone();
    pool.execute(move || {
        tx.send(do_blocking_work(i)).unwrap();
    });
}

for _ in 0..n_workers * 2 {
    let result = rx.recv().unwrap();
    println!("{result:?}");
}
```

## 7. Thread-local 存储

```rust
use std::cell::RefCell;
use std::thread;

thread_local! {
    static COUNTER: RefCell<u32> = RefCell::new(1);
    static EVENTS: RefCell<Vec<String>> = RefCell::new(Vec::new());
}

// 每个线程有独立的副本，无需同步
COUNTER.with(|c| {
    *c.borrow_mut() = 2;
});

thread::spawn(|| {
    COUNTER.with(|c| {
        println!("{:?}", *c.borrow());  // 1（新线程的独立副本）
    });
}).join().unwrap();
```

**适用：** 减少锁竞争（每线程独立计数器）、随机数生成器、数据库连接。

## 8. 死锁与避免

### 死锁的四个必要条件
1. 互斥（资源不可共享）
2. 持有并等待（持有锁 A，等待锁 B）
3. 不可剥夺（锁不能被强制释放）
4. 循环等待（A 等 B，B 等 A）

### 避免死锁的策略

```rust
// 1. 固定锁顺序（lock ordering）
// 所有线程按相同顺序获取锁，避免循环等待
fn transfer(from: &Account, to: &Account, amount: u64) {
    // 总是先锁 id 小的账户
    let (first, second) = if from.id < to.id {
        (from, to)
    } else {
        (to, from)
    };
    let _g1 = first.balance.lock().unwrap();
    let _g2 = second.balance.lock().unwrap();
    // ...
}

// 2. 使用 try_lock + 超时
use std::time::Duration;
let g1 = mutex1.try_lock_for(Duration::from_millis(100));
match g1 {
    Some(guard) => { /* ... */ }
    None => { /* 获取锁失败，重试或放弃 */ }
}

// 3. 合并多个锁为一个结构（减少锁数量）
struct Combined {
    inner: Mutex<CombinedInner>,  // 一个锁保护多个字段
}
struct CombinedInner {
    counter: u32,
    cache: HashMap<K, V>,
}

// 4. 避免在持有锁时调用未知代码（回调可能反向获取锁）
let data = {
    let guard = mutex.lock().unwrap();
    guard.data.clone()  // clone 出来
};  // 锁已释放
callback(data);  // 在锁外调用回调
```

## 9. 并发检查清单

- [ ] 线程间共享数据用 Arc，不要用 Rc（Rc 非 Send）
- [ ] 多线程可变数据用 Mutex/RwLock/DashMap
- [ ] 锁的临界区尽量小（避免在锁内做 I/O 或 sleep）
- [ ] 读远多于写时用 RwLock 而非 Mutex
- [ ] 简单计数器用 AtomicXxx 而非 Mutex
- [ ] 多消费者场景用 crossbeam channel 而非 std mpsc
- [ ] CPU 密集并行用 rayon
- [ ] 多线程 HashMap 用 DashMap 而非 Arc<Mutex<HashMap>>
- [ ] 锁顺序全局统一，避免死锁
- [ ] 考虑 parking_lot 替代标准库锁（更快、不 poison）
- [ ] 线程局部数据用 thread_local! 减少锁竞争

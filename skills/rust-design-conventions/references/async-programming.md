# 异步编程深度指南

> Rust 的 async 是零成本协作式并发：Future 是状态机，async fn 编译为状态机转换，没有 GC、没有虚拟机开销。

## 1. Future 基础

### Future trait

```rust
trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}

enum Poll<T> {
    Ready(T),
    Pending,
}
```

**核心机制：** Future 是惰性的——只有被 poll 时才会执行。创建 Future 不开始任何工作。

```rust
// 创建 Future 不会执行任何东西
let fut = async { println!("hello"); };
// 这里 "hello" 还没打印！

// 必须被 await 或 spawn 才会执行
fut.await;  // 现在才打印
```

### 手写 Future 示例

```rust
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

struct Delay { when: Instant }

impl Future for Delay {
    type Output = &'static str;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<&'static str> {
        if Instant::now() >= self.when {
            Poll::Ready("done")
        } else {
            // 注册 waker，资源就绪时唤醒
            let waker = cx.waker().clone();
            let when = self.when;
            std::thread::spawn(move || {
                let now = Instant::now();
                if now < when {
                    std::thread::sleep(when - now);
                }
                waker.wake();
            });
            Poll::Pending
        }
    }
}
```

### async fn 是语法糖

```rust
// 这两段代码等价
async fn fetch_user(id: u64) -> User {
    let row = db.query(id).await?;
    User::from(row)
}

// 等价于（编译器生成的状态机）
fn fetch_user(id: u64) -> impl Future<Output = Result<User>> {
    FetchUserFuture::Start { id }
}

// 状态机：
enum FetchUserFuture {
    Start { id: u64 },
    AwaitingDb { query: DbQuery },
    Done,
}
```

## 2. Pin 与自引用

### 为什么需要 Pin

async 块生成的状态机可能包含**自引用**（一个字段引用同结构内的另一个字段）：

```rust
// 自引用结构（危险）
struct SelfRef {
    data: String,
    // ptr 指向 data 内部字节
    ptr: *const u8,
}

let mut s = SelfRef { data: "hello".into(), ptr: std::ptr::null() };
s.ptr = s.data.as_ptr();

// ❌ 如果 move s，data 移动到新地址，但 ptr 仍指向旧地址 → 悬垂指针！
let s2 = s;  // move 发生
// 访问 s2.ptr → use after free
```

`Pin<P>` 保证被引用的值不会被 move，从而让自引用安全。

### Pin 的规则

```rust
use std::pin::Pin;

// Pin<P> 是包裹指针的 wrapper
// 一旦数据被 Pin，不能通过安全代码获取 &mut T 来 move 它

// Unpin trait：表示类型即使被 move 也安全（无自引用）
// 大多数类型都是 Unpin：i32, String, Vec, ...
// async 块/函数返回的 Future 通常 !Unpin（可能自引用）

// 实践：
// 1. Box::pin 把 Future 放到堆上并 Pin
let fut = Box::pin(async { 42 });
fut.await;

// 2. pin! 宏把 Future Pin 在栈上
tokio::pin!(fut);
fut.await;

// 3. 不要在 Unpin 类型上纠结 Pin（基本类型都是 Unpin）
```

### 处理 !Unpin 类型

```rust
use tokio::pin;

// 用 pin! 宏在栈上 pin（零分配）
async fn example() {
    let fut = some_async_fn();
    pin!(fut);  // fut: Pin<&mut ...>
    (&mut fut).await;
}

// 用 Box::pin 堆上 pin（有分配但可跨函数边界传递）
fn make_fut() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async { /* ... */ })
}
```

## 3. 运行时（Tokio）

### Tokio 基础

```rust
#[tokio::main]
async fn main() {
    println!("Hello from async!");
}
// 等价于：
fn main() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        println!("Hello from async!");
    });
}
```

### 运行时类型

```rust
// 1. 多线程运行时（默认，工作窃取）
#[tokio::main]
async fn main() { /* ... */ }

// 2. 单线程运行时（轻量）
#[tokio::main(flavor = "current_thread")]
async fn main() { /* ... */ }

// 3. 自定义 worker 线程数
#[tokio::main(worker_threads = 4)]
async fn main() { /* ... */ }

// 4. 手动构建
let rt = tokio::runtime::Builder::new_multi_thread()
    .worker_threads(4)
    .enable_all()         // 启用 time, io, net
    .thread_stack_size(2 * 1024 * 1024)
    .build()
    .unwrap();
```

### spawn 任务

```rust
#[tokio::main]
async fn main() {
    // tokio::spawn 返回 JoinHandle
    let handle = tokio::spawn(async {
        do_work().await
    });

    // 等待任务完成
    let result = handle.await.unwrap();
}

// spawn 的要求：Future 必须 Send + 'static
// 因为任务可能被调度到不同线程
tokio::spawn(async {
    let rc = Rc::new(42);  // ❌ Rc 不是 Send
    println!("{rc}");
});

tokio::spawn(async {
    let arc = Arc::new(42);  // ✓ Arc 是 Send + Sync
    println!("{arc}");
});
```

## 4. 异步 I/O

### Tokio 网络

```rust
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;

    loop {
        let (socket, addr) = listener.accept().await?;
        // 每个连接 spawn 一个任务
        tokio::spawn(async move {
            handle_connection(socket).await
        });
    }
}

async fn handle_connection(mut socket: TcpStream) {
    let mut buf = [0; 1024];
    loop {
        let n = match socket.read(&mut buf).await {
            Ok(0) => return,  // 连接关闭
            Ok(n) => n,
            Err(_) => return,
        };
        if socket.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}
```

### 异步文件 I/O

```rust
// tokio::fs 实际上用 spawn_blocking 包装阻塞 I/O
let content = tokio::fs::read_to_string("file.txt").await?;

// 大量文件 I/O 时，直接用 spawn_blocking 更明确
let data = tokio::task::spawn_blocking(|| {
    std::fs::read_to_string("file.txt")
}).await.unwrap()?;
```

## 5. select! 多路复用

### 基本用法

```rust
use tokio::select;

tokio::pin!(fut1);
tokio::pin!(fut2);

loop {
    select! {
        result = &mut fut1 => {
            println!("fut1 完成: {result:?}");
            break;
        }
        result = &mut fut2 => {
            println!("fut2 完成: {result:?}");
            break;
        }
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            println!("超时");
            break;
        }
    }
}
```

### select! 的语义（重要陷阱）

```rust
// ⚠️ select! 每次循环会重新创建 Future
// 如果 Future 内部有进度，会丢失！
loop {
    select! {
        // ❌ 每次迭代创建新的 recv() Future
        msg = receiver.recv() => {
            println!("{msg:?}");
        }
    }
}
// 这看起来对，但效率低（每次重建 Future）

// ✓ 正确：pin 住 Future，复用
tokio::pin! {
    let recv_fut = receiver.recv();
}
loop {
    select! {
        msg = &mut recv_fut => {
            println!("{msg:?}");
            // 注意：完成后需要重建
            recv_fut.set(receiver.recv());
        }
    }
}

// 或者用 fused future
use futures::stream::StreamExt;
while let Some(msg) = receiver.next().await {  // stream API 自动 fuse
    println!("{msg:?}");
}
```

### select! 公平性

```rust
// select! 默认随机选择就绪的分支，避免饥饿
// 但如果你需要优先级：
select! {
    // 优先检查 shutdown
    _ = shutdown.recv() => break,
    // 然后处理消息
    msg = msg_recv.recv() => process(msg),
}

// ⚠️ 上面的代码：如果 shutdown 和 msg 同时就绪，
// select! 会随机选，不保证优先 shutdown
// 如果需要严格优先级，用嵌套 select! 或 biased

select! {
    biased;  // 改为按声明顺序优先
    _ = shutdown.recv() => break,
    msg = msg_recv.recv() => process(msg),
}
```

## 6. 异步同步原语

### tokio::Mutex vs std::sync::Mutex

```rust
// ❌ 反模式：在异步代码中用 std::sync::Mutex 持有跨 await 的锁
async fn bad(mutex: &std::sync::Mutex<Data>) {
    let guard = mutex.lock().unwrap();
    do_async().await;  // 持有锁期间 await → 可能阻塞整个 worker 线程
    guard.process();
}

// ✓ 方案 A：用 tokio::sync::Mutex（可以跨 await 持有）
async fn good(mutex: &tokio::sync::Mutex<Data>) {
    let guard = mutex.lock().await;
    do_async().await;  // 持有锁期间 await OK
    guard.process();
}

// ✓ 方案 B（更快）：std::sync::Mutex，但锁不跨 await
async fn fast(mutex: &std::sync::Mutex<Data>) {
    let result = {
        let guard = mutex.lock().unwrap();
        guard.compute()  // 锁内只做同步操作
    };  // 锁释放
    do_async(result).await;  // 锁外 await
}
```

**原则：** 优先用 std Mutex + 短临界区 + 锁外 await。只有必须跨 await 持有锁时才用 tokio Mutex。

### 其他异步原语

```rust
use tokio::sync::{RwLock, Semaphore, mpsc, oneshot, broadcast, Notify};

// 1. Semaphore：限制并发数
let sem = Arc::new(Semaphore::new(10));  // 最多 10 并发
for url in urls {
    let permit = sem.clone().acquire_owned().await.unwrap();
    tokio::spawn(async move {
        fetch(url).await;
        drop(permit);  // 释放 permit
    });
}

// 2. oneshot：单次一次性通道
let (tx, rx) = oneshot::channel();
tokio::spawn(async move {
    let result = compute().await;
    tx.send(result).unwrap();
});
let result = rx.await.unwrap();

// 3. mpsc：多生产者单消费者
let (tx, mut rx) = mpsc::channel(100);
for i in 0..10 {
    let tx = tx.clone();
    tokio::spawn(async move {
        tx.send(i).await.unwrap();
    });
}
while let Some(msg) = rx.recv().await {
    println!("{msg}");
}

// 4. broadcast：多生产者多消费者（每个消费者独立）
let (tx, mut rx1) = broadcast::channel(100);
let mut rx2 = tx.subscribe();
tokio::spawn(async move {
    while let Ok(msg) = rx1.recv().await { println!("rx1: {msg}"); }
});
tokio::spawn(async move {
    while let Ok(msg) = rx2.recv().await { println!("rx2: {msg}"); }
});
tx.send(42).unwrap();

// 5. Notify：轻量通知
let notify = Arc::new(Notify::new());
let notify2 = notify.clone();
tokio::spawn(async move {
    notify2.notified().await;
    println!("被通知");
});
tokio::time::sleep(Duration::from_secs(1)).await;
notify.notify_one();
```

## 7. Spawn blocking

```rust
// CPU 密集或阻塞操作必须用 spawn_blocking
// 否则会阻塞 tokio worker 线程，影响其他任务

// ❌ 阻塞整个 worker
async fn bad() {
    let result = std::fs::read("huge.bin").unwrap();  // 阻塞 syscall！
    process(result).await;
}

// ✓ 用 spawn_blocking
async fn good() {
    // 在专门的阻塞线程池运行
    let result = tokio::task::spawn_blocking(|| {
        std::fs::read("huge.bin").unwrap()
    }).await.unwrap();
    process(result).await;
}

// CPU 密集
async fn hash_file(path: String) -> String {
    tokio::task::spawn_blocking(move || {
        // 这个 SHA-256 计算可能要几百毫秒
        sha256_file(&path)
    }).await.unwrap()
}
```

## 8. 取消与超时

### 超时

```rust
use tokio::time::timeout;

// 5 秒超时
match timeout(Duration::from_secs(5), do_work()).await {
    Ok(result) => println!("完成: {result:?}"),
    Err(_) => println!("超时"),
}

// Deadline（绝对时间点）
let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
match tokio::time::timeout_at(deadline, long_task()).await {
    Ok(result) => { /* ... */ },
    Err(_) => { /* 超时 */ },
}
```

### 取消语义

```rust
// Rust 的 Future 取消 = drop
// 当 Future 被 drop，它的所有子 Future 也被 drop，资源被清理

// select! 中未完成的分支会被 drop
select! {
    result = long_op() => println!("完成: {result}"),
    _ = timeout(Duration::from_secs(1)) => {
        // long_op() 的 Future 在此处被 drop → 取消
    }
}

// 协作式取消：通过 select! 监听取消信号
async fn cancellable_work(mut cancel: CancellationToken) {
    loop {
        select! {
            _ = cancel.cancelled() => {
                cleanup().await;  // 取消时清理
                return;
            }
            item = next_item() => {
                process(item).await;
            }
        }
    }
}

// tokio::select! 一次只 drop 未完成的分支，已完成的分支会被执行
```

## 9. Stream（异步迭代器）

```rust
use futures::stream::{Stream, StreamExt};

// Stream 是异步版的 Iterator
// trait Stream { async fn next() -> Option<T> }

// 使用
let mut stream = receiver_stream();
while let Some(item) = stream.next().await {
    process(item);
}

// Stream 组合子
let results: Vec<_> = stream
    .filter(|x| async { x.is_valid() })
    .map(|x| async move { transform(x).await })
    .buffer_unordered(10)  // 最多 10 个并发
    .collect()
    .await;

// Tokio 的 mpsc Receiver 实现了 Stream
use tokio_stream::wrappers::ReceiverStream;
let (tx, rx) = tokio::sync::mpsc::channel(100);
let stream = ReceiverStream::new(rx);
```

### 并发处理 Stream

```rust
use futures::stream::{StreamExt, FuturesUnordered};

// 方式 1：buffer_unordered（不保证顺序）
let results = stream
    .map(|url| fetch(url))
    .buffer_unordered(50)  // 50 并发，谁先完成谁先出
    .collect::<Vec<_>>()
    .await;

// 方式 2：FuturesUnordered 手动管理
let mut futures = FuturesUnordered::new();
for url in urls {
    futures.push(fetch(url));
}
while let Some(result) = futures.next().await {
    process(result);
}

// 方式 3：并发 + 顺序（buffer_ordered）
let results = stream
    .map(|url| fetch(url))
    .buffered(50)  // 50 并发，但保持原始顺序
    .collect::<Vec<_>>()
    .await;
```

## 10. 常见陷阱

### 1. 忘记 await

```rust
// ❌ 创建 Future 但不 await
async fn handler() {
    log::info!("start");
    save_to_db();  // 没 await！Future 被 drop，操作未执行
    log::info!("end");
}

// ✓ 正确
async fn handler() {
    save_to_db().await;
}
```

### 2. 阻塞调用阻塞运行时

```rust
// ❌ 任何阻塞调用都会阻塞 worker 线程
async fn bad() {
    std::thread::sleep(Duration::from_secs(5));  // 阻塞整个 worker！
    let _ = std::fs::read("file").unwrap();      // 阻塞！
    let _ = reqwest::blocking::get(url).unwrap(); // 阻塞！
}

// ✓ 正确
async fn good() {
    tokio::time::sleep(Duration::from_secs(5)).await;
    let _ = tokio::fs::read("file").await.unwrap();
    let _ = reqwest::get(url).await.unwrap();
}
```

### 3. Send 要求

```rust
// ❌ 持有非 Send 数据跨 await
async fn bad() {
    let rc = Rc::new(vec![1, 2, 3]);  // Rc 不是 Send
    process(&rc).await;  // rc 跨 await → Future 不是 Send
}
tokio::spawn(bad());  // ❌ 编译错误

// ✓ 用 Arc
async fn good() {
    let arc = Arc::new(vec![1, 2, 3]);
    process(&arc).await;
}

// ✓ 或不跨 await 持有
async fn good2() {
    let result = {
        let rc = Rc::new(vec![1, 2, 3]);
        rc.len()  // 同步用完
    };  // rc drop
    async_op(result).await;
}
```

### 4. 不必要的 Box

```rust
// ❌ 把 Future Box 起来（额外堆分配）
fn make() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async { /* ... */ })
}

// ✓ 用 impl Future（零分配）
fn make() -> impl Future<Output = ()> + Send {
    async { /* ... */ }
}
// 注意：递归 Future 必须用 Box（编译器无法知道大小）
```

### 5. 高频 spawn 的开销

```rust
// ❌ 每个请求都 spawn，任务调度开销大
for item in items {
    tokio::spawn(process(item));  // 1M items → 1M tasks
}

// ✓ 批量处理
async fn process_all(items: Vec<Item>) {
    // 用 chunk + 并发控制
    for chunk in items.chunks(1000) {
        let futs: Vec<_> = chunk.iter().map(|i| process(i)).collect();
        futures::future::join_all(futs).await;
    }
}
```

## 11. 异步检查清单

- [ ] 阻塞 I/O 用 `tokio::fs` 或 `spawn_blocking`，不用 std 阻塞 API
- [ ] 跨 await 持有的锁用 `tokio::sync::Mutex`，否则用 `std::sync::Mutex`
- [ ] spawn 的 Future 满足 Send + 'static
- [ ] 用 `select!` 时注意 Future 重建陷阱（pin 住复用）
- [ ] 高并发用 Semaphore 限制并发数，避免压垮下游
- [ ] 用 `timeout` 保护可能挂起的 I/O
- [ ] Stream 处理用 buffer_unordered/buffered 控制并发
- [ ] CPU 密集任务用 `spawn_blocking`，不阻塞 worker
- [ ] 任务间通信用 mpsc/oneshot/broadcast channel
- [ ] 不要忘记 await（clippy::let_underscore_future 可帮忙）

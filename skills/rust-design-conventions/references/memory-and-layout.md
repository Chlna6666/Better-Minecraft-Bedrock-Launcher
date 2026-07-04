# 内存管理与底层知识

> Rust 的核心竞争力在于零运行时开销的同时提供内存安全。理解内存底层是写好 Rust 的前提。

## 1. Stack vs Heap

### 核心区别

| 维度 | Stack（栈） | Heap（堆） |
|------|------------|------------|
| 分配方式 | 编译期已知大小，自动分配/释放 | 运行时动态分配，需手动/自动管理 |
| 速度 | 极快（移动 SP 寄存器，纳秒级） | 慢（allocator 查找空闲块，可能涉及系统调用） |
| 缓存友好 | 连续访问，CPU 缓存命中率高 | 分散，缓存不友好 |
| 大小 | 有限（Linux 默认 8MB，主线程 1MB） | 几乎只受虚拟内存限制 |
| 生命周期 | LIFO，作用域结束自动回收 | 显式释放（Rust 通过所有权） |
| 存储 | 函数局部变量、`Copy` 类型、引用、指针本体 | `Box<T>`、`Vec<T>`、`String` 的数据本体 |

### 哪些数据在 Stack，哪些在 Heap

```rust
fn example() {
    // ─── Stack 上 ───
    let x: i32 = 42;                  // i32 是 Copy，直接在栈上
    let arr: [u8; 4] = [1, 2, 3, 4];  // 固定大小数组，在栈上
    let tuple: (i32, f64) = (1, 2.0); // 固定大小，在栈上
    let ptr: &i32 = &x;               // 引用本身（指针）在栈上

    // ─── Heap 上 ───
    let s: String = String::from("hello");  // String 结构在栈，数据在堆
    let v: Vec<i32> = vec![1, 2, 3];         // Vec 结构在栈，数据在堆
    let b: Box<i32> = Box::new(42);          // Box 指针在栈，i32 在堆

    // ─── 混合 ───
    let arr_of_strings: [String; 3] = [
        String::from("a"),  // 3 个 String 结构在栈，每个的数据在堆
        String::from("bb"),
        String::from("ccc"),
    ];
    // sizeof([String; 3]) = 3 * 24 = 72 bytes，全在栈上
    // 但 "a", "bb", "ccc" 的字符数据在堆上
}
```

### Stack 与 Heap 的实际内存图

```
Stack（从高地址向低地址生长）         Heap（从低地址向高地址生长）
┌──────────────────────┐             ┌──────────────────────┐
│ fn example 的栈帧     │             │ ... 已分配内存 ...    │
│  ├ x (i32):       4B │             │   ┌──────────────┐  │
│  ├ arr ([u8;4]):  4B │             │   │ "hello" data │  │← String::from("hello") 数据
│  ├ ptr (&i32):    8B │             │   └──────────────┘  │
│  ├ s (String):   24B │─── ptr ────→│   ┌──────────────┐  │
│  │   ├ ptr:      8B │             │   │ [1, 2, 3]    │  │← Vec::new() 数据
│  │   ├ len:      8B │             │   └──────────────┘  │
│  │   └ cap:      8B │             │   ┌──────────────┐  │
│  ├ v (Vec):      24B │─── ptr ────→│   │ 42 (i32)     │  │← Box::new(42) 数据
│  └ b (Box):       8B │─── ptr ────→│   └──────────────┘  │
└──────────────────────┘             └──────────────────────┘
```

## 2. 所有权系统与内存生命周期

### 所有权三原则
1. **每个值有且仅有一个所有者**（owner）
2. **当所有者离开作用域，值被销毁**（Drop::drop 被调用）
3. **赋值时默认是移动（move），除非类型实现了 Copy**

```rust
{
    let s1 = String::from("hi");   // s1 是 owner
    let s2 = s1;                   // 所有权 move 到 s2，s1 失效
    // println!("{s1}");           // ❌ 编译错误：s1 已被 move
}                                  // s2 离开作用域，drop 被调用，堆内存释放
```

### Move 语义的内存含义

```rust
let s1 = String::from("hello");
let s2 = s1;
// Move 发生了什么：
// 1. s1 的 24 字节（ptr, len, cap）被 bitwise 拷贝到 s2
// 2. s1 的 ptr 被逻辑置无效（编译期禁止访问）
// 3. 堆上的 "hello" 字节数据没动，没有拷贝
// 所以 move 是廉价的——只复制了栈上的元数据
```

### Clone 是深拷贝

```rust
let s1 = String::from("hello");
let s2 = s1.clone();
// Clone 做了什么：
// 1. 在堆上分配新的 5 字节空间
// 2. 将 "hello" 数据逐字节复制到新空间
// 3. s2 的 ptr 指向新分配的内存
// 所以 clone 是昂贵的——涉及堆分配和数据复制
```

### Copy 的条件
- 类型本身和所有字段都是 Copy（通常是 trivially copyable，无堆数据）
- 大小通常较小（建议 ≤ 24 字节）

```rust
#[derive(Clone, Copy)]
struct Point { x: f64, y: f64 }  // 16 字节，Copy 合理

// 大类型不应 Copy
struct BigArray([u8; 1024]);     // 1024 字节，每次拷贝代价大，不要 impl Copy
```

## 3. 内存布局

### #[repr] 属性

```rust
// 默认 repr：编译器可以重排字段以最小化 padding
struct Default {
    a: u8,    // 1 字节
    b: u64,   // 8 字节
    c: u8,    // 1 字节
}
// 编译器可能重排为 b, a, c，避免 padding 浪费

// repr(C)：按声明顺序布局，与 C ABI 兼容
#[repr(C)]
struct CLayout {
    a: u8,    // 1 字节 + 7 字节 padding
    b: u64,   // 8 字节
    c: u8,    // 1 字节 + 7 字节 padding
}
// sizeof = 24 字节，大量 padding

// repr(transparent)：包装类型与内部类型布局完全相同
#[repr(transparent)]
struct Wrapper(u64);  // sizeof(Wrapper) == sizeof(u64) == 8

// repr(packed)：去除所有 padding（危险！可能产生未对齐访问）
#[repr(packed)]
struct Packed {
    a: u8,
    b: u32,  // 紧跟在 a 后，无 padding
}

// repr(C, packed)：C 布局 + 紧凑（用于解析二进制协议）
#[repr(C, packed)]
struct Header {
    version: u8,
    flags: u16,
    length: u32,
}
```

### 枚举的内存布局

```rust
// Rust 枚举是 tagged union：discriminant（标签）+ 数据
enum E {
    A,                  // 无数据
    B(u8),              // 1 字节
    C(u64),             // 8 字节
    D(String),          // 24 字节
}
// sizeof(E) = 1 (discriminant) + padding + 24 (最大变体) = 32 字节
// 所有变体共用同一块内存（union 语义）

// Option<Box<T>> 的 niche optimization
// Box<T> 是非空指针，None 复用 null 表示，无需额外 discriminant
let x: Option<Box<u64>> = None;  // sizeof == 8 (与 Box<u64> 相同)

// Option<NonNull<T>> 同理
// 但 Option<Vec<T>> 没有 niche 优化，Vec 不保证非空
```

### 对齐（Alignment）

```rust
use std::mem::{align_of, size_of};

// 每种类型有对齐要求
assert_eq!(align_of::<u8>(), 1);
assert_eq!(align_of::<u32>(), 4);
assert_eq!(align_of::<u64>(), 8);

// 结构体的对齐 = 字段最大对齐
struct S { a: u8, b: u64 }
assert_eq!(align_of::<S>(), 8);

// 大小必须是 align 的整数倍
struct S2 { a: u8 }  // sizeof == 1, align == 1
struct S3 { a: u8, b: u8 }  // sizeof == 2, align == 1

// 通过 align_of 判断是否需要 padding
// 若字段未满足其类型的对齐要求，编译器插入 padding
```

### 字段重排示例

```rust
// ❌ 差布局：每个 u8 后都需要 7 字节 padding 给 u64
struct Bad {
    a: u8,    // 1B
    // 7B padding
    b: u64,   // 8B
    c: u8,    // 1B
    // 7B padding
}
// sizeof(Bad) = 24 bytes（只有 10 字节有效数据）

// ✓ 好布局：降序排列，padding 最小
struct Good {
    b: u64,   // 8B
    a: u8,    // 1B
    c: u8,    // 1B
    // 6B padding
}
// sizeof(Good) = 16 bytes（节省 8 字节）

// 注意：默认 repr 下 Rust 编译器会自动重排
// 但显式排列更清晰，便于团队理解和 #[repr(C)] 场景
```

## 4. 内存碎片

### 碎片产生机制

```
初始堆状态（连续空闲）：
┌───────────────────────────────────┐
│           Free                    │
└───────────────────────────────────┘

多次分配/释放后（外部碎片）：
┌──────┬──────┬──────┬──────┬──────┐
│ Used │ Free │ Used │ Free │ Used │
│ 16B  │ 4B   │ 32B  │ 8B   │ 16B  │
└──────┴──────┴──────┴──────┴──────┘
总空闲 = 12B，但最大连续空闲 = 8B
申请 12B 会失败，虽然总空闲够（外部碎片）
```

### 减少碎片的策略

```rust
// 1. 使用 arena allocator（一次性大块分配，统一释放）
use bumpalo::Bump;
let arena = Bump::new();
let node1 = arena.alloc(Node::new(1));  // 从 arena 分配
let node2 = arena.alloc(Node::new(2));
drop(arena);  // 一次性释放所有，无碎片

// 2. 对象池（复用对象，避免反复 alloc/dealloc）
use std::sync::Mutex;
struct Pool<T> {
    items: Mutex<Vec<Box<T>>>,
}
impl<T> Pool<T> {
    fn acquire(&self, new: impl FnOnce() -> T) -> Box<T> {
        self.items.lock().unwrap().pop().unwrap_or_else(|| Box::new(new()))
    }
    fn release(&self, item: Box<T>) {
        self.items.lock().unwrap().push(item);
    }
}

// 3. 使用 slab allocator（jemalloc/mimalloc）
// Cargo.toml: [dependencies] tikv-jemallocator = "0.5"
use tikv_jemallocator::Jemalloc;
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;
// jemalloc 在多线程和碎片化场景表现更好
```

### Vec 增长策略与碎片

```rust
// Vec 容量增长策略：通常翻倍（amortized O(1) push）
let mut v = Vec::new();
for i in 0..1000 {
    v.push(i);
    // 在容量 0,1,2,4,8,...,1024 时重新分配
    // 每次分配新空间、复制旧数据、释放旧空间
    // 旧空间释放后可能形成碎片
}

// 减少 Vec 碎片的实践：
// 1. 预分配容量
let mut v = Vec::with_capacity(1000);
for i in 0..1000 { v.push(i); }  // 0 次重新分配

// 2. 收缩多余容量
v.shrink_to_fit();  // 释放未使用的容量

// 3. 如果知道最终大小，用 collect 而非 push 循环
let v: Vec<_> = (0..1000).collect();  // 内部用 size_hint 预分配
```

## 5. 智能指针与分配器

### 指针类型选择

| 类型 | 场景 | 开销 |
|------|------|------|
| `&T` / `&mut T` | 借用，单一所有者作用域内 | 零（栈指针） |
| `Box<T>` | 单所有者堆分配 | 1 次堆 alloc + free |
| `Rc<T>` | 单线程多所有者 | 1 次 alloc + 引用计数加减 |
| `Arc<T>` | 多线程多所有者 | 1 次 alloc + 原子计数加减 |
| `Cow<'_, T>` | 可能借用也可能 owned | 零或 1 次 alloc |
| `*const T` / `*mut T` | FFI、不安全操作 | 零（裸指针） |

### 引用计数开销

```rust
use std::sync::Arc;

// Rc 单线程引用计数
let rc = Rc::new(100);  // 1 次 alloc
let rc2 = rc.clone();   // 非原子 ++count（每次 clone 几纳秒）
drop(rc);               // --count
drop(rc2);              // count=0，触发 dealloc

// Arc 多线程引用计数
let arc = Arc::new(100);  // 1 次 alloc
let arc2 = arc.clone();   // 原子 ++count（比 Rc 慢 5-10 倍）
// Arc 适合长时间共享，不适合短生命周期频繁 clone/drop
```

## 6. 全局分配器

### 替换全局分配器

```rust
// Cargo.toml
// [dependencies]
// tikv-jemallocator = "0.5"

// main.rs
use tikv_jemallocator::Jemalloc;

#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

// jemalloc 优势：
// - 多线程性能更好（per-thread arena）
// - 碎片更少（更好的 small object 处理）
// - 可观测性（统计、profiling）
```

### 自定义分配器用于调试

```rust
use std::alloc::{GlobalAlloc, Layout, System};

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // 记录分配次数和大小，用于发现意外的堆分配
        ATOMIC_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
}
```

## 7. 内存泄漏（即使 Rust 也可能）

### Rc 循环引用

```rust
use std::{cell::RefCell, rc::Rc};

struct Node {
    next: RefCell<Option<Rc<Node>>>,
}

let a = Rc::new(Node { next: RefCell::new(None) });
let b = Rc::new(Node { next: RefCell::new(None) });

*a.next.borrow_mut() = Some(b.clone());  // a → b
*b.next.borrow_mut() = Some(a.clone());  // b → a，循环引用！
// a 和 b 的引用计数都是 2，永远不会归零，内存泄漏
drop(a); drop(b);  // 计数变为 1，但 Node 不会被释放

// 解决方案：用 Weak<T> 打破循环
use std::rc::{Rc, Weak};
struct Node2 {
    parent: RefCell<Weak<Node2>>,       // 弱引用，不增加 strong count
    children: RefCell<Vec<Rc<Node2>>>,  // 强引用
}
```

### 'static 生命周期导致的"泄漏"

```rust
// 永远不释放的数据
use std::sync::Mutex;
static CACHE: Mutex<Vec<String>> = Mutex::new(Vec::new());
// 存入的数据永远不会释放（'static 生命周期）
// 如果是缓存这种场景，应该有淘汰策略
```

### Box::leak 故意泄漏

```rust
// 将 Box 转为 &'static mut，永远不释放
let static_ref: &'static mut [u8] = Box::leak(Box::new([0u8; 1024]));
// 适用场景：配置初始化，整个程序生命周期都需要的只读数据
// 不要在循环中 leak，否则真的会内存耗尽
```

## 8. 内存相关检查清单

- [ ] 大小未知的类型用 Box/Rc/Arc 包装（避免栈溢出）
- [ ] 固定大小数组放在栈，动态大小用 Vec
- [ ] Newtype 用 `#[repr(transparent)]` 消除开销
- [ ] 结构体字段按对齐大小降序排列
- [ ] 避免不必要的 clone（让调用者决定）
- [ ] Rc 循环引用用 Weak 打破
- [ ] Vec 预分配容量避免反复 realloc
- [ ] 评估是否需要 arena/pool allocator 减少 fragmentation
- [ ] 考虑 jemalloc/mimalloc 替代默认分配器
- [ ] 用 size_of_val / align_of 检查内存占用

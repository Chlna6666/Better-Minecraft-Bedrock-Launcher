# Unsafe Rust 与 FFI

> Unsafe 不是关闭 Rust 的安全保证，而是**手动承担编译器无法验证的责任**。80% 的 bug 出在 20% 的 unsafe 代码中——所以 unsafe 代码必须最小化、清晰隔离、严格审查。

## 1. Unsafe 能做什么

`unsafe` 块解锁**五个**超能力：

1. **解引用裸指针**（`*const T` / `*mut T`）
2. **调用 unsafe 函数**（包括 FFI）
3. **实现 unsafe trait**
4. **访问/修改可变 static**
5. **访问 union 字段**

**Unsafe 不能做什么：** 绕过借用检查、忽略 `'static`、绕过 trait 约束。这些是编译期的，unsafe 无法关闭。

## 2. 裸指针

### 创建与解引用

```rust
let x = 42;
let raw: *const i32 = &x;       // 安全：创建裸指针
let val = unsafe { *raw };       // unsafe：解引用

let mut y = 0;
let raw_mut: *mut i32 = &mut y;
unsafe { *raw_mut = 100; }
```

### 何时需要裸指针

```rust
// 1. FFI（与 C 交互）
extern "C" {
    fn abs(x: i32) -> i32;  // C 函数签名
}
let x = unsafe { abs(-5) };  // unsafe 调用

// 2. 性能关键路径（避免借用检查开销，罕见）
fn fast_swap(buf: &mut [u8], a: usize, b: usize) {
    unsafe {
        let pa = buf.as_ptr().add(a);
        let pb = buf.as_ptr().add(b);
        std::ptr::swap(pa, pb);
    }
}

// 3. 自引用结构（async 状态机内部）
// 4. 实现自定义容器（Vec, HashMap 内部都用 unsafe）
```

## 3. Unsafe 的不变量（Invariant）

写 unsafe 代码时，你必须手动保证这些不变量（编译器无法检查）：

### 3.1 引用有效性

```rust
// ✗ 错误：解引用悬垂指针
let raw: *const i32 = {
    let local = 42;
    &local
};  // local 已 drop，raw 悬垂
unsafe { println!("{}", *raw); }  // UB！

// ✓ 正确：保证数据生命周期
let data = Box::new(42);
let raw: *const i32 = &*data;
unsafe { println!("{}", *raw); }  // data 仍存活

// ✗ 错误：违反别名规则（aliasing rule）
let mut x = 0;
let r1 = &x;
let r2 = &mut x;
// 编译器禁止此操作，但用 unsafe 可以绕过：
let r1 = &x;
let raw = &x as *const i32;
let r2 = unsafe { &mut *(raw as *mut i32) };
// 现在同时有 &x 和 &mut x → UB！编译器优化会出错
```

### 3.2 对齐（Alignment）

```rust
// ✗ 错误：未对齐访问
let bytes: [u8; 4] = [0; 4];
let ptr = bytes.as_ptr() as *const u32;
unsafe { let v = *ptr; }  // 可能 UB（取决于地址对齐）

// ✓ 正确：检查对齐
unsafe {
    if (ptr as usize) % std::mem::align_of::<u32>() == 0 {
        let v = *ptr;
    }
}

// ✓ 更好：用 read_unaligned
unsafe {
    let v = ptr.read_unaligned();  // 处理未对齐读取
}

// ✓ 最佳：用 bytemuck / zerocopy 安全转换
```

### 3.3 初始化（Validity）

```rust
// ✗ 错误：读取未初始化内存
let v: i32 = unsafe {
    let raw: *mut i32 = std::mem::MaybeUninit::uninit().as_ptr();
    *raw  // UB！读到的是垃圾值
};

// ✓ 正确：MaybeUninit 显式管理初始化
let mut buf: MaybeUninit<[u8; 1024]> = MaybeUninit::uninit();
unsafe {
    // 写入初始化
    buf.as_mut_ptr().write_bytes(0, 1024);
    // 假设已初始化，读取
    let arr = buf.assume_init();
}

// ✗ 错误：enum 的无效 discriminant
let bits = 0xFFFF_FFFFu32;
let val = unsafe { std::mem::transmute::<u32, bool>(bits) };
// bool 只能是 0 或 1，0xFFFF_FFFF 是无效的 → UB
```

## 4. Send/Sync 的 unsafe impl

```rust
// 某些类型编译器无法自动推断 Send/Sync，需要手动 impl
use std::cell::UnsafeCell;
use std::marker::PhantomData;

// 自定义并发原语：内部用 UnsafeCell，手动保证线程安全
struct MyRwLock<T> {
    inner: UnsafeCell<T>,
    // ...
}

// SAFETY: 我们保证通过 lock()/unlock() 同步访问，满足线程安全
unsafe impl<T: Send> Send for MyRwLock<T> {}
// SAFETY: 通过 RwLock 协议保证 &MyRwLock<T> 可跨线程共享
unsafe impl<T: Send + Sync> Sync for MyRwLock<T> {}

// ⚠️ 这是危险的：错误的 impl 会导致数据竞争 UB
// 必须在注释中详细说明为何安全（SAFETY 注释）
```

### 不当 Send/Sync impl 的后果

```rust
// ❌ 危险：让 Rc 实现 Sync
struct Bad(Rc<i32>);
unsafe impl Sync for Bad {}  // ❌ 错误！

// 现在 Bad 可以跨线程共享，但 Rc 不是线程安全的
// 多个线程同时 clone → 数据竞争 → 内存损坏
```

## 5. Unsafe 函数与 API 设计

### 最小化 unsafe 边界

```rust
// ❌ 差：整个函数 unsafe，调用者承担所有责任
pub unsafe fn process(buf: *mut u8, len: usize) {
    // 大量代码
}

// ✓ 好：unsafe 限制在最小范围，封装为安全 API
pub fn process(buf: &mut [u8]) {
    // 内部用 unsafe，但对外提供安全接口
    let ptr = buf.as_mut_ptr();
    let len = buf.len();
    unsafe { raw_process(ptr, len) }  // 安全，因为 &mut [u8] 保证有效
}

// ✓ 更好：用 safety doc 说明前置条件
/// # Safety
/// - `ptr` 必须有效，指向至少 `len` 个字节的内存
/// - 调用期间内存不能被其他引用访问
pub unsafe fn raw_process(ptr: *mut u8, len: usize) { /* ... */ }
```

### Safety 注释规范

```rust
// 每个 unsafe 块必须有 SAFETY 注释
fn example(slice: &mut [u8], idx: usize) {
    // SAFETY: idx 由调用者保证 < slice.len()，as_ptr().add(idx) 有效
    unsafe {
        let p = slice.as_ptr().add(idx);
        println!("{}", *p);
    }
}

// 每个unsafe 函数必须有 # Safety 文档
/// # Safety
/// 调用者必须保证 `ptr` 非空且对齐到 `align_of::<T>()`，
/// 且指向的内存已正确初始化为 T。
pub unsafe fn read_value<T>(ptr: *const T) -> T {
    ptr.read()
}
```

## 6. FFI（外部函数接口）

### 调用 C 函数

```rust
// 声明外部 C 函数
extern "C" {
    fn abs(input: i32) -> i32;
    fn strlen(s: *const c_char) -> usize;
}

fn main() {
    let x = unsafe { abs(-42) };  // 调用需要 unsafe
    println!("abs(-42) = {x}");
}
```

### 暴露 Rust 函数给 C

```rust
// extern "C" 让 Rust 函数有 C ABI
#[no_mangle]  // 防止名字修饰（mangling），保留原名
pub extern "C" fn rust_add(a: i32, b: i32) -> i32 {
    a + b
}

// C 侧声明：
// int rust_add(int a, int b);
```

### 类型映射

```rust
use std::os::raw::{c_int, c_char, c_void, c_ulong};

// Rust 类型与 C 类型的映射
// Rust       C
// ----       -
// i32        int        (c_int)
// i64        long       (c_long)
// u32        unsigned   (c_uint)
// f64        double
// *mut T     T*
// *const T   const T*
// *mut c_void  void*
// bool 不能直接映射到 C（C 的 _Bool 与 Rust bool 大小可能不同）

extern "C" {
    fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
}
```

### 字符串传递

```rust
use std::ffi::{CString, CStr, c_char};

// Rust String → C 字符串（需要 CString，确保以 null 结尾）
let rust_str = "hello";
let c_string = CString::new(rust_str).unwrap();
let raw: *const c_char = c_string.as_ptr();  // 传给 C
unsafe { c_function(raw); }
// c_string 必须保持存活直到 C 用完

// C 字符串 → Rust String
extern "C" { fn getenv(name: *const c_char) -> *const c_char; }
let name = CString::new("PATH").unwrap();
unsafe {
    let val_ptr = getenv(name.as_ptr());
    if !val_ptr.is_null() {
        let val = CStr::from_ptr(val_ptr);
        let rust_string = val.to_string_lossy().into_owned();
        println!("PATH = {rust_string}");
    }
}
```

### 回调函数

```rust
// C 库期望一个回调函数
extern "C" {
    // 参数是函数指针
    fn qsort(
        base: *mut c_void,
        nmemb: usize,
        size: usize,
        compar: extern "C" fn(*const c_void, *const c_void) -> c_int,
    );
}

// 提供给 C 的回调函数必须 extern "C"
#[no_mangle]
extern "C" fn compare_ints(a: *const c_void, b: *const c_void) -> c_int {
    let a = unsafe { *(a as *const i32) };
    let b = unsafe { *(b as *const i32) };
    (a - b).signum() as c_int
}

fn main() {
    let mut arr = [5, 2, 8, 1, 9];
    unsafe {
        qsort(
            arr.as_mut_ptr() as *mut c_void,
            arr.len(),
            std::mem::size_of::<i32>(),
            compare_ints,
        );
    }
}
```

## 7. 常见 unsafe 陷阱

### 1. 从引用构造多个可变引用

```rust
// ❌ 错误：从 &T 构造 &mut T
fn evil(r: &i32) -> &mut i32 {
    unsafe { &mut *(r as *const i32 as *mut i32) }
}
// 这违反了 Rust 的别名规则
// 编译器假设 &T 不会被修改，会做优化导致 UB
```

### 2. 错误的 transmute

```rust
// transmute 是最危险的 unsafe 操作
use std::mem::transmute;

// ❌ 错误：大小不匹配
let x: u8 = 42;
let y: u32 = unsafe { transmute(x) };  // UB：读了 3 字节未初始化内存

// ❌ 错误：类型语义不匹配
let ptr: *const u8 = &42;
let num: usize = unsafe { transmute(ptr) };  // 虽然可行但不推荐
// 用 cast 更明确：ptr as usize
```

### 3. 未定义行为（UB）

```rust
// UB 一旦发生，整个程序行为未定义
// 编译器可能基于"不会发生 UB"做优化，导致诡异结果

// 常见 UB：
// 1. 解引用空指针/悬垂指针
// 2. 使用未初始化内存
// 3. 数据竞争（多线程同时读写，至少一个写）
// 4. 违反别名规则
// 5. 无效的 enum 值
// 6. 整数溢出（debug 模式 panic，release 模式 wrapping，但依赖其行为是 UB）
// 7. 调用 ABI 不匹配的函数
```

### 4. 错误的内存布局假设

```rust
// ❌ 假设结构体字段顺序
#[repr(Rust)]  // 默认，编译器可重排
struct S { a: u8, b: u32 }

// ❌ 假设 sizeof == sum of fields
// 实际有 padding，不能直接 memcpy 到 buffer

// ✓ 用 #[repr(C)] 明确布局
#[repr(C)]
struct Header {
    magic: u32,
    version: u16,
    flags: u16,
}

// ✓ 用 bytemuck::Pod 安全做 zero-copy
// 需要 #[derive(bytemuck::Pod, bytemuck::Zeroable)]
```

## 8. 减少 unsafe 的策略

### 1. 用安全包装器

```rust
// ❌ 直接用裸指针
unsafe { /* 复杂裸指针操作 */ }

// ✓ 用 Vec, slice, Box 等安全抽象
// 标准库已经封装了大部分 unsafe
```

### 2. 用第三方安全库

```rust
// 需要零拷贝解析？用 zerocopy（安全）
use zerocopy::FromBytes;
let bytes = &[0u8; 4];
let num = i32::read_from(bytes).unwrap();

// 并发原语？用 crossbeam（经过形式化验证）
use crossbeam::atomic::AtomicCell;

// 原子操作？用 atomic 库
```

### 3. 模式：内部 unsafe

```rust
// 对外安全的 API，内部 unsafe
pub struct VecMap<K, V> {
    keys: Vec<K>,
    values: Vec<V>,
}

impl<K, V> VecMap<K, V> {
    pub fn get(&self, key: &K) -> Option<&V> where K: Eq {
        // 安全 API
        self.keys.iter().position(|k| k == key)
            .map(|i| &self.values[i])
    }

    // 私有 unsafe 辅助
    unsafe fn get_unchecked(&self, idx: usize) -> &V {
        self.values.get_unchecked(idx)
    }
}
```

## 9. 审查 unsafe 代码

### 审查清单

- [ ] 每个 `unsafe` 块有 SAFETY 注释说明为何安全？
- [ ] 每个 unsafe 函数有 `# Safety` 文档说明前置条件？
- [ ] Unsafe 范围最小化（不是整函数 unsafe）？
- [ ] 是否考虑了所有 UB 可能性（别名、对齐、初始化）？
- [ ] Unsafe 代码对外提供安全 API（不泄露 unsafe）？
- [ ] Send/Sync 的 unsafe impl 有充分理由？
- [ ] FFI 类型映射正确（用了 c_int/c_char 而非 i32/i8）？
- [ ] 字符串通过 CString/CStr 传递，避免非 null 结尾？
- [ ] 用 bytemuck/zerocopy 替代手写 transmute？
- [ ] 是否真的需要 unsafe？能否用安全抽象替代？

### 工具

```bash
# 静态分析 unsafe 代码
cargo install cargo-inspect    # 查看 HIR/MIR
cargo install miri             # 检测 UB（运行测试时）
MIRIFLAGS="-Zmiri-track-raw-pointers" cargo +nightly miri test

# Lint
cargo clippy -- -W clippy::multiple_unsafe_ops_per_block
# 警告单个 unsafe 块内多个 unsafe 操作（便于审查）
```

## 10. Unsafe 使用决策

**应该用 unsafe 的场景：**
- FFI（与 C 库交互）
- 实现底层容器（Vec, HashMap, channels）
- 极端性能优化（验证过 benchmark）
- 嵌入式系统（硬件寄存器访问）
- 操作系统开发

**不应该用 unsafe 的场景：**
- "绕过借用检查器"
- "我以为这样更快"（未验证）
- 避免重构（应该修复设计）
- 普通业务逻辑

**原则：** 每行 unsafe 代码都应该有充分理由，且被审查。

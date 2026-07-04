# 生命周期（Lifetimes）专题

> 生命周期是 Rust 最独特也最强大的特性——它在编译期验证所有引用的有效性，**消灭了悬垂指针、use-after-free、iterator invalidation 等整类 bug**。理解生命周期是掌握 Rust 的关键。

## 1. 生命周期是什么

### 本质：编译期的"作用域标签"

```rust
{
    let r;                      // ─┐
                                //  │ 'b: r 的作用域
    {                           //  │
        let x = 5;              //  │   ─┐
        r = &x;                 //  │    │ 'a: x 的作用域
    }                           //  │   ─┘ x 被 drop
                                //  │
    println!("r: {}", r);       //  │ ❌ r 引用的 x 已失效（悬垂引用）
}                               // ─┘
```

**编译器规则：** 引用的生命周期必须 ≥ 使用该引用的作用域。`r: &'b i32` 中 `'b` 必须 ≤ 被引用数据的作用域。

### 生命周期标注语法

```rust
// 'a 是生命周期参数，类似泛型参数 T
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() { x } else { y }
}
// 含义：返回值的生命周期 'a = x 和 y 生命周期的交集
// 编译器验证：调用方保证 x、y 在使用返回值期间都存活
```

**关键直觉：** 生命周期参数**不改变**数据的实际存活时间，它只是**标注引用之间的关系**，让编译器验证安全性。

## 2. 生命周期省略规则（Elision Rules）

好消息：90% 的情况你不需要手写生命周期，编译器会自动推断。

### 三条省略规则

```rust
// 规则 1：每个引用参数自动获得独立的生命周期
fn foo(x: &i32, y: &i32) { ... }
// 等价于：fn foo<'a, 'b>(x: &'a i32, y: &'b i32)

// 规则 2：只有一个输入生命周期时，输出生命周期 = 输入
fn foo(x: &i32) -> &i32 { ... }
// 等价于：fn foo<'a>(x: &'a i32) -> &'a i32

// 规则 3：方法（有 &self/&mut self）中，输出生命周期 = self
fn foo(&self, x: &str) -> &str { ... }
// 等价于：fn foo<'a, 'b>(&'a self, x: &'b str) -> &'a str
```

### 何时必须手写

```rust
// 多个输入引用 + 输出引用 → 必须明确关系
fn longest(x: &str, y: &str) -> &str { ... }
// ❌ 编译错误：无法推断输出生命周期是 x 还是 y 的

fn longest<'a>(x: &'a str, y: &'a str) -> &'a str { ... }  // ✓
// 取 'a = 交集，保证安全
```

## 3. 'static 生命周期

### 含义：数据在程序的整个运行期间都有效

```rust
// 'static 引用：可以永远安全使用
fn foo() -> &'static str {
    "hello"  // 字符串字面量编译进二进制，永远有效
}

// 'static 不只是引用，owned 数据通过 Box::leak 也能成为 'static
let leaked: &'static mut [u8] = Box::leak(Box::new([0u8; 1024]));
```

### 'static 的常见误解

```rust
// ❌ 误解：'static 意味着"程序结束才释放"
// 实际：'static 只是"可以安全地持有到程序结束"，不代表必须持有这么久

// 'static 约束的两种含义：
// 1. 引用 'static：数据真的永远存活（字面量、leaked）
// 2. T: 'static：类型 T 不含任何非 'static 的引用
//    （String: 'static ✓，&'a str: 'static 仅当 'a = 'static）

fn spawn_task<T: Send + 'static>(task: T) { ... }
// T: 'static 表示 task 不含短生命周期借用，可以安全移到新线程
```

### 哪些类型自动是 'static

```rust
// 所有 owned 类型都是 'static（不借用外部数据）
String: 'static      // ✓
Vec<u8>: 'static     // ✓
i32: 'static         // ✓（Copy 类型）
Box<i32>: 'static    // ✓（owned 堆数据）

// 含引用的类型取决于引用的生命周期
&'static str: 'static        // ✓
&'a str (a ≠ 'static): 'static  // ❌
struct Ref<'a, T> { r: &'a T }   // Ref<'a, T>: 'static 仅当 'a = 'static
```

## 4. 结构体中的生命周期

### 结构体持有引用——必须标注

```rust
// 结构体含引用 → 必须声明生命周期参数
struct TextEditor<'a> {
    content: &'a str,  // 借用外部 content，不能比它活得久
}

let content = String::from("hello");
let editor = TextEditor { content: &content };
// editor 借用 content，content 必须 ≥ editor 活着
// ❌ drop(content) 在 editor 之前 → 编译错误
```

### 何时结构体该用引用 vs owned

```rust
// ❌ 通常：结构体长期持有数据，用 owned 更简单
struct BadConfig<'a> {
    name: &'a str,  // 限制了 Config 必须短于原始数据
}
// 调用方烦恼：必须持有原始数据

// ✓ 推荐：结构体 owned 自己的数据
struct Config {
    name: String,
}

// ✓ 借用合理场景：临时视图、零拷贝解析
struct Parser<'a> {
    input: &'a [u8],  // 解析期间借用，不拷贝
    pos: usize,
}
// 短生命周期、明确的作用域
```

### 自引用结构体（编译器禁止）

```rust
// ❌ 自引用：结构体字段引用自己的另一字段
struct SelfRef {
    data: String,
    ptr: &str,  // 指向 data 内部？编译器无法表达
}

// 解决方案：
// 1. 用 owning crate（运行时检查）
use owning_ref::OwningRef;
let or = OwningRef::new(Box::new(String::from("hello")));
let or = or.map(|s| &s[..3]);  // 自引用，库保证安全

// 2. 用索引替代引用
struct SelfRefIdx {
    data: String,
    start: usize,  // 用索引而非引用
    end: usize,
}

// 3. 用 Pin（async 状态机内部使用）
```

## 5. 生命周期与多态

### 生命周期作为泛型参数

```rust
// 结构体可以泛型化生命周期
struct Container<'a, T> {
    data: &'a [T],
}

// 'a 可以是任意生命周期，'short 或 'long 都行
let v = vec![1, 2, 3];
let c1: Container<'_, i32> = Container { data: &v };  // 短
static V2: [i32; 3] = [4, 5, 6];
let c2: Container<'static, i32> = Container { data: &V2 };  // 长
```

### HRTB（Higher-Ranked Trait Bounds）

```rust
// for<'a> 表示"对所有生命周期 'a"
// 最常见：Fn trait
fn apply(f: impl Fn(&str) -> bool, s: &str) -> bool {
    f(s)
}
// 隐式展开为：impl for<'a> Fn(&'a str) -> bool
// 即：f 可以接受任意生命周期的 &str

// 显式 HRTB
fn store(f: Box<dyn for<'a> Fn(&'a str) -> &'a str>) { ... }
// f 接受任意 &str 输入，返回同样生命周期的 &str

// HRTB 常用于：解析器、迭代器、回调函数
```

## 6. 型变（Variance）

型变描述复合类型的生命周期如何与子类型化（subtyping）交互。这是 Rust 最难的概念之一。

### 三种型变

```rust
// 子类型化：'static 是 'a 的子类型（'static: 'a）
// 因为 'static 比 'a 长，能用在所有 'a 的地方

// 1. 协变（covariant）：跟随方向
//    &'a T 是协变的：'long 可以替代 'short
fn covariant<'a>(x: &'a str) {
    let s: &'static str = "hi";
    let _: &'a str = s;  // ✓ 'static 可作为 &'a str
}

// 2. 逆变（contravariant）：反向
//    fn(T) 是逆变的
fn contravariant(f: fn(&'static str)) {
    let g: fn(&'short str) = |_| ();
    let _: fn(&'static str) = g;  // ✓ 短输入的函数可用于长输入
}

// 3. 不变（invariant）：严格相等
//    &mut T 是不变的
fn invariant<'a>(x: &mut &'a str) {
    let s: &'static str = "hi";
    // *x = s;  // ❌ 即使 'static 更长也不行
    // 因为可变引用可能被写入短生命周期数据
}
```

### 为什么需要理解型变

```rust
// 经典陷阱：为什么 &mut &'a T 不能接受 'static
fn extend_lifetime<'a>(x: &mut &'a str) {
    let s: &'static str = "hi";
    *x = s;  // ❌ 不变，禁止
}

// 原因：如果允许，会发生 use-after-free
fn evil() {
    let mut short_ref: &str;
    {
        let local = String::from("temp");
        short_ref = &local;
        extend_lifetime(&mut short_ref);  // 如果允许...
        // short_ref 现在指向 "hi"（'static）
    }  // local drop
    // 但如果 extend 内部把 short_ref 改成 &local 再传出来 → 悬垂
}
// 所以 &mut T 是不变的，阻止此类 bug
```

### 型变总结表

| 类型 | 对 T 的型变 | 对生命周期的型变 |
|------|-----------|----------------|
| `&'a T` | 协变 | 协变 |
| `&'a mut T` | 不变 | 协变 |
| `Box<T>` / `Vec<T>` | 协变 | — |
| `Cell<T>` / `RefCell<T>` | 不变 | — |
| `fn(T) -> U` | T 逆变，U 协变 | — |
| `*const T` | 协变 | — |
| `*mut T` | 不变 | — |

**实践意义：** 大多数时候你不需要主动思考型变。但当遇到"为什么编译器拒绝这段看起来安全的代码"时，型变往往是答案。

## 7. 生命周期与数据结构设计

### 7.1 避免生命周期污染

```rust
// ❌ 借用扩散：一个借用字段让整个结构体被生命周期污染
struct App<'a> {
    config: &'a Config,  // 借用 Config
    db: Database,
    cache: Cache,
}
// App 现在被 'a 污染，所有持有 App 的地方都要处理 'a

// ✓ 方案 1：clone 出 owned
struct App {
    config: Config,  // owned
    db: Database,
    cache: Cache,
}

// ✓ 方案 2：Arc 共享
struct App {
    config: Arc<Config>,  // 共享 owned
}

// ✓ 方案 3：运行时借用（Rental/owning_ref）
```

### 7.2 Arena 分配器模式

```rust
// Arena 模式：所有数据共享同一生命周期，简化借用
use bumpalo::Bump;

struct Parser<'a> {
    arena: &'a Bump,
    tokens: Vec<&'a str>,  // 所有 token 都在 arena 里
}

impl<'a> Parser<'a> {
    fn parse(&mut self, input: &'a str) -> &'a Node<'a> {
        // 所有分配都在 arena，生命周期统一
        self.arena.alloc(Node {
            children: vec![self.arena.alloc(Node { ... })],
        })
    }
}
// arena drop 时所有数据一起释放
```

### 7.3 内部可变性与生命周期

```rust
use std::cell::RefCell;

// RefCell 让不可变引用可以修改数据
struct Graph {
    nodes: RefCell<Vec<Node>>,
}

impl Graph {
    fn add_node(&self) {  // 注意：&self 不是 &mut self
        self.nodes.borrow_mut().push(Node { ... });
    }
}
// 借用检查移到运行时，生命周期更灵活
```

## 8. 常见编译错误与修复

### 错误 1：返回局部变量的引用

```rust
// ❌
fn get_value() -> &str {
    let s = String::from("hi");
    &s  // s 在函数结束被 drop，悬垂引用
}

// ✓ 修复 1：返回 owned
fn get_value() -> String {
    String::from("hi")
}

// ✓ 修复 2：返回 'static
fn get_value() -> &'static str {
    "hi"
}

// ✓ 修复 3：传入 buffer
fn get_value(buf: &mut String) {
    buf.push_str("hi");
}
```

### 错误 2：借用时间过长

```rust
// ❌
let mut v = vec![1, 2, 3];
let first = &v[0];  // 借用 v
v.push(4);          // ❌ 不能在借用期间修改 v
println!("{first}");

// ✓ 修复：缩短借用范围
let mut v = vec![1, 2, 3];
{
    let first = &v[0];
    println!("{first}");
}  // 借用结束
v.push(4);  // ✓
```

### 错误 3：迭代器失效

```rust
// ❌
let mut v = vec![1, 2, 3];
for x in &v {
    if *x == 2 { v.push(4); }  // ❌ 借用 v 期间修改
}

// ✓ 修复：先收集要做的操作
let mut v = vec![1, 2, 3];
let mut to_add = vec![];
for x in &v {
    if *x == 2 { to_add.push(4); }
}
v.extend(to_add);
```

### 错误 4：生命周期不匹配

```rust
// ❌
fn first_word<'a>(s: &'a str) -> &'a str {
    let local = String::from(s);
    &local  // ❌ local 比 'a 短
}

// ✓ 修复：直接操作输入
fn first_word(s: &str) -> &str {
    // 省略规则自动推断生命周期
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b' ' { return &s[..i]; }
    }
    s
}
```

## 9. 生命周期检查清单

### API 设计
- [ ] 函数参数优先用省略规则（不写多余的生命周期）
- [ ] 多个输入引用 + 输出引用时明确关系
- [ ] 短期临时视图可用借用，长期持有用 owned
- [ ] 跨线程/异步任务要 `'static`

### 结构体
- [ ] 结构体含引用 → 标注生命周期参数
- [ ] 评估是否真的需要借用（owned/Arc 通常更简单）
- [ ] 自引用用索引/owning_ref/Pin 解决
- [ ] 避免生命周期污染（一个借用字段影响整个结构体）

### 编译错误排查
- [ ] 返回值是否引用了局部变量？
- [ ] 借用是否跨越了修改操作？
- [ ] 迭代期间是否修改了容器？
- [ ] 生命周期参数关系是否明确？

### 高级
- [ ] 跨函数传递闭包时考虑 HRTB（`for<'a>`）
- [ ] 理解 `&mut T` 的不变性（防止 use-after-free）
- [ ] `'static` 约束意味着"无短生命周期借用"，不是"永不释放"
- [ ] 用 arena 简化复杂生命周期场景

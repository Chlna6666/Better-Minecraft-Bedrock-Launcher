# 零成本抽象

> Rust 的核心设计哲学：**你能用高级抽象（迭代器、泛型、trait），编译后的机器码与你手写的低级代码一样快。** 不同于 C++ 模板的"几乎零成本"，Rust 通过单态化 + 内联 + 优化在编译期完成所有抽象展开。

## 1. 什么是零成本

### 零成本的两个含义（Bjarne Stroustrup 的定义）

1. **你不用的，你不用付钱**（What you don't use, you don't pay for）
2. **你用的，你不能再手写更好**（What you do use, you couldn't write better by hand）

### 示例：迭代器 vs 手写循环

```rust
// 高级抽象
let sum: i64 = data.iter().filter(|&&x| x > 0).map(|&x| x * 2).sum();

// 手写低级循环
let mut sum: i64 = 0;
for i in 0..data.len() {
    let x = data[i];
    if x > 0 {
        sum += x * 2;
    }
}

// 编译后两者生成的机器码相同！（LLVM 内联 + 优化）
// 这就是零成本抽象——你用了迭代器，但没有运行时开销
```

## 2. 单态化（Monomorphization）

### 泛型如何变零成本

```rust
// 泛型函数
fn max<T: Ord>(a: T, b: T) -> T {
    if a > b { a } else { b }
}

// 调用
let x = max(1, 2);          // i32 版本
let y = max(1.0, 2.0);      // f64 版本
let z = max('a', 'b');      // char 版本

// 编译器为每个具体类型生成一份代码（单态化）
fn max_i32(a: i32, b: i32) -> i32 { if a > b { a } else { b } }
fn max_f64(a: f64, b: f64) -> f64 { if a > b { a } else { b } }
fn max_char(a: char, b: char) -> char { if a > b { a } else { b } }

// 结果：
// - 无虚函数调用（直接内联）
// - 类型已知，优化器完全可见
// - 但二进制体积变大（每个类型一份代码）
```

### 泛型 vs trait object

```rust
// 泛型（静态分发，零成本）
fn process<T: Handler>(handler: &T) {
    handler.handle();  // 编译期内联，无运行时开销
}

// trait object（动态分发，有开销）
fn process_dyn(handler: &dyn Handler) {
    handler.handle();  // 通过 vtable 间接调用，无法内联
}

// 性能对比：
// - 泛型：直接调用，可内联，最快
// - dyn：一次间接跳转 + 无法内联，慢 5-20%
```

### 单态化的代价：二进制膨胀

```rust
// 如果泛型函数被很多类型实例化，二进制会膨胀
fn process<T>(x: T) { /* 100 行代码 */ }

// 实例化 100 种类型 → 二进制中 100 份代码
// 解决：用 trait object 减少实例化（牺牲一点速度换体积）
fn process_dyn(x: &dyn Trait) { /* ... */ }
```

## 3. 动态分发 vs 静态分发

### 静态分发（Static Dispatch）

```rust
trait Animal {
    fn speak(&self);
}

struct Dog;
struct Cat;

impl Animal for Dog { fn speak(&self) { println!("woof"); } }
impl Animal for Cat { fn speak(&self) { println!("meow"); } }

// 泛型函数：静态分发
fn make_speak<T: Animal>(a: &T) {
    a.speak();  // 编译期已知类型，直接调用 + 内联
}

fn main() {
    make_speak(&Dog);  // 编译为 make_speak_dog，speak 被内联
    make_speak(&Cat);  // 编译为 make_speak_cat，speak 被内联
}
```

### 动态分发（Dynamic Dispatch）

```rust
// trait object：动态分发
fn make_speak_dyn(a: &dyn Animal) {
    a.speak();  // 运行时通过 vtable 查找
}

fn main() {
    let animals: Vec<Box<dyn Animal>> = vec![Box::new(Dog), Box::new(Cat)];
    for a in &animals {
        make_speak_dyn(a.as_ref());  // 通过 vtable 调用
    }
}
```

### vtable 的内存布局

```
Box<dyn Animal> = (data_ptr, vtable_ptr)

data_ptr → ┌──────────────┐
           │ Dog 实例数据  │
           └──────────────┘

vtable_ptr → ┌──────────────────┐
             | drop_in_place ptr|
             | size             |
             | alignment        |
             | speak fn ptr     | ← 调用 a.speak() 时跳转到此
             | (其他方法 ptr)    |
             └──────────────────┘

每次方法调用：
1. 从对象加载 vtable_ptr
2. 从 vtable 加载方法指针
3. 间接调用（无法被 CPU 分支预测完美预测）
4. 无法内联 → 错失优化机会
```

### 选择策略

| 场景 | 选择 | 原因 |
|------|------|------|
| 类型在编译期已知 | 泛型（静态分发） | 零开销，可内联 |
| 类型集合在编译期已知（有限种） | 枚举 + match | 比 trait object 更快 |
| 类型集合开放/插件化 | `dyn Trait` | 灵活性优先 |
| 存储异构集合 | `Box<dyn Trait>` | 必须动态分发 |
| 热点循环内调用 | 静态分发 | 避免间接调用开销 |

### 枚举分发（避免 trait object）

```rust
// 如果类型种类有限且已知，用枚举替代 trait object
enum Animal {
    Dog(Dog),
    Cat(Cat),
    Bird(Bird),
}

impl Animal {
    fn speak(&self) {
        match self {
            Animal::Dog(d) => d.speak(),
            Animal::Cat(c) => c.speak(),
            Animal::Bird(b) => b.speak(),
        }
    }
}

// 优势：
// - 无堆分配（不需要 Box）
// - 无 vtable（直接 match）
// - 编译器可能优化为跳转表
// - 数据局部性好（enum 本身在栈或内联在 Vec 中）

// 比 Vec<Box<dyn Animal>> 快得多
let zoo: Vec<Animal> = vec![
    Animal::Dog(Dog),
    Animal::Cat(Cat),
];
```

## 4. 内联（Inlining）

### 内联是零成本的关键

```rust
// 小函数会被自动内联
#[inline]
fn square(x: i32) -> i32 { x * x }

let areas: Vec<_> = sides.iter().map(|&s| square(s)).collect();
// 编译后 square 完全消失，变成：
let areas: Vec<_> = sides.iter().map(|&s| s * s).collect();
```

### 跨 crate 内联

```rust
// 默认情况，函数只在本 crate 内可内联
// 跨 crate 调用时，LLVM 看不到函数体，无法内联

// 解决：用 #[inline] 提示编译器
#[inline]
pub fn helper(x: u32) -> u32 {
    // 这个函数会在调用者 crate 中展开
    x.wrapping_mul(31)
}
```

### 何时手动加 #[inline]

```rust
// ❌ 不要随便加 #[inline]
// 编译器已经足够智能，多数情况自动决策

// ✓ 这些情况考虑：
// 1. 极小的函数（1-5 行），跨 crate 调用
#[inline]
pub fn is_empty(&self) -> bool { self.len == 0 }

// 2. 泛型函数（已经会被内联，但 #[inline] 加速编译）
#[inline]
fn map<T, U, F: Fn(T) -> U>(vec: Vec<T>, f: F) -> Vec<U> { /* ... */ }

// 3. 性能关键的热点路径（用 benchmark 验证）
// #[inline(always)] 强制内联，但可能导致代码膨胀
```

## 5. 迭代器零成本

### 迭代器适配器的组合

```rust
// 复杂的迭代器链
let result: u64 = (1..=100)
    .map(|x| x * x)              // 平方
    .filter(|x| x % 2 == 0)      // 只要偶数
    .take(10)                    // 取前 10 个
    .sum();                       // 求和

// 编译器将整个链融合为单个循环（loop fusion）
// 等价于：
let mut result = 0u64;
let mut count = 0;
for x in 1..=100 {
    let squared = x * x;
    if squared % 2 == 0 {
        result += squared;
        count += 1;
        if count == 10 { break; }
    }
}
// 没有中间 Vec，没有额外分配，单次遍历
```

### 自定义迭代器零成本

```rust
struct Counter { count: u32 }

impl Iterator for Counter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        self.count += 1;
        if self.count <= 5 { Some(self.count) } else { None }
    }
}

// 使用时零成本
let sum: u32 = Counter { count: 0 }
    .map(|x| x * 2)
    .filter(|x| x > 4)
    .sum();
// 整个链被内联、融合为高效循环
```

## 6. Trait 的零成本设计

### 关联类型 vs 泛型参数

```rust
// 泛型参数：每个具体类型生成一份代码（单态化）
trait Container<T> {
    fn get(&self) -> &T;
}
// impl Container<i32> for MyStruct → 一份代码
// impl Container<String> for MyStruct → 又一份代码

// 关联类型：一个 impl 只有一份代码
trait Container {
    type Item;
    fn get(&self) -> &Self::Item;
}
impl Container for MyStruct {
    type Item = i32;  // 一个 MyStruct 只能有一种 Item
    fn get(&self) -> &i32 { /* ... */ }
}
// 更少的单态化，更小的二进制
```

### 空类型零成本 PhantomData

```rust
use std::marker::PhantomData;

// PhantomData 占用 0 字节，但告诉编译器"我逻辑上拥有 T"
struct Validator<T> {
    rules: Vec<Rule>,
    _marker: PhantomData<T>,  // 0 字节，零成本
}

// 用 PhantomData 实现类型状态
struct Unvalidated;
struct Validated;

struct User<State> {
    name: String,
    _state: PhantomData<State>,  // 0 字节
}

let u: User<Unvalidated> = User { name: "Alice".into(), _state: PhantomData };
// fn validate(u: User<Unvalidated>) -> User<Validated>
// 编译期保证状态机正确，运行时零开销
```

### Seal trait（防止外部实现）

```rust
// 模块外无法实现这个 trait，保证 API 封闭
mod private { pub trait Sealed {} }

pub trait Public: private::Sealed {
    fn method(&self);
}

// 只有本 crate 的类型能 impl Public
impl private::Sealed for MyType {}
impl Public for MyType { /* ... */ }

// 外部 crate 不能 impl Public（无法 impl Sealed）
// 这是零成本的 API 设计工具
```

## 7. 编译期计算

### const fn

```rust
// const fn 在编译期可执行
const fn fibonacci(n: u32) -> u64 {
    if n < 2 { n as u64 } else { fibonacci(n-1) + fibonacci(n-2) }
}

const FIB_10: u64 = fibonacci(10);  // 编译期计算，运行时零开销

// Rust 1.61+ 支持更多 const fn（含循环、if let 等）
const fn build_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        table[i] = crc32_step(i as u8);
        i += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = build_table();  // 1024 字节在编译期生成
```

### const generics

```rust
// 编译期已知的数组大小作为泛型参数
fn dot_product<const N: usize>(a: &[f64; N], b: &[f64; N]) -> f64 {
    let mut sum = 0.0;
    for i in 0..N { sum += a[i] * b[i]; }
    sum
}

let r = dot_product(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
// 编译器知道 N=3，可能展开循环
```

## 8. 类型擦除与恢复

### trait object 的类型擦除

```rust
// Box<dyn Trait> 擦除了具体类型
let x: Box<dyn Display> = Box::new(42);
// 只剩 vtable，丢失了"这是 i32"的信息

// 想恢复类型需要 Any
use std::any::Any;
let x: Box<dyn Any> = Box::new(42);
if let Some(n) = x.downcast_ref::<i32>() {
    println!("是 i32: {n}");
}
// 但 Any 有 TypeId 比较开销，且 vtable 调用
```

## 9. 常见的"伪零成本"

### 1. 闭包不总是零成本

```rust
// 闭包捕获方式影响成本
let v = vec![1, 2, 3];

// 1. 不捕获：零成本（函数指针）
let f: fn() -> i32 = || 42;

// 2. 按引用捕获：相当于结构体含引用
let f = || println!("{v:?}");
// 等价于 struct Closure<'a> { v: &'a Vec<i32> }

// 3. 按值 move 捕获：相当于结构体拥有数据
let f = move || println!("{v:?}");
// 等价于 struct Closure { v: Vec<i32> }

// 4. 修改捕获：需要 mutable 借用
let mut count = 0;
let mut f = || { count += 1; };
// 等价于 struct Closure<'a> { count: &'a mut i32 }
```

### 2. Box<dyn Trait> 不是零成本

```rust
// ❌ 错误印象：trait object 是零成本
let v: Vec<Box<dyn Animal>> = vec![/* ... */];
// 实际成本：
// - 堆分配（每个 Box）
// - vtable 间接调用
// - 缓存不友好（指针跳转）

// ✓ 真正零成本：泛型
fn process_all<T: Animal>(animals: &[T]) {
    for a in animals { a.speak(); }  // 全部内联
}
```

### 3. format! 不是零成本

```rust
// format! 涉及堆分配
let s = format!("{} {}", a, b);  // 分配 String

// 零成本替代：write! 到预分配缓冲
use std::fmt::Write;
let mut buf = String::with_capacity(64);
write!(buf, "{} {}", a, b).unwrap();
// 或使用 itoa/ryu 等零分配数字格式化
```

## 10. 零成本抽象检查清单

- [ ] 泛型函数能在编译期内联（无 `dyn`）
- [ ] 类型集合有限时用枚举分发代替 trait object
- [ ] 跨 crate 的小函数加 `#[inline]`
- [ ] 编译期常量用 `const` / `const fn`
- [ ] 迭代器链不引入中间 collect
- [ ] PhantomData 用于零开销类型标记
- [ ] 关联类型优于泛型参数（减少单态化）
- [ ] 评估 trait object 是否真的必要（性能损失）
- [ ] 热点路径避免 `format!`，用 `write!` 或专用库
- [ ] 用 `cargo bench` 验证抽象确实零成本

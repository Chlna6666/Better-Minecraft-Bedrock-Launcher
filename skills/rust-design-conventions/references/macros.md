# 宏系统（Macros）

> 宏是 Rust 的"代码生成代码"机制。与 C/C++ 的文本替换宏完全不同——Rust 宏操作的是**抽象语法树（AST）**，类型安全、卫生（hygienic）、无副作用。

## 1. 两种宏

| 类型 | 定义方式 | 何时展开 | 复杂度 | 用途 |
|------|---------|---------|--------|------|
| **macro_rules! 声明宏** | `macro_rules!` | 编译期，模式匹配 | 低 | `vec!`, `println!`, 自定义 DSL |
| **过程宏（proc macro）** | 独立 crate + `#[proc_macro]` | 编译期，操作 TokenStream | 高 | derive、属性、函数式宏 |

## 2. 声明宏（macro_rules!）

### 基本语法

```rust
macro_rules! vec_of_strings {
    // 匹配模式 => 展开
    () => { Vec::new() };
    ($($x:expr),*) => {{
        let mut v = Vec::new();
        $( v.push(String::from($x)); )*
        v
    }};
}

let v = vec_of_strings!("a", "b", "c");
// 等价于：
// let v = { let mut v = Vec::new(); v.push(String::from("a")); v.push(String::from("b")); v.push(String::from("c")); v };
```

### 匹配符（片段分类器）

| 标记 | 匹配内容 | 示例 |
|------|---------|------|
| `$x:expr` | 表达式 | `1+2`, `foo()` |
| `$x:ident` | 标识符 | `foo`, `bar` |
| `$x:ty` | 类型 | `i32`, `Vec<u8>` |
| `$x:tt` | 单个 token tree | 任意单个 token 或括号组 |
| `$x:item` | 完整 item | `fn`, `struct`, `impl` |
| `$x:pat` | 模式 | `Some(x)`, `_` |
| `$x:stmt` | 语句 | `let x = 1;` |
| `$x:block` | 块 | `{ ... }` |
| `$x:literal` | 字面量 | `42`, `"hi"`, `true` |
| `$x:meta` | 属性元数据 | `#[derive(Debug)]` 的内容 |
| `$x:lifetime` | 生命周期 | `'a`, `'static` |
| `$x:vis` | 可见性 | `pub`, `pub(crate)` |

### 重复语法

```rust
// $(...)+  匹配 1 次或多次
// $(...)*  匹配 0 次或多次
// 分隔符可以是 , ; => 等

macro_rules! sum {
    // 单个元素
    ($x:expr) => { $x };
    // 多个元素（逗号分隔）
    ($x:expr, $($rest:expr),*) => {
        $x + sum!($($rest),*)
    };
}

let s = sum!(1, 2, 3, 4);  // 10
```

### 标准库经典示例

```rust
// vec! 的简化实现
macro_rules! vec {
    () => { Vec::new() };
    ($elem:expr; $n:expr) => {
        ::std::vec::from_elem($elem, $n)  // vec![0; 100]
    };
    ($($x:expr),+ $(,)?) => {{
        let mut v = Vec::new();
        $( v.push($x); )+
        v
    }};
}

// println! 的简化实现
macro_rules! my_println {
    ($fmt:literal) => {
        println!($fmt)
    };
    ($fmt:literal, $($arg:tt)*) => {
        println!($fmt, $($arg)*)
    };
}
```

### 卫生性（Hygiene）

```rust
// 宏内的标识符不会污染外部作用域
macro_rules! using_x {
    () => {
        let x = 42;  // 宏内的 x
        println!("{}", x);
    };
}

fn main() {
    let x = "hello";  // 外部的 x
    using_x!();  // 打印 42，不影响外部 x
    println!("{}", x);  // 仍然是 "hello"
}
```

### 何时用声明宏

```rust
// ✓ 适合：
// 1. 减少重复的样板代码
macro_rules! impl_from {
    ($type:ty, $variant:ident) => {
        impl From<$type> for Error {
            fn from(e: $type) -> Self {
                Error::$variant(e)
            }
        }
    };
}
impl_from!(io::Error, Io);
impl_from!(serde_json::Error, Json);

// 2. 构建小型 DSL
let query = sql! { SELECT * FROM users WHERE active = true };
let html = html! { <div><p>Hello</p></div> };

// 3. 调试/日志（自动注入文件名行号）
debug!(var_name);  // 打印 "var_name = 42" at file:line
```

### 调试宏

```rust
// 用 trace_macros! 查看宏展开
trace_macros!(true);
vec![1, 2, 3];  // 打印每次宏展开
trace_macros!(false);

// 用 cargo expand 查看完全展开的代码
// cargo install cargo-expand
// cargo expand
```

## 3. 过程宏（Procedural Macros）

过程宏是接收 `TokenStream`、返回 `TokenStream` 的 Rust 函数。复杂但强大。

### 三种类型

| 类型 | 语法 | 作用 |
|------|------|------|
| **derive 宏** | `#[derive(MyMacro)]` | 为类型自动实现 trait |
| **属性宏** | `#[my_macro]` | 修改 item（函数、结构体等） |
| **函数式宏** | `my_macro!(...)` | 类似声明宏但能做任意计算 |

### 项目结构

过程宏**必须在独立的 crate** 中定义（crate type 为 `proc-macro`）：

```
my_macros/
├── Cargo.toml
└── src/
    └── lib.rs

# Cargo.toml
[lib]
proc-macro = true

[dependencies]
syn = { version = "2", features = ["full"] }  # 解析 Rust 代码为 AST
quote = "1"                                    # 把 AST 转回 TokenStream
proc-macro2 = "1"                              # 兼容层
```

### Derive 宏示例：自动实现 Builder

```rust
// src/lib.rs
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Fields};

#[proc_macro_derive(Builder, attributes(builder))]
pub fn derive_builder(input: TokenStream) -> TokenStream {
    // 1. 解析输入为 AST
    let ast = parse_macro_input!(input as DeriveInput);
    let name = &ast.ident;
    let builder_name = format!("{}Builder", name);
    let builder_ident = syn::Ident::new(&builder_name, name.span());

    // 2. 提取字段
    let fields = match &ast.data {
        Data::Struct(Data { fields: Fields::Named(named), .. }) => &named.named,
        _ => panic!("Builder only works on structs with named fields"),
    };

    let field_names: Vec<_> = fields.iter().map(|f| &f.ident).collect();
    let field_types: Vec<_> = fields.iter().map(|f| &f.ty).collect();

    // 3. 生成代码
    let expanded = quote! {
        pub struct #builder_ident {
            #(#field_names: Option<#field_types>),*
        }

        impl #builder_ident {
            pub fn new() -> Self {
                Self {
                    #(#field_names: None),*
                }
            }

            #(pub fn #field_names(mut self, val: #field_types) -> Self {
                self.#field_names = Some(val);
                self
            })*

            pub fn build(self) -> Result<#name, String> {
                Ok(#name {
                    #(#field_names: self.#field_names.ok_or_else(|| format!("missing {}", stringify!(#field_names)))?),*
                })
            }
        }

        impl #name {
            pub fn builder() -> #builder_ident {
                #builder_ident::new()
            }
        }
    };

    expanded.into()
}
```

使用：
```rust
#[derive(Builder)]
struct User {
    name: String,
    email: String,
    age: u32,
}

let user = User::builder()
    .name("Alice".into())
    .email("a@b.com".into())
    .age(30)
    .build()?;
```

### 属性宏示例：自动计时

```rust
#[proc_macro_attribute]
pub fn timed(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::ItemFn);
    let name = &input.sig.ident;
    let block = &input.block;
    let sig = &input.sig;
    let vis = &input.vis;

    let expanded = quote! {
        #vis #sig {
            let start = std::time::Instant::now();
            let result: #sig::Output = (|| #block)();
            let elapsed = start.elapsed();
            println!("{} took {:?}", stringify!(#name), elapsed);
            result
        }
    };
    expanded.into()
}

// 使用
#[timed]
fn expensive() -> u64 { /* ... */ }
```

### 函数式宏示例

```rust
#[proc_macro]
pub fn make_answer(_item: TokenStream) -> TokenStream {
    quote!({ 42 }).into()
}

// 使用
let x: i32 = make_answer!();
```

### 帮助属性（Helper Attributes）

```rust
#[proc_macro_derive(Builder, attributes(builder))]
//                                              ^^^^^^^^
// 注册 builder helper 属性

// 使用
#[derive(Builder)]
struct Query {
    #[builder(default = "1")]
    limit: u32,
    #[builder(each = "tag")]
    tags: Vec<String>,
}
```

## 4. 宏 vs 泛型 vs Trait

```rust
// 任务：实现通用的"打印 + 返回"操作

// 1. 泛型函数（最简单）
fn show<T: Display>(x: T) -> T {
    println!("{x}");
    x
}

// 2. Trait + blanket impl（类型安全的多态）
trait Show: Sized {
    fn show(self) -> Self where Self: Display {
        println!("{self}");
        self
    }
}
impl<T: Display> Show for T {}  // 所有 Display 都有 show

// 3. 宏（最灵活，但绕过类型检查）
macro_rules! show {
    ($x:expr) => {{
        let v = $x;
        println!("{v}");
        v
    }};
}
```

### 选择决策

```
需要多次求值同一参数？
  是 → 宏（泛型只求值一次）
  否 ↓

需要在不同类型上做相同操作？
  是 → 泛型函数或 trait
  否 ↓

需要生成新类型/函数定义？
  是 → 宏（derive 宏）
  否 ↓

需要 DSL（特定领域语言）？
  是 → macro_rules!
  否 ↓

需要编译期反射/复杂代码生成？
  是 → 过程宏
  否 ↓

默认：用普通函数 + 泛型
```

## 5. 何时不要用宏

```rust
// ❌ 滥用宏替代函数
macro_rules! add {
    ($a:expr, $b:expr) => { $a + $b };
}
// 问题：
// - 无类型检查：add!("a", 1) 编译通过但运行出错
// - 调试困难
// - IDE 支持差

// ✓ 用泛型函数
fn add<T: Add>(a: T, b: T) -> T::Output { a + b }
```

### 宏的缺点
- **可读性差**：复杂宏难理解
- **调试难**：错误信息指向展开后的代码，可能远离源码
- **无类型检查**：声明宏不验证表达式类型
- **编译慢**：复杂过程宏显著拖慢编译
- **IDE 弱**：跳转、补全、重构支持差

## 6. 宏的最佳实践

### 声明宏

```rust
// ✓ 1. 规则从特殊到一般
macro_rules! match_expr {
    // 特殊规则在前
    ("error") => { Err(()) };
    ("ok", $v:expr) => { Ok($v) };
    // 一般规则在后
    ($e:expr) => { $e };
}

// ✓ 2. 用 $(,)? 支持尾随逗号
macro_rules! my_vec {
    ($($x:expr),+ $(,)?) => { /* ... */ };
}
my_vec![1, 2, 3,];  // 尾随逗号 OK

// ✓ 3. 文档注释
/// 简短描述宏的作用。
///
/// # 示例
/// ```
/// my_macro!(arg);
/// ```
#[macro_export]
macro_rules! my_macro { /* ... */ }
```

### 过程宏

```rust
// ✓ 1. 用 syn 解析而非手写 TokenStream
// ✓ 2. 提供清晰的错误信息
use proc_macro_error::{abort, proc_macro_error};

#[proc_macro_derive(MyDerive)]
#[proc_macro_error]
pub fn derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    if !matches!(ast.data, Data::Struct(_)) {
        abort!(ast, "MyDerive only supports structs");
    }
    // ...
}

// ✓ 3. 用 #[automatically_derived] 标记生成代码（避免 clippy 警告）
let expanded = quote! {
    #[automatically_derived]
    impl #name { /* ... */ }
};
```

## 7. 宏系统检查清单

### 选择
- [ ] 能用函数 + 泛型解决就不用宏
- [ ] 需要代码生成（derive）才用过程宏
- [ ] 需要 DSL 或减少样板才用声明宏

### 声明宏
- [ ] 模式从特殊到一般排列
- [ ] 支持 `$(,)?` 尾随逗号
- [ ] 文档 + 示例
- [ ] 用 `cargo expand` 验证展开结果

### 过程宏
- [ ] 独立 crate + `proc-macro = true`
- [ ] 用 syn/quote/proc-macro2 三件套
- [ ] 清晰的错误信息（proc-macro-error）
- [ ] 标记 `#[automatically_derived]`

### 通用
- [ ] 不为绕过类型系统而用宏
- [ ] 宏保持小而专注
- [ ] 公共宏加 `#[macro_export]`
- [ ] 评估编译时间影响

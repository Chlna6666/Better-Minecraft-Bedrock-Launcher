# 命名规范

> 命名是软件工程中最难的两件事之一（另一件是缓存失效）。好的命名让代码自解释，差的命名让每个读者都要重新推理一遍意图。本规范基于 Rust 官方 [RFC 430](https://github.com/rust-lang/rfcs/blob/master/text/0430-finalizing-naming-conventions.md) 与 [API Guidelines](https://rust-lang.github.io/api-guidelines/naming.html)，并参考 Rust 语言圣经（Rust Course）。

## 1. 总体约定

### 核心原则：type-level 驼峰，value-level 蛇形

Rust 的根本命名哲学：**类型层面的构造用 `UpperCamelCase`，值层面的构造用 `snake_case`**。

| 类别 | 规则 | 示例 |
|------|------|------|
| Crate（包） | `snake_case`，不加 `-rs`/`-rust` 后缀 | `serde_json` ✓；`json-rs` ✗ |
| Module（模块） | `snake_case` | `user_service`, `http_client` |
| Type（struct/enum/trait） | `UpperCamelCase` | `HttpClient`, `OrderStatus` |
| 函数、方法 | `snake_case` | `find_by_id`, `total_price` |
| 通用构造器 | `new` 或 `with_more_details` | `new()`, `with_capacity()` |
| 转换构造器 | `from_some_other_type` | `from_utf8()` |
| 宏 | `snake_case!` | `vec!`, `println!` |
| 局部变量 | `snake_case` | `user_count`, `is_ready` |
| 静态变量、常量 | `SCREAMING_SNAKE_CASE` | `MAX_CONNECTIONS` |
| 类型参数（泛型） | `UpperCamelCase`，通常单大写字母 | `T`, `K`, `V`；描述性 `Item`, `Output` |
| 生命周期 | 短小写或语义化 | `'a`, `'de`, `'src` |
| Cargo feature | 不含占位词 | `std` ✓；`use-std`/`with-std` ✗ |

### 缩略词规则（RFC 430 关键细节，易错点）

```rust
// 驼峰命名中：缩略词当作一个单词，只首字母大写
struct Uuid;          // ✓ 而非 UUID
struct Usize;         // ✓ 而非 USize
struct Stdin;         // ✓ 而非 StdIn
struct HttpRequest;   // ✓ 而非 HTTPRequest
fn parse_url() {}     // ✓ 而非 parseURL

// 蛇形命名中：缩略词全小写
fn is_xid_start() {}  // ✓ 而非 is_XID_start
struct Config {
    http_client: Client,  // ✓ 字段 snake_case，不能写 httpClient
    api_key: String,      // ✓ 不能写 apiKey
}
```

### 蛇形命名单字母分段规则

```rust
// 除最后一部分外，其它部分不能由单个字母组成
// 文件名/模块名由类型名转 snake_case
struct BTreeMap;        // → 文件 btree_map.rs   ✓
//                       → b_tree_map.rs        ✗（B 是单字母分段）

const PI_2: f64 = 2.0 * std::f64::consts::PI;  // ✓ PI_2（最后部分可以是单字母/数字，前面不行）
//                                                ✗ PI2

// Crate 名不加 -rs 或 -rust 后缀（每个 crate 都是 Rust 写的，冗余）
// serde_json ✓
// serde_json_rs ✗
```

### mut 在命名中的体现

```rust
// 当返回类型含 mut 限定符时，命名应体现 mut 位置
impl<T> Vec<T> {
    fn as_mut_slice(&mut self) -> &mut [T];  // ✓ as_mut_slice
    // ❌ as_slice_mut  （mut 位置错误）
}

// getter 同理
impl S {
    pub fn first(&self) -> &First { &self.first }            // 不可变
    pub fn first_mut(&mut self) -> &mut First { &mut self.first }  // ✓ 加 _mut 后缀
    // ❌ get_first_mut / get_mut_first / mut_first
}
```

## 2. 函数命名规范

### 2.1 命名模式速查表

函数名应传达**动词 + 对象 +（可选）限定**。不同语义场景使用固定前缀：

| 场景 | 前缀/模式 | 返回类型 | 示例 |
|------|----------|---------|------|
| 布尔判断 | `is_` / `has_` / `can_` / `should_` | `bool` | `is_empty()`, `has_permission()` |
| 无损转换 | `as_` | `&T` 或 `&mut T` | `as_str()`, `as_bytes()` |
| 有损/分配转换 | `to_` | owned `T` | `to_string()`, `to_vec()` |
| 消费转换 | `into_` (方法) / `from_` (关联函数) | owned `T` | `into_string()`, `from_bytes()` |
| 构造 | `new` / `with_` / `from_` | `Self` | `new()`, `with_capacity()` |
| Builder | `with_` / `set_` | `Self` (消费) | `with_timeout()`, `set_retry()` |
| 获取（可能失败） | `get_` | `Option<T>` / `Result<T>` | `get_user()` |
| 获取（保证存在） | 无 `get_` 前缀 | `T` / `&T` | `name()`, `len()` |
| 迭代 | `iter` / `iter_mut` / `into_iter` | `impl Iterator` | `iter()`, `iter_mut()` |
| 校验 | `validate_` / `check_` | `Result` / `bool` | `validate_email()` |
| 解析 | `parse_` | `Result` | `parse_header()` |
| 序列化 | `serialize_` / `to_` | `Result` / `Vec<u8>` | `serialize_packet()` |

### 2.2 类型转换约定（C-CONV，RFC 430 重点）

类型转换的方法前缀与性能开销和所有权变化严格对应：

| 方法前缀 | 性能开销 | 所有权变化 | 含义 |
|---------|---------|-----------|------|
| `as_` | **Free**（零成本） | borrowed → borrowed | 借用转借用，无分配 |
| `to_` | **Expensive**（昂贵） | borrowed → borrowed（含校验）/ borrowed → owned（非 Copy）/ owned → owned（Copy 类型） | 涉及计算或分配 |
| `into_` | **Variable**（不定） | owned → owned（非 Copy 类型） | 消费 self 转换 |

#### 标准库权威示例

```rust
// as_：零成本，borrowed → borrowed
str::as_bytes(&self) -> &[u8]
//   把 str 变成 UTF-8 字节数组，性能开销为 0
//   输入借用 &str，输出借用 &[u8]

// to_：昂贵，borrowed → borrowed（但含校验/计算）
Path::to_str(&self) -> Option<&str>
//   执行一次昂贵的 UTF-8 字节检查
//   ⚠️ 输入输出都是借用，但开销大，所以用 to_ 而非 as_

// to_：昂贵，borrowed → owned（分配新内存）
str::to_lowercase(&self) -> String
//   遍历字符串字符，可能分配新 String

// into_：消费所有权
String::into_bytes(self) -> Vec<u8>
//   返回 String 底层 Vec<u8>，转换本身零消耗
//   但获取 String 所有权，返回独立所有权的 Vec<u8>
```

#### into_inner 约定

```rust
// 当一个值被某类型包装时，访问内部值用 into_inner()
// 适用于"单值包装"类型
BufReader::into_inner(self) -> R;        // 取回内部 reader
GzDecoder::into_inner(self) -> R;        // 取回内部 reader
AtomicBool::into_inner(self) -> bool;    // 取回内部 bool
```

### 2.3 Getter 命名（C-GETTER）

```rust
// 默认：Getter 不加 get_ 前缀
pub struct S {
    first: First,
    second: Second,
}

impl S {
    // ✓ 而非 get_first
    pub fn first(&self) -> &First { &self.first }

    // ✓ 而非 get_first_mut / get_mut_first / mut_first
    pub fn first_mut(&mut self) -> &mut First { &mut self.first }
}
```

#### 何时才用 `get_` 前缀（例外）

仅当**有且仅有一个值**能被 Getter 获取时，才使用 `get` 前缀：

```rust
// ✓ Cell::get 能直接访问到 Cell 中的唯一内容
impl<T: Copy> Cell<T> {
    fn get(&self) -> T;
}

// ✓ HashMap::get 通过 key 获取"那个"值
impl<K, V> HashMap<K, V> {
    fn get(&self, key: &K) -> Option<&V>;
}

// ❌ 普通字段访问加 get_ 是反模式
impl User {
    fn get_name(&self) -> &str { &self.name }  // 冗余，应为 name()
}
```

#### _unchecked 约定

```rust
// 当 get_ 含运行时检查时，可提供 _unchecked 变体换取性能
fn get(&self, index: K) -> Option<&V>;
fn get_mut(&mut self, index: K) -> Option<&mut V>;
unsafe fn get_unchecked(&self, index: K) -> &V;       // 跳过边界检查
unsafe fn get_unchecked_mut(&mut self, index: K) -> &mut V;
```

### 2.4 迭代器命名（C-ITER / C-ITER-TY）

#### 方法命名

```rust
// 同构集合的迭代器方法固定命名
fn iter(&self) -> Iter             // Iter: Iterator<Item = &U>
fn iter_mut(&mut self) -> IterMut  // IterMut: Iterator<Item = &mut U>
fn into_iter(self) -> IntoIter     // IntoIter: Iterator<Item = U>
```

#### 迭代器类型名与方法名匹配

```rust
// 迭代器类型名必须与产生它的方法名对应
Vec::iter()        -> Iter          // ✓
Vec::iter_mut()    -> IterMut       // ✓
Vec::into_iter()   -> IntoIter      // ✓
BTreeMap::keys()   -> Keys          // ✓
BTreeMap::values() -> Values        // ✓
```

#### 例外：非同构集合

```rust
// str 既是字节集合又是字符集合，不直接定义 iter，而用语义化方法名
str::bytes() -> Bytes        // 遍历字节
str::chars() -> Chars        // 遍历字符

// 函数（非方法）返回迭代器时用描述性名，不用 iter
// url crate: percent_encode() 函数返回 PercentEncode 迭代器
```

### 2.5 构造函数命名

```rust
// new：默认构造
impl Vec<T> { fn new() -> Self { ... } }

// with_：带参数构造（设置初始状态）
impl Vec<T> { fn with_capacity(cap: usize) -> Self { ... } }
impl Client { fn with_timeout(t: Duration) -> Self { ... } }

// from_：从其他类型转换构造
impl String { fn from_utf8(v: Vec<u8>) -> Result<Self> { ... } }

// default：Default trait 实现
impl Config { fn default() -> Self { ... } }

// parse_：从字符串解析
impl Url { fn parse(s: &str) -> Result<Self> { ... } }

// ❌ 避免：用 build/create/make 等同义词
impl User { fn build() -> Self { ... } }      // 用 new 或 builder()
impl User { fn create() -> Self { ... } }     // 用 new
impl User { fn make() -> Self { ... } }       // 用 new

// ✓ 多种构造方式时，名字描述"构造来源"
impl Color {
    fn from_rgb(r: u8, g: u8, b: u8) -> Self { ... }
    fn from_hex(hex: &str) -> Result<Self> { ... }
    fn random() -> Self { ... }
}
```

### 2.6 布尔函数命名

```rust
// ✓ 状态判断用 is_
fn is_empty(&self) -> bool { self.items.is_empty() }
fn is_active(&self) -> bool { self.status == Status::Active }

// ✓ 拥有用 has_
fn has_permission(&self, perm: Permission) -> bool { ... }

// ✓ 能力判断用 can_
fn can_write(&self) -> bool { !self.read_only }

// ✓ 意图判断用 should_
fn should_retry(&self, error: &Error) -> bool { ... }

// ❌ 用 check_ 返回 bool（check 暗示校验并可能报错）
fn check_permission(&self) -> bool { ... }  // 歧义：返回 bool 还是 Result？

// ✓ 校验用 validate_ 返回 Result
fn validate_email(email: &str) -> Result<Email, ValidationError> { ... }
```

### 2.7 函数命名完整示例

```rust
// ✓ 一个完整的 Service 命名示例
impl UserService {
    // 构造
    fn new(db: Db) -> Self { ... }
    fn with_cache(db: Db, cache: Cache) -> Self { ... }

    // 查询：可能失败用 get_
    fn get_user(&self, id: UserId) -> Result<User> { ... }
    fn get_users(&self, ids: &[UserId]) -> Result<Vec<User>> { ... }

    // 查询：保证存在的集合属性用复数名词
    fn active_users(&self) -> Vec<&User> { ... }

    // 迭代
    fn iter_users(&self) -> impl Iterator<Item = &User> + '_ { ... }

    // 布尔判断
    fn is_admin(&self, user: &User) -> bool { ... }
    fn has_permission(&self, user: &User, perm: Permission) -> bool { ... }

    // 校验（返回 Result）
    fn validate_email(&self, email: &str) -> Result<Email, ValidationError> { ... }

    // 状态变更
    fn create_user(&self, input: UserInput) -> Result<User> { ... }
    fn update_user(&self, id: UserId, patch: UserPatch) -> Result<User> { ... }
    fn delete_user(&self, id: UserId) -> Result<()> { ... }

    // 转换
    fn to_dto(&self, user: &User) -> UserDto { ... }  // 分配
    fn into_archive(self) -> ArchivedService { ... }  // 消费
}
```

## 3. 类型命名

### 3.1 Struct 与 Enum

```rust
// ✓ 名词或名词短语
struct User { ... }              // 名词
struct HttpClient { ... }        // 名词
struct ConnectionPool { ... }    // 名词

// ✓ Enum 用单数名词（表示"一个值可以是这些之一"）
enum HttpStatus { Ok, NotFound, ServerError }  // 单数 Status
enum OrderState { Pending, Paid, Shipped }      // 单数 State
enum Color { Red, Green, Blue }                  // 单数 Color

// ❌ Enum 用复数（暗示集合而非互斥）
enum HttpStatuses { ... }   // 应为 HttpStatus
enum OrderStates { ... }    // 应为 OrderState

// ✓ Enum 变体 UpperCamelCase
enum Message {
    Quit,                          // 无数据
    Move { x: i32, y: i32 },       // 命名字段
    Write(String),                 // 元组
    ChangeColor(Color),            // 嵌套
}

// ❌ Enum 变体用 SCREAMING_SNAKE（这是 C/Java 习惯，非 Rust）
enum Message {
    QUIT,  // ❌
    MOVE,  // ❌
}
```

### 3.2 错误类型词序一致性（C-WORD-ORDER）

```rust
// 标准库错误类型统一使用「谓语-宾语-错误」词序
JoinPathsError     // join + paths + error
ParseBoolError     // parse + bool + error
ParseCharError     // parse + char + error
ParseFloatError    // parse + float + error
ParseIntError      // parse + int + error
RecvTimeoutError   // recv + timeout + error
StripPrefixError   // strip + prefix + error

// ✓ 新错误类型遵循同词序
ParseAddrError     // ✓ 谓语(parse) + 宾语(addr) + error
// ❌ AddrParseError  // 词序错误

// 原则：可以选择合适的词序，但必须在包的范畴内保持一致
```

### 3.3 Trait 命名

```rust
// ✓ 行为 trait：优先用动词（RFC 430 推荐）
trait Read { ... }           // 动词 ✓（优于 Reader）
trait Iterator { ... }       // 名词（角色）
trait Clone { ... }          // 动词
trait Draw { ... }           // 动词 ✓（优于 Drawable）
trait Print { ... }          // 动词 ✓（优于 Printable）

// ✓ 转换 trait：用 From/Into 风格
trait From<T> { fn from(t: T) -> Self; }
trait Into<T> { fn into(self) -> T; }
trait TryFrom<T> { type Error; fn try_from(t: T) -> Result<Self, Self::Error>; }

// ✓ 抽象 trait：用名词描述角色
trait Repository<T> { ... }      // 仓储角色
trait EventBus { ... }           // 事件总线
trait Serializer { ... }         // 序列化器

// ❌ Trait 加 I 前缀（这是 Java/Go/C# 习惯，非 Rust）
trait IUserService { ... }    // ❌ 应为 UserService
trait IRepository { ... }     // ❌ 应为 Repository

// ❌ Trait 加 Trait 后缀
trait ReadTrait { ... }       // ❌ 应为 Read
trait CloneTrait { ... }      // ❌ 应为 Clone
```

### 3.4 Newtype 命名

```rust
// ✓ Newtype 包装用领域名词，不加 Wrapper 后缀
struct UserId(Uuid);              // ✓
struct Email(String);             // ✓
struct OrderId(u64);              // ✓

// ❌ 加 Wrapper 后缀（冗余）
struct UserIdWrapper(Uuid);       // ❌

// ✓ 标注 transparent 表示零开销
#[repr(transparent)]
struct UserId(Uuid);

// ✓ 带验证逻辑的 Newtype 构造
impl Email {
    fn parse(s: &str) -> Result<Self, EmailError> { ... }
}
```

### 3.5 错误类型命名

```rust
// ✓ 库错误用 Error 后缀
#[derive(Debug, thiserror::Error)]
pub enum ParseError { ... }       // ✓
pub enum DatabaseError { ... }    // ✓
pub struct ConfigError { ... }    // ✓

// ✓ 类型别名 Result 简化
pub type Result<T> = std::result::Result<T, ParseError>;

// ❌ 错误类型加 Exception（Java 习惯）
pub enum ParseException { ... }   // ❌

// ❌ 错误类型加 Failure 后缀
pub enum ParseFailure { ... }     // ❌
```

## 4. 模块与 Crate 命名

### 4.1 模块命名

```rust
// ✓ 模块名 snake_case，单数
mod user;            // ✓
mod order_service;   // ✓
mod http_client;     // ✓

// ❌ 复数
mod users;           // ❌ 应为 user
mod http_clients;    // ❌ 应为 http_client

// ✓ 模块文件名与模块名一致
// src/user_service.rs → mod user_service;

// ✓ 子模块用目录结构
mod parser;       // src/parser.rs 或 src/parser/mod.rs
mod parser::ast;  // src/parser/ast.rs

// ✓ 公共 API re-export 时用 self 或 as
pub mod parser;
pub use parser::ast;        // 直接暴露 ast
pub use parser::ast as p_ast; // 重命名（避免冲突）
```

### 4.2 Crate 命名

```toml
# Cargo.toml
# ✓ crate 名 snake_case，用连字符（crates.io 规范）
[package]
name = "http-client"       # ✓
name = "order_service"     # ✓
name = "serde_json"        # ✓

# ❌ 大写、下划线、驼峰
name = "HttpClient"         # ❌
name = "http_client"       # ❌ crates.io 用连字符（_ 会被转为 -）

# ❌ 加 -rs 或 -rust 后缀（每个 crate 都是 Rust 写的，冗余）
name = "json-rs"            # ❌ 应为 json
name = "parser-rust"        # ❌ 应为 parser
```

```rust
// ✓ 代码中引用 crate 时，连字符自动转为下划线
// Cargo.toml: name = "http-client"
// 代码中：use http_client::Client;  ← 自动转为下划线

// ✓ Crate 名应简短、描述性、可搜索
// 好：tokio, serde, reqwest, clap
// 差：my_utils, common_lib, helpers
```

## 5. 泛型参数与生命周期命名

### 5.1 泛型参数

```rust
// ✓ 单字母：T（Type）、E（Error）、K（Key）、V（Value）
struct HashMap<K, V> { ... }
fn parse<T: FromStr>(s: &str) -> Result<T, T::Err> { ... }

// ✓ 多字母描述性：当类型角色不明显时
trait Repository<T> {
    type Error;
    fn find_by_id(&self, id: &T::Id) -> Result<Option<T>, Self::Error>;
}

// ✓ 约定俗成的多字母泛型
struct Stream<Item> { ... }        // 迭代元素
struct Future<Output> { ... }     // 异步输出
struct HttpRequest<Body> { ... }  // HTTP 体

// ❌ 无意义的单字母
struct Container<X, Y, Z> { ... }  // X/Y/Z 是什么？

// ✓ 用描述性名字
struct Container<Front, Middle, Back> { ... }

// ✓ 类型参数顺序：生命周期在前，类型在后
struct Parser<'a, T> { input: &'a [T] }  // 'a 在 T 前
```

### 5.2 生命周期命名

```rust
// ✓ 默认用 'a（编译器省略时不必手写）
fn foo<'a>(x: &'a str) -> &'a str { ... }

// ✓ 多个生命周期时用 'a, 'b（短）
fn longest<'a, 'b>(x: &'a str, y: &'b str) -> &'a str { ... }

// ✓ 语义化生命周期：当关系复杂时，用描述性名字
struct Lexer<'src> {        // 'src = 源代码的生命周期
    source: &'src str,
}

struct Iterator<'buf> {      // 'buf = buffer 的生命周期
    buffer: &'buf [u8],
}

// ✓ 'static 是保留名，表示程序生命周期
fn get_config() -> &'static str { "production" }

// ❌ 无意义的多生命周期
fn foo<'a, 'b, 'c, 'd>(a: &'a str, b: &'b str, c: &'c str, d: &'d str)
// → 简化关系或用语义化名字
```

## 6. 变量命名

### 6.1 局部变量

```rust
// ✓ snake_case，描述性
let user_count = 0;
let is_valid = check(&input);
let file_path = "/etc/config";

// ✓ 短作用域可用短名
for i in 0..n { ... }              // 循环索引 i
let (k, v) = pair;                 // 解构
let (tx, rx) = channel();          // sender/receiver

// ❌ 过度缩写
let usr_cnt = 0;                   // 应为 user_count
let cfg = load_config();           // cfg 在短作用域 OK，长作用域应为 config

// ✓ 遮蔽（shadowing）转换类型时用同名
let raw: &str = "42";
let parsed: u32 = raw.parse().unwrap();  // 或直接 let raw = raw.parse()...
let raw = raw.parse::<u32>().unwrap();   // ✓ 遮蔽，表达"同概念的转换"
```

### 6.2 布尔变量

```rust
// ✓ is_/has_/should_ 前缀
let is_ready = check_status();
let has_errors = !errors.is_empty();
let should_retry = retry_count < MAX;

// ❌ 无前缀的布尔（歧义）
let ready = check_status();       // ready 是布尔还是状态对象？
let errors = !errors.is_empty();  // errors 是集合还是布尔？
```

### 6.3 集合变量

```rust
// ✓ 复数名词
let users = vec![...];
let active_users = users.iter().filter(|u| u.is_active()).collect();

// ✓ 计数用 _count 后缀
let user_count = users.len();
let error_count = errors.len();

// ✓ 单数 + 索引
for (index, user) in users.iter().enumerate() { ... }

// ❌ 用 _list/_array 后缀（冗余，类型已说明）
let user_list = vec![...];  // ❌ 应为 users
let user_array = [...];     // ❌ 应为 users
```

## 7. 常量与静态变量

```rust
// ✓ 常量 SCREAMING_SNAKE_CASE
const MAX_CONNECTIONS: u32 = 100;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const API_BASE_URL: &str = "https://api.example.com";

// ✓ 关联常量
struct User {
    const MAX_NAME_LEN: usize = 255;
}

// ✓ 静态变量
static COUNTER: AtomicUsize = AtomicUsize::new(0);
static CONFIG: LazyLock<Config> = LazyLock::new(|| load_config());

// ❌ 常量用 camelCase 或 snake_case
const maxConnections = 100;     // ❌
const max_connections = 100;     // ❌

// ❌ 常量加 CONST 前缀/后缀
const CONST_MAX_RETRY = 5;       // ❌
const MAX_RETRY_CONST = 5;      // ❌
```

## 8. Cargo Feature 命名（C-FEATURE）

```toml
# ✓ Feature 名直接用功能名，不含占位词
[features]
default = ["std"]
std = []                        # ✓ 而非 use-std / with-std
async = ["dep:tokio"]            # ✓ 而非 with-async / enable-async
tls = ["rustls"]                 # ✓ 而非 use-tls / with-tls

# ❌ 占位词前缀（无意义）
[features]
default = ["use-std"]            # ❌
use-tls = ["rustls"]             # ❌
with-async = ["dep:tokio"]       # ❌
```

```rust
// 对应代码中 #[cfg(feature = "std")]
#![cfg_attr(not(feature = "std"), no_std)]
// feature 名与 Cargo.toml 一致，不加修饰
```

## 9. 命名反模式汇总

```rust
// ❌ 1. 含糊动词
fn process(data: &Data) { ... }       // process 什么？
fn handle(x: &str) { ... }            // handle 什么？
fn do_it() { ... }                     // 做什么？

// ✓ 明确动词 + 对象
fn validate_order(data: &Order) { ... }
fn send_notification(message: &str) { ... }
fn calculate_total() { ... }

// ❌ 2. 误导性命名（命名与返回类型不一致）
fn get_user(&self) -> () { ... }       // get_ 但不返回值
fn is_valid(&self) -> Result<()> { ... } // is_ 但返回 Result 而非 bool
fn to_string(&self) -> &str { ... }    // to_ 但返回引用（应为 as_）

// ✓ 命名与返回类型一致
fn delete_user(&self) -> () { ... }              // void 操作
fn is_valid(&self) -> bool { ... }               // 布尔
fn validate(&self) -> Result<(), Error> { ... }  // Result
fn as_str(&self) -> &str { ... }                 // 借用

// ❌ 3. 缩写滥用
fn calc_avg(usr: &Usr) -> f64 { ... }   // calc, avg, usr 都该展开

// ✓ 例外：约定俗成的缩写
fn parse_url(url: &Url) { ... }         // URL 通用
fn send_http_req(req: HttpRequest) { ... } // req/res 通用
fn convert_to_csv(data: &[Row]) { ... }  // CSV 通用

// ❌ 4. 名词用作函数
fn user() -> User { ... }        // 名词当函数，应为 create_user 或 get_user
fn database() -> Db { ... }      // 同上

// ❌ 5. 动词用作类型
struct Process { ... }           // 应为 Processor 或 ProcessingContext
struct Handle { ... }            // 应为 Handler

// ❌ 6. 类型名重复模块名
mod user {
    struct User { ... }          // 冗余：user::User
    fn create_user() { ... }     // 冗余：user::create_user
}

// ✓ 省略模块名（路径已提供上下文）
mod user {
    struct Record { ... }        // user::Record
    fn create() -> Record { ... } // user::create()
}

// ❌ 7. 匈牙利命名法
let strName: String = ...        // ❌ 类型前缀（Rust 有类型系统）
let iCount: i32 = ...            // ❌

// ❌ 8. 否定命名
let is_not_empty = !items.is_empty();  // ❌ 双重否定
if !is_not_empty { ... }               // 读起来绕

// ✓ 肯定命名
let is_empty = items.is_empty();
let has_items = !items.is_empty();

// ❌ 9. 缩略词大小写错误（RFC 430 常见违反）
struct HTTPClient;   // ❌ 缩略词不应全大写
struct Uuid;          // ✓ 只首字母大写
fn is_XID_start() {}  // ❌ 蛇形中缩略词应全小写
fn is_xid_start() {}  // ✓

// ❌ 10. 蛇形单字母分段
mod b_tree_map;      // ❌ B 是单字母分段
mod btree_map;       // ✓
```

## 10. 命名规范检查清单

### 大小写
- [ ] 类型/trait 用 UpperCamelCase
- [ ] 函数/变量/模块用 snake_case
- [ ] 常量用 SCREAMING_SNAKE_CASE
- [ ] 文件名与模块名一致（snake_case）
- [ ] 缩略词：驼峰中只首字母大写（`Uuid` 非 `UUID`），蛇形中全小写（`is_xid_start`）
- [ ] 蛇形除最后部分外无单字母分段（`btree_map` 非 `b_tree_map`）

### 函数
- [ ] 布尔函数用 `is_`/`has_`/`can_`/`should_` 前缀
- [ ] 转换严格遵循：`as_`（零成本借用）/`to_`（昂贵或分配）/`into_`（消费）
- [ ] 单值包装取内部用 `into_inner()`
- [ ] Getter 默认不加 `get_`；仅"唯一值可获取"或"需 key"时才用 `get_`
- [ ] 返回 mut 的方法用 `_mut` 后缀（`as_mut_slice` 非 `as_slice_mut`）
- [ ] 迭代器方法：`iter`/`iter_mut`/`into_iter`，类型名匹配（`Iter`/`IterMut`/`IntoIter`）
- [ ] 构造用 `new`/`with_`/`from_`，不用 `build`/`create`/`make`
- [ ] 动词 + 对象，避免 `process`/`handle`/`do` 含糊词

### 类型
- [ ] Enum 用单数名词，变体 UpperCamelCase（非 SCREAMING）
- [ ] 错误类型词序一致（`ParseAddrError` 而非 `AddrParseError`，谓语-宾语-错误）
- [ ] Trait 优先用动词（`Draw` 优于 `Drawable`），不加 `I` 前缀或 `Trait` 后缀
- [ ] Newtype 不加 `Wrapper` 后缀
- [ ] 错误类型用 `Error` 后缀（非 `Exception`/`Failure`）

### 模块与 Crate
- [ ] 模块/crate 名 snake_case，单数
- [ ] crate 名用连字符（crates.io 规范）
- [ ] crate 名不加 `-rs`/`-rust` 后缀
- [ ] 避免模块名重复（`user::User` → `user::Record`）

### Cargo Feature
- [ ] Feature 名不含占位词（`std` 非 `use-std`/`with-std`）

### 泛型与生命周期
- [ ] 泛型用 `T`/`K`/`V` 或描述性 `Item`/`Output`
- [ ] 生命周期默认 `'a`，复杂场景用 `'src`/`'buf` 语义化
- [ ] 生命周期参数在类型参数前

### 变量
- [ ] 布尔变量用 `is_`/`has_` 前缀
- [ ] 集合用复数名词，计数用 `_count` 后缀
- [ ] 不用匈牙利命名法（`strName`）
- [ ] 不用否定命名（`is_not_empty` → `is_empty`）

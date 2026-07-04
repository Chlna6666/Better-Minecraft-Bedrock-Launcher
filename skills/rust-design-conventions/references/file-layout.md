# 文件布局与模块化

> 文件布局是 Rust 项目可维护性的骨架。臃肿的源码文件（单文件 1000+ 行、实现与测试混杂）是技术债的主要来源——阅读困难、导航混乱、合并冲突频发、重构成本高昂。本规范定义文件拆分阈值、模块化原则与目录组织约定，让每个文件**单一职责、体量适中、易于导航**。

## 1. 核心原则：单一职责 + 体量适中

一个文件应该：
- **只做一件事**：一个类型族、一组相关函数、一个领域的逻辑
- **体量适中**：实现文件目标 100-300 行，硬上限 500 行
- **可独立理解**：打开文件不需要先理解其他巨文件
- **职责清晰命名**：文件名能准确描述内容

### 反模式：上帝文件

```rust
// ❌ 反模式：单文件包揽一切
// src/service.rs  (1500 行)
//   ├─ User 相关 (400 行)
//   ├─ Order 相关 (500 行)
//   ├─ Payment 相关 (300 行)
//   └─ Notification 相关 (300 行)
//
// 问题：
// - 改 User 要在 1500 行里找
// - 改 Order 可能误碰 Payment
// - 多人同时改易冲突
// - 无法独立理解某一块
```

## 2. 文件拆分阈值

### 2.1 行数阈值

| 文件类型 | 建议上限 | 硬上限 | 触发动作 |
|---------|---------|--------|---------|
| 实现文件（`*.rs`） | 300 行 | 500 行 | 超过 300 行考虑拆分，超过 500 行**必须拆分** |
| 测试文件（`*_tests.rs`） | 200 行 | 400 行 | 超过 200 行按被测功能分组拆分 |
| 模块入口（`mod.rs`） | 50 行 | 100 行 | 只放 re-export 和模块文档，不放实现 |
| `lib.rs` / `main.rs` | 100 行 | 200 行 | `lib.rs` 只 re-export，`main.rs` 只编排 |
| 单个函数 | 30 行 | 50 行 | 超过 50 行拆分为子函数 |
| 单个 `impl` 块 | 200 行 | 400 行 | 方法过多时按功能分组拆 impl |

**注意：** 行数是参考信号，不是绝对规则。一个 400 行的纯数据定义（如查表）可接受；一个 200 行但逻辑纠缠的函数必须拆。

### 2.2 拆分信号

出现以下信号时，即使未达行数阈值也应拆分：

- **职责混杂**：一个文件里有多个不相关的类型族
- **改 A 必须读 B**：修改某功能必须理解文件其他部分
- **import 爆炸**：文件顶部 30+ 个 `use`，说明职责过广
- **命名困难**：无法用一句话描述"这个文件做什么"
- **测试难写**：为文件某部分写测试要 mock 文件其他部分

## 3. 拆分策略

### 3.1 按领域/类型族拆分

最常用的拆分维度。把相关的类型放一起，不相关的分开：

```
// ❌ 拆分前：所有模型堆一起
src/models.rs  (800 行)
  ├─ User (200 行)
  ├─ Order (300 行)
  ├─ Product (150 行)
  └─ Payment (150 行)

// ✓ 拆分后：按领域分文件
src/models/
  ├─ mod.rs           # re-export，<30 行
  ├─ user.rs          # ~200 行
  ├─ order.rs         # ~300 行
  ├─ product.rs       # ~150 行
  └─ payment.rs       # ~150 行
```

```rust
// src/models/mod.rs
mod user;
mod order;
mod product;
mod payment;

pub use user::{User, UserId};
pub use order::{Order, OrderId, OrderStatus};
pub use product::{Product, ProductId};
pub use payment::{Payment, PaymentMethod};
```

### 3.2 按职责层拆分

同一领域的不同职责层分开（领域驱动设计风格）：

```
src/
├── domain/           # 领域模型（纯逻辑，无 IO）
│   ├── mod.rs
│   ├── user.rs       # User 实体 + 业务规则
│   └── order.rs
├── repository/       # 数据访问层
│   ├── mod.rs
│   ├── user_repo.rs  # UserRepository trait + DbUserRepo
│   └── order_repo.rs
├── service/          # 应用服务层（编排）
│   ├── mod.rs
│   ├── user_service.rs
│   └── order_service.rs
└── api/              # 接口层（HTTP handler 等）
    ├── mod.rs
    ├── user_handler.rs
    └── order_handler.rs
```

### 3.3 按类型拆分大型类型

当一个类型的方法过多（如 50+ 方法），按功能分组拆 `impl`：

```
src/http/
├── mod.rs
├── client.rs            # HttpClient 结构体 + 核心方法
├── client_builder.rs    # impl HttpClientBuilder（构造相关）
├── client_request.rs    # impl HttpClient（请求相关方法）
└── client_auth.rs       # impl HttpClient（认证相关方法）
```

```rust
// src/http/client.rs
pub struct HttpClient { /* 字段 */ }
impl HttpClient {
    pub fn new() -> Self { /* ... */ }
}

// src/http/client_request.rs
use super::client::HttpClient;
impl HttpClient {
    pub async fn get(&self, url: &str) -> Result<Response> { /* ... */ }
    pub async fn post(&self, url: &str, body: Body) -> Result<Response> { /* ... */ }
}

// src/http/client_auth.rs
impl HttpClient {
    pub fn with_auth(mut self, token: &str) -> Self { /* ... */ }
}
```

### 3.4 按复杂度拆分

把"复杂逻辑"从"简单胶水代码"中分离：

```
src/parser/
├── mod.rs               # 编排：tokenize → parse → build_ast
├── lexer.rs             # 词法分析（复杂状态机）
├── parser_core.rs       # 语法分析核心
├── error_recovery.rs    # 错误恢复逻辑（独立复杂度）
└── ast_builder.rs       # AST 构造
```

## 4. 模块组织规范

### 4.1 `mod.rs` 只做编排

`mod.rs` 是模块入口，职责限定为：
- 声明子模块（`mod xxx;`）
- re-export 公共 API（`pub use xxx::*;`）
- 模块级文档注释（`//!`）
- **不放实现代码**

```rust
// src/service/mod.rs  ✓ 正解
//! 服务层：编排领域逻辑与数据访问。

mod auth;
mod order;
mod user;

pub use auth::AuthService;
pub use order::OrderService;
pub use user::UserService;

// 注意：这里不放 fn do_something() {} 实现
```

```rust
// ❌ 反模式：mod.rs 塞实现
// src/service/mod.rs (400 行)
mod auth;
pub fn shared_helper() { /* ... */ }  // 实现混在入口
pub fn validate_common() { /* ... */ }
// 问题：mod.rs 膨胀，入口失去"目录"作用
```

**共享辅助代码**应放在专门的 `common.rs` 或 `util.rs`：

```rust
// src/service/mod.rs
mod auth;
mod order;
mod user;
mod common;  // 共享辅助

pub use auth::AuthService;
// ...
```

### 4.2 模块深度控制

模块路径不宜过深（每层都是认知负担）：

```
✓ 推荐：3 层以内
src/
├── parser/
│   ├── lexer.rs
│   └── ast/
│       └── node.rs    # 3 层：parser > ast > node

✗ 过深：5+ 层
src/domain/services/order/processing/steps/validation.rs
// 读者要追踪 6 层路径才能定位文件
```

**规则：** 超过 3 层考虑扁平化或合并子模块。

### 4.3 模块命名与文件名一致

```rust
// 模块名 = 文件名（snake_case，单数）
mod user;           // src/user.rs
mod http_client;    // src/http_client.rs
mod order_service;  // src/order_service.rs

// ❌ 不一致
mod UserService;     // ❌ 模块名应 snake_case
mod users;           // ❌ 单数（除非真的是集合工具）
```

## 5. 完整项目布局示例

### 5.1 中型库项目

```
my-lib/
├── Cargo.toml
├── src/
│   ├── lib.rs                # <100 行，只 re-export 公共 API
│   ├── error.rs              # 错误类型
│   ├── config.rs             # 配置
│   ├── domain/               # 领域层
│   │   ├── mod.rs            # <30 行 re-export
│   │   ├── user.rs           # ~200 行
│   │   ├── user_tests.rs     # #[path] 外置测试
│   │   ├── order.rs
│   │   └── order_tests.rs
│   ├── repository/           # 数据访问层
│   │   ├── mod.rs
│   │   ├── user_repo.rs
│   │   └── order_repo.rs
│   └── service/              # 应用服务层
│       ├── mod.rs
│       ├── auth.rs
│       └── auth_tests.rs
├── tests/                    # 集成测试
│   ├── common/
│   │   └── mod.rs
│   ├── auth_flow.rs
│   └── order_pipeline.rs
├── benches/
│   └── parse_bench.rs
└── examples/
    └── basic_usage.rs
```

### 5.2 Workspace 多 crate 项目

```
my-workspace/
├── Cargo.toml                    # [workspace] members = ["crates/*"]
└── crates/
    ├── core/                     # 核心库（无 IO 依赖）
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── domain/
    │       └── ...
    ├── protocol/                 # 序列化协议
    │   ├── Cargo.toml
    │   └── src/
    ├── client/                   # HTTP 客户端
    │   ├── Cargo.toml
    │   └── src/
    ├── server/                   # 服务端
    │   ├── Cargo.toml
    │   └── src/
    └── cli/                      # 命令行
        ├── Cargo.toml
        └── src/
            └── main.rs           # 薄壳，只编排
```

**依赖方向单向：** `cli → client → core`，`server → core`，不可反向。

### 5.3 `lib.rs` 与 `main.rs` 分离

```rust
// src/lib.rs  ✓ 库入口，暴露公共 API
mod domain;
mod repository;
mod service;

pub use domain::{User, Order};
pub use error::{AppError, Result};
pub use service::{AuthService, OrderService};

// src/main.rs  ✓ 二进制入口，薄壳
use my_lib::{AuthService, OrderService};

fn main() -> my_lib::Result<()> {
    let auth = AuthService::new()?;
    let orders = OrderService::new()?;
    // 编排：解析参数 → 调用 lib → 输出
    run_app(auth, orders)
}
```

**规则：** 业务逻辑放 `lib.rs`，`main.rs` 只做参数解析和顶层编排。这样逻辑可被集成测试和多个 binary 复用。

## 6. 可见性与封装

### 6.1 最小化可见性

```rust
// ❌ 默认 pub，过度暴露
pub struct User {
    pub name: String,
    pub email: String,
    pub password_hash: String,  // 不应暴露！
}

// ✓ 字段私有，按需暴露访问器
pub struct User {
    name: String,
    email: String,
    password_hash: String,
}

impl User {
    pub fn name(&self) -> &str { &self.name }
    pub fn email(&self) -> &str { &self.email }
    // password_hash 不暴露
}
```

### 6.2 可见性层级

| 关键字 | 可见范围 | 用途 |
|--------|---------|------|
| （无） | 仅本模块 | 默认，最严格 |
| `pub(self)` | 仅本模块 | 同上，显式标注 |
| `pub(super)` | 父模块 | 模块内部协作 |
| `pub(crate)` | 本 crate | crate 内共享，不对外暴露 |
| `pub(in path)` | 指定路径内 | 精确控制 |
| `pub` | 全部 | 公共 API，谨慎使用 |

```rust
// src/service/mod.rs
mod auth;
mod order;

// crate 内可见（被其他模块用，但不对外）
pub(crate) use auth::AuthService;

// 仅本 crate 的 service 模块树内可见
pub(in crate::service) use order::internal_validate;

// 完全公共
pub use order::OrderService;
```

### 6.3 re-export 集中管理

公共 API 在 `lib.rs`（或模块 `mod.rs`）集中 re-export，**不在子模块直接 `pub`**：

```rust
// ✓ 正解：子模块实现用 pub(crate)，lib.rs 决定暴露什么
// src/domain/user.rs
pub(crate) struct User { /* ... */ }  // crate 内可见

// src/lib.rs
mod domain;
pub use domain::user::User;  // 在这里决定对外暴露 User
```

这样修改公共 API 只需改 `lib.rs`，不用追踪每个子模块。

## 7. 避免文件臃肿的实操模式

### 7.1 模式：类型 + 实现分离

大型类型把"定义"与"方法实现"分文件：

```
src/graph/
├── mod.rs
├── graph.rs          # struct Graph 定义 + 核心方法
├── graph_traversal.rs # impl Graph { bfs, dfs } 遍历方法
├── graph_mutate.rs    # impl Graph { add_node, remove_edge } 修改方法
└── graph_serialze.rs  # impl Graph { serialize, deserialize }
```

### 7.2 模式：配置与逻辑分离

```rust
// ❌ 配置常量混在逻辑文件
// src/server.rs
const MAX_CONN: u32 = 100;
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Server { /* 逻辑 */ }

// ✓ 配置独立文件
// src/config.rs
pub const MAX_CONN: u32 = 100;
pub const TIMEOUT: Duration = Duration::from_secs(30);

// src/server.rs
use crate::config::{MAX_CONN, TIMEOUT};
pub struct Server { /* ... */ }
```

### 7.3 模式：错误类型集中

```
src/
├── error.rs          # 所有错误类型集中（thiserror 定义）
├── domain/
└── ...
```

错误类型分散在各模块会导致 `use` 路径混乱。集中定义 + 类型别名简化：

```rust
// src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("用户不存在: {user_id}")]
    UserNotFound { user_id: u64 },
    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;
```

### 7.4 模式：trait 定义与实现分离

```
src/repository/
├── mod.rs
├── traits.rs         # trait UserRepository 等定义
├── in_memory.rs      # impl UserRepository for InMemoryRepo
├── db.rs             # impl UserRepository for DbRepo
└── mock.rs           # impl UserRepository for MockRepo（测试用）
```

让 trait 定义独立于实现，便于切换和 mock。

## 8. 导入组织

文件顶部 `use` 按分组排列，避免 import 混乱：

```rust
// 标准：分组顺序
// 1. std 库
use std::collections::HashMap;
use std::sync::Arc;

// 2. 外部 crate
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// 3. 本 crate 其他模块
use crate::domain::User;
use crate::error::Result;

// 4. 本模块内（super / self）
use super::repository::UserRepository;
```

**规则：**
- 单文件 `use` 超过 30 行 → 拆分信号的强烈指标
- 用 `cargo +nightly fmt` 自动排序（`imports_granularity = "Crate"`、`group_imports = "StdExternalCrate"`）

## 9. 文件布局反模式

### 9.1 上帝文件

```rust
// ❌ src/everything.rs (2000 行)
// 包含 User, Order, Payment, Auth, DbClient, HttpClient...
// 解决：按领域拆分到子目录
```

### 9.2 碎片化过度

```rust
// ❌ 过度拆分：每个函数一个文件
src/
├── user_create.rs      # 就一个函数
├── user_update.rs      # 就一个函数
├── user_delete.rs      # 就一个函数
// 问题：文件数爆炸，相关逻辑分散，导航反而更难

// ✓ 合并相关函数
src/user.rs             # User 的所有操作
```

**平衡：** 拆分到"每个文件聚焦一个类型族/职责"，不要拆到每个函数。

### 9.3 平铺目录

```rust
// ❌ 所有文件堆在 src/ 根
src/
├── user.rs
├── order.rs
├── payment.rs
├── auth.rs
├── http_client.rs
├── db.rs
├── config.rs
├── error.rs
├── utils.rs
├── ... (30 个文件)
// 问题：文件多了找不到

// ✓ 按层/领域分组
src/
├── domain/{user, order, payment}.rs
├── service/{auth, order}.rs
├── infra/{http_client, db}.rs
└── {config, error, utils}.rs
```

**经验法则：** `src/` 根下文件超过 10 个，考虑分组到子目录。

## 10. 文件布局检查清单

### 体量
- [ ] 实现文件 <300 行（硬上限 500）
- [ ] 测试文件 <200 行（超限用 `#[path]` 外置或分组拆分）
- [ ] `mod.rs` <50 行，只 re-export 不放实现
- [ ] `lib.rs` <100 行，只 re-export 公共 API
- [ ] `main.rs` <100 行，只编排不实现业务逻辑
- [ ] 单函数 <50 行

### 模块化
- [ ] 每个文件单一职责（能用一句话描述）
- [ ] 相关类型聚合，不相关类型分离
- [ ] 模块深度 ≤3 层
- [ ] 模块名与文件名一致（snake_case 单数）
- [ ] 共享辅助代码放 `common.rs`/`util.rs`，不塞 `mod.rs`

### 可见性
- [ ] 字段默认私有，按需暴露访问器
- [ ] 用 `pub(crate)` 限制 crate 内可见
- [ ] 公共 API 在 `lib.rs`/`mod.rs` 集中 re-export
- [ ] 不为测试把私有项改成 `pub`（见 testing-standards.md）

### 组织
- [ ] `use` 分组排列（std → 外部 → 本 crate → 本模块）
- [ ] 单文件 `use` <30 行（超限考虑拆分）
- [ ] 错误类型集中定义
- [ ] trait 定义与实现分离
- [ ] 大型类型按方法族拆 `impl`

### 目录
- [ ] `src/` 根文件 ≤10 个（超限分组）
- [ ] workspace 多 crate 依赖方向单向
- [ ] `tests/common/` 放集成测试共享代码
- [ ] 不出现上帝文件（单文件多领域）也不碎片化（一函数一文件）

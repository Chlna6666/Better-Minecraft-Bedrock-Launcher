# 命名规范

本规范覆盖 BMCBL 工作区的 crate、Rust 标识符、模块路径、源文件名和文档路径。
命名应让读者从**完整路径、调用表达式和签名**迅速理解主要职责。名称优先表达功能；
参数值、可选条件和实现步骤由签名、类型和函数体表达。

Rust 的大小写规则仍按 Rust 惯例。JavaScript/TypeScript 的 camelCase、C# 的 PascalCase
可以作为清晰表达职责的参考，不能照搬到 Rust 标识符上。

## 1. 语言风格与标识符

| 对象 | Rust 写法 | 示例 |
| --- | --- | --- |
| Cargo package | 小写，通常使用 kebab-case | `bedrock-world` |
| Rust crate、模块、文件和目录 | `snake_case` | `bedrock_world`、`chunk_records.rs` |
| 函数、方法、字段、局部变量 | `snake_case` | `load_records`、`record_count` |
| 类型、trait、enum 变体 | `UpperCamelCase` | `ChunkRecord`、`RecordSource` |
| 常量、静态变量 | `SCREAMING_SNAKE_CASE` | `MAX_RECORDS` |
| 缩写 | 按普通单词分段 | `HttpClient`、`parse_url` |

JS/TS 常用 camelCase，C# 常用 PascalCase；这些语言的命名形式各自遵守生态约定。
跨语言借鉴其清楚表达职责的做法，Rust 代码仍使用 `snake_case` / `UpperCamelCase`。

公开 API、FFI、生成代码、协议字段和已有稳定外部契约可以有局部例外。例外只覆盖
契约要求的名称，不改变其他 Rust 代码的命名规则。

## 2. 函数和方法表达主要职责

### 2.1 从调用位置确定上下文

先读完整调用位置。模块、类型和 receiver 已经提供的上下文，不要在函数名里重复：

`world::chunk_records::load(options)`
`ChunkRecordStore::load(options)`

模块外的独立函数可以写出必要对象，例如 `load_chunk_records(options)`。
若调用上下文已经是 `chunk_records`，`chunk_records::load_chunk_records(...)` 就重复了。
同理，方法名不必重复类型名或 receiver 的领域。

函数名优先表达**主要动作和操作对象**，不要逐项罗列输入、参数名、参数值、配置项、
可选条件或内部步骤。签名负责表达接受什么值、哪些值可选，以及如何选择行为。

### 2.2 参数属于签名，不属于函数名

不要为一个操作的不同参数组合创建函数名，也不要把参数名或参数约束机械拼进函数名。
用参数、`Option`、enum 或配置结构体表达调用差异；只有当两个入口代表不同的领域操作时，
才使用不同函数名。

例如，`load_chunk_records_with_actor_limit(max_actor_records)` 把一个可选参数写进名称，
还可能误导读者以为所有调用都受此限制。按上下文使用 `load_chunk_records(options)`，
或在 `chunk_records` 模块内使用 `load(options)`；`max_actor_records` 保留在签名或选项中。

配置结构体应在参数相互关联、可选项较多或调用需要一个有名字的配置时使用。
不要求每个函数都包一层 `Options` / `Descriptor`；不要用参数对象掩饰本来应拆开的操作。
布尔参数只在调用处读起来明确时使用；代表多个模式时优先用 enum。

### 2.3 选择能说明核心动作的动词

动词应准确描述函数主要完成的工作。不要仅因返回集合就用 `collect`，也不要因内部
读取数据就把所有函数都叫 `load`。名称应区分真实不同的操作，例如查询、加载、解析、
校验、转换、计算指纹、聚合、构建和写出；不要求每个函数遵循固定的动词词典。

`parse` 只用于把文本或字节表示转换为结构化数据，不是“准备”或“读取”的通用替代词。
返回 `ScanOptions` 之类配置值的函数应按用途命名为 `scan_options`；不要因为函数返回值
尚未解析的数据，或因为想让名称听起来像操作，就添加 `parse_`。与 `encode` 成对的二进制
反向操作通常命名为 `decode`。保留真正执行语法 / 文档解析的 `parse` API，不按动词做全局替换。

`_blocking` 不是普通同步函数的默认后缀。若同一职责确有异步与阻塞入口，优先将阻塞入口放入
`blocking` 子模块，使调用路径表达调度方式、函数名仍表达动作，例如
`request::blocking::fetch_bytes_with_retry`。只有拆分子模块会让职责边界更含糊时，才在函数名保留
`_blocking`。`_async` 也不应仅因函数声明为 `async fn` 就机械添加；同步 / 异步差异通常由签名表达。

例如，`collect_chunk_record_fingerprints(records)` 若主要工作是计算指纹，应改为
`fingerprint_chunks(records)`；如果模块已是 `chunk_records`，可写作
`chunk_records::fingerprint(records)`。只有遍历已有值并聚合这一行为本身是核心职责时，
`collect` 才准确。

`with`、`for`、`many`、`inner`、`limit`、`if_needed` 等不是禁词，也不是默认模板。
仅当该词表达调用者需要区分的稳定语义时才保留；不要只为了反复参数、条件或实现细节
而扩展名称。没有固定字符数上限，过长或堆叠多个动作的名字应促使重新检查上下文、
主要职责和函数拆分，而不是触发自动截短。

### 2.4 常用 API 语义

- 布尔状态查询通常使用 `is_*`、`has_*`、`can_*` 或 `should_*`，并返回 `bool`。
  `supports_*` 表达能力探测，不用于表达普通开关当前是否启用。
- 普通属性访问器使用属性名；`get` / `get_mut` 用于 key、index 或容器查询。HTTP 传输层的
  `get` 可以表示 GET 请求；面向具体远程资源的数据接口应按操作命名为 `fetch`、`search` 等，
  不用 `get_*` 作为泛化读取动词。
- 转换方法遵循 [Rust API Guidelines 的 `as_` / `to_` / `into_` 约定](https://rust-lang.github.io/api-guidelines/naming.html#ad-hoc-conversions-follow-as_-to_-into_-conventions-c-conv)：
  `as_*` 用于无额外运行成本的借用视图，`to_*` 用于保留输入所有权但需要实质转换或计算的结果，
  `into_*` 消耗 `self` 并转移或拆出其所有权。普通属性读取用属性名，不要因为返回字符串或路径就写成
  `to_*`；例如枚举返回其固定目录名时应使用 `directory_name()`。有额外区分参数的转换构造器可用
  `from_*`，否则优先评估标准 `From` / `TryFrom` trait。
- `new` 用于惯用的最小构造；builder 用于多项可选配置。`create` 表达有意义的创建动作，
  不与普通构造混用。
- 错误类型通常以 `Error` 结尾；公开函数应有与实际错误和 panic 契约相符的 rustdoc。

测试名可较长，以表达场景和预期结果；产品 API 不应复制测试句子作为函数名。

### 2.5 审查导入别名 `as`

- `use path::Item as Alias` 只在别名解决真实名称冲突、区分平台 / 后端 / 格式类型，或建立有意的
  crate 根公开入口时保留。别名不能只是把来源模块名重复到标识符里，也不能为了继续使用旧路径而
  掩盖模块已经移动或重命名。
- 同作用域只有一个来源且别名不增加语义时，直接导入原名；若为了缩短路径而加别名，优先导入模块
  并在调用点写出模块路径。先检查完整调用点，不按 `as` 语法形式批量删除。
- 如果类型别名会隐藏调用者需要知道的同步 / 异步、平台、格式、安全或性能差异，应在名称中保留该差异，
  如 `StdMutex` / `AsyncMutex`；不要把 `FxHashMap` 伪装成普通 `HashMap`。只有确实建立了稳定的实现边界时，
  才可以隐藏这类差异。
- `as _` 用于只导入 trait 方法而不引入名称，确认对应方法调用后保留。
- `pub use ... as ...` 可以为类型、trait 或有意设计的公开 facade 建立入口，包括 crate 根入口和领域模块入口。
  领域自由函数仍以 `domain[::subdomain...]::operation` 为规范路径，不要扁平导出到 crate 根；若来源模块本身
  仍公开，也不要仅为第二条调用路径创建别名。facade 必须建立清楚的模块职责或唯一公开路径，不能充当旧名兼容包装。
  名称仍须准确表达对象和动作。除非存在明确的外部兼容契约，不新增兼容别名。
- 审查别名时核对完整模块路径、导入项、调用点、可见性及是否有同名 API；删除或改名时同步修复
  文档、测试、宏路径和条件编译分支。

## 3. 模块路径、文件和目录

### 3.1 按职责组织完整路径

Rust 模块、源文件名和目录名使用小写 `snake_case`。模块路径应能对应到文件树，例如：

`src/world/chunk_records.rs` → `world::chunk_records`

按稳定的领域或职责组织目录。父模块已经表达的词，不必在子文件名和每个函数名里重复。
完整路径可以自然地提供上下文：

`src/world/chunk_records.rs` 中的 `pub fn load(options: LoadOptions)`
可通过 `world::chunk_records::load(...)` 调用。

模块名应使用准确的领域术语，并把紧密相关的数据与操作放在同一职责模块。公开函数名
结合模块路径阅读；模块已说明对象时，函数名表达动作即可：

```rust
bedrock_world::level::parse(data);
bedrock_world::level::read(path);
bedrock_world::level::write(path, &document);
```

### 3.1.1 领域路径和命名审查（强制）

公开的模块级领域操作使用 **`crate::domain[::subdomain...]::operation(...)`**：领域层级标识被操作的
对象 / 文件 / 格式，最后一段表达动作。自然属于某个类型 receiver 的操作使用 `Type::operation(...)`。
领域层级必须体现真实职责；不得为了符合形式而创建包装模块或泛化层级。

```rust
bedrock_world::level::read(path);
bedrock_world::level::write(path, data);
bedrock_world::player::read(path);
```

评审和重命名必须满足以下规则：

1. **禁止无意义的语义重复**：模块路径已明确操作对象时，不得在函数名中再次重复对象；例如优先使用
   `level::read`，而不是 `level::read_level_dat_document`。
2. **禁止机械式重命名**：修改前必须检查函数签名、实际行为、所属模块和调用关系，不得只依据函数名称
   或长度批量替换。
3. **禁止牺牲语义准确性**：同一模块存在多个操作对象或多种读取行为时，使用更精确的函数名，或拆分为
   能表达差异的子模块；不得为了短名合并不同语义。
4. **公共 API 与内部 API 遵循同一模块命名原则**：按完整路径和职责命名，不因函数是 `pub` 就在函数名中
   重复完整领域名；可见性本身不是命名上下文。
5. **优先用模块分层消除歧义**：`domain::operation` 不足以表达真实行为时，先评估
   `domain::subdomain::operation` 等领域层级，再决定是否需要更具体的函数名。只在子领域真实、独立、内聚时
   增加中间层；禁止 `bedrock_world::write::level_dat(...)` 这类动作在领域前的反向路径。
6. **不保留无必要的旧名称包装**：确认工作区调用点已经迁移后可以删除重复入口；删除前必须检查外部调用方 / 依赖、
   宏路径、测试、公开导出和文档示例，并确认符合当前公开 API / 发布契约。crate 根不能把领域自由函数扁平
   重导出为第二套入口；不要用兼容包装掩盖模块迁移。

领域模块直接使用具体领域名，如 `level`、`player`、`map_item`；不要使用 `*_domains` 聚合多个领域。
实现增长时可按内聚职责拆入领域目录的子模块，对外路径应继续准确表达真实领域层级。

Rust 库没有统一的读写路径模板：[`serde_json`](https://docs.rs/serde_json/latest/serde_json/)
和 [`fastnbt`](https://docs.rs/fastnbt/latest/fastnbt/) 在格式命名空间下使用
`from_slice` / `from_bytes`、`to_writer`；[`std::fs`](https://doc.rust-lang.org/std/fs/)
和 [`tokio::fs`](https://docs.rs/tokio/latest/tokio/fs/) 使用 `read` / `write`；当读写器
本身是成组类型时，[`zstd::stream`](https://docs.rs/zstd/latest/zstd/stream/) 还会按 `read` /
`write` 拆分子模块。这些是按职责选路径的例子，不要求机械照搬某个库的层次。

BMCBL 的领域对象路径采用“领域模块在前、操作在后”，例如 `level::read` 和 `level::write`。
`level.rs` 作为模块入口；当职责确实增多时，再把内聚实现拆到 `level/` 子模块，保留调用方
可理解的 `level::动作` 路径。不要创建只为转发单个函数的泛化命名空间或兼容别名。

不要机械地在所有层级重复 `document`。若同一模块内存在读取原始 NBT 根和完整
`LevelDatDocument` 的不同 API，再按返回对象区分名称；若模块路径和签名已说明目标，
`parse`、`read`、`write` 等动作名无需追加冗余对象词。命名应与输入、返回类型、错误映射
和实际副作用一致。

不要只为缩短路径删除必要领域词，也不要因为目录层次变深就机械扁平化。先看新成员属于
哪个状态、数据模型、职责和生命周期边界，再决定放入现有模块、子模块或新文件。

### 3.2 文件名对应内容，不对应单个函数清单

文件名应概括一个内聚模块的职责或类型族。不要按每个函数单独建文件，也不要把不相关
职责塞进 `common`、`utils`、`misc` 等泛化文件。模块可以按领域实体、格式、平台边界或
操作职责组织；同一模块族应使用清楚且一致的分类维度。

避免无信息量重复，例如在职责已由 `item::representation` 表达时，继续把相同领域词堆入每个
叶子文件名。必要的格式、平台、版本或数据模型限定词应保留，前提是它们区分真实职责
或外部身份。

Rust 支持 `foo.rs` 与 `foo/bar.rs` 共同构成模块树，也支持 `foo/mod.rs`。新模块通常
采用 `foo.rs` + `foo/bar.rs`；已有布局只有在本次重组能改善导航或职责边界时再迁移。
不要同时创建同一模块的两个入口。生产模块必须使用 Rust 常规文件布局；`#[path = "..."]`
及 `cfg_attr` 中的 `path` 只允许用于测试或性能基准模块，禁止用它映射生产子模块或选择平台实现。平台差异应通过
常规目录下的 `#[cfg]` 模块组织。

### 3.3 命名重组可以改变源代码路径

当更名或迁移能让模块所有权、职责或调用路径更准确时，可以一起调整函数名、模块名、
目录和文件名。任务已允许破坏性变更时，不需要为了旧的 Rust 源码路径默认保留兼容别名。
以完整路径和职责改善为依据，不以“更短”本身为依据，也不做未经语义判断的批量替换。

重组前后要核对 `mod` / `pub mod`、`pub use`、所有 `use` 引用、`#[path]`、`include!`、
构建脚本、feature / `cfg`、测试和文档链接。若公开 Rust API 的模块路径变化会影响外部
调用者，按任务授权和项目发布契约处理。

Rust 源文件路径与运行时数据路径是不同契约。不要因源码整理改写存档目录、用户配置、
资源包/插件约定路径、序列化字段、协议内容、CLI 参数、环境变量或 FFI 符号。

### 3.4 文档路径

- 集中文档使用 `docs/UPPER_SNAKE.md`；存在英文镜像时使用 `*.en.md` 并同步维护。
- ADR 使用 `docs/adr/NNNN-kebab-case.md`，按决策顺序编号。
- 文档文件名表达稳定主题，不使用日期、临时阶段或改名审计作为长期命名。

## 4. BMCBL 图形层术语

| 概念 | nova-gfx（wgpu 对齐） | gpui | 约定 |
| --- | --- | --- | --- |
| 图形设备 | `Device` / `Backend` | `NovaBackend` | 避免 `GfxDevice` |
| 提交 | `SubmissionDevice`、`SubmissionId` | `PendingSubmission` | 避免 `BackendQueue` |
| 交换链 | `Swapchain`、`SwapchainId` | `swapchain` 字段 | `backbuffer` 只用于 DXGI 原生语义 |
| 绑定资源 | `BindGroup`、`BindGroupLayout` | 内部资源绑定 | 新 API 不用 `ResourceSet` |
| 绘制命令 | `RenderCommand::Draw(DrawCommand)` | `DrawCommand` | 不用 `RenderStep` 表示命令 |
| 呈现帧 | `PresentFrameDescriptor`、`render_and_present` | `PresentationPacket` | 按实际帧语义区分 |
| 离屏 pass | `RenderPassCommands`、`render_to_textures` | `PreparedColorLayer` | 按 pass / layer 类型命名 |
| 脏区域 | `present_region` 参数 | `DirtyRegion`、`damage` | gfx-core 通用接口不用 `damage` |
| 保留层 | 不出现 | `retained`、`RetainedPaint` | nova-gfx 不使用 `retained` |

通用 gfx API 不暴露 UI 或产品策略。图形词汇按真实类型和职责使用；不同概念无需为了表面
一致而强行统一。后端专有词只留在后端边界，不能泄漏为通用契约。

## 5. 命名评审清单

- [ ] 按完整模块路径、类型 / receiver 和调用点判断名称是否重复或缺少上下文。
- [ ] 函数名表达主要动作与对象；参数、参数值和可选条件由签名或选项类型表达。
- [ ] 动词描述真实核心操作；查询、加载、计算、聚合、构建等没有被混为一谈。
- [ ] `use ... as ...` 别名解决真实冲突、表达清晰的 API 层级，或只导入 trait；没有保留过渡 / 兼容旧名。
- [ ] 领域命名通过 §3.1.1 六项强制规则；删除旧入口前已检查外部调用、宏、测试、公开导出和文档示例。
- [ ] 公共 API 的模块路径和签名能准确识别操作对象；短名有完整路径语义支撑，`parse` / `read` / `write` 没有机械互换。
- [ ] 名称符合 Rust 大小写惯例；类型和符号没有无意义重复。
- [ ] 文件名、目录名、模块路径共同表达内聚职责和领域所有权。
- [ ] 改路径时核对模块声明、导出、调用方、构建引用、测试和文档。
- [ ] 破坏性变更按任务授权处理；没有把源码命名规则套到运行时或外部数据契约上。
- [ ] 测试名称描述行为，不把实现步骤误作产品 API 命名模板。

`scripts/check-naming.ps1` 会检查第一方 Rust 源码中的 `#[path = "..."]` 和
`cfg_attr(..., path = "...")`（包括多行属性），并拒绝生产用途；它们只允许出现在测试或性能基准代码中。
脚本也会检查其明确覆盖的图形术语约定。自动脚本不能判断名称
是否准确表达功能；长名称阈值和匹配词只是审查线索，最终判断应基于调用上下文、签名、实现
和引用关系。

本仓库现有检查命令：

`pwsh -File scripts/check-nova-gfx-boundary.ps1`
`pwsh -File scripts/check-naming.ps1`
`pwsh -File scripts/check-docs.ps1`

命名脚本仅覆盖其明确扫描的文件和符号；零告警不代表所有工作区名称与路径均经过语义审查。

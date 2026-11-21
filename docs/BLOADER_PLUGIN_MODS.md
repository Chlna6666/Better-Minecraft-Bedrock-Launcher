# BMCBL 插件虚拟 Mod ↔ BLoader 协议规范

> 状态：**协议设计及分阶段实现基线（v1；尚未完成双端联调）**。BLoader 已开始实现通信、加载与可选原生 ABI，但 BMCBL 会话宿主尚未完成，不能视为端到端已可用。协议主版本 `1`、次版本 `0`；开发阶段允许破坏性修订，必须同步更新双方及测试。

## 1. 范围、术语与职责

“虚拟 Mod”是**BMCBL 插件目录里已安装的公共 DLL，由 BMCBL 按游戏版本授权后通知 BLoader 直接从原位置加载**的机制。**不复制、不创建额外 Mod 暂存目录、不写进游戏的 `mods/`**；“虚拟”指与各个游戏版本的 Mod 目录解耦，而不是内存 PE 加载或 WASM 沙箱。典型路径为 `BMCBL/plugins/<plugin_id>/game-mods/<module>.dll`，多个 Minecraft 版本可以共用同一文件，但各自独立授权。

| 组件 | 唯一职责 | 明确禁止 |
| --- | --- | --- |
| BMCBL 插件打包器 | 验证 `game_mods` 声明，把 DLL 纳入 `.bmcblx` | 将插件提供的绝对路径当作运行时加载目标 |
| BMCBL 插件管理器 | 安装、启停、逐版本逐摘要授权；生成不可变启动快照 | 允许 WASM 插件自行选择 Minecraft PID、会话或任意 DLL |
| BMCBL 会话宿主 | 创建受限 IPC、校验进程、重验已安装插件 DLL、传递公共插件根路径与相对模块路径 | 为每个游戏版本复制 DLL、创建额外 Mod 暂存树，或把账号令牌和启动器特权 API 暴露给 Mod RPC |
| BLoader | 校验启动计划及实际载荷、在指定阶段加载 DLL、报告状态、提供受限原生 ABI | 自行枚举 BMCBL 插件目录、解释插件配置权限或重新决定授权 |
| 插件 WASM | 通过 BMCBL 授权的宿主接口与**自己的** Mod 通信 | 直接访问 BLoader IPC、指定加载路径、伪造其他插件的模块身份 |

- `plugin_id` 为 BMCBL 插件 ID，`module_id` 为该插件内声明的 Mod ID；二者组合的 `plugin_id/module_id` 是稳定逻辑标识，不能仅用 DLL 文件名作为身份。
- `plugin_generation` 是 BMCBL 内一次插件运行实例的标识；插件卸载、重载、禁用后不得复用。`session_id` 是一次游戏启动的随机会话标识。二者都**不是密码或安全能力凭据**。
- `preload` 表示 BLoader 自有 `mod-preload`，须在 Minecraft OEP 释放前、但在 Windows loader lock 之外执行；`ready` 表示 BLoader 自有 `mod-native`，在 OEP 已释放并满足 `ReadyLevel::Process` 后执行。它们与 PreLoader/LeviLamina 的 Mod 类型没有替代或隐式接管关系。BLoader 当前流程先处理 PreLoader，再处理自身 `mod-preload`；不要把 `preload` 误解为“早于 PreLoader”。
- 已在 `mods/`、`BLoader/mods/` 中由既有机制管理的 Mod 保留原有行为；虚拟 Mod **不得写入这些目录或伪装为 LeviLamina Mod**。
- v1 `game_mods.platform` 指**Minecraft 进程内的目标二进制 ABI**，不是 BMCBL 宿主操作系统：`windows-x86_64` PE DLL 可同时用于 Windows 原生游戏及 Linux 上 Wine/Proton/UMU 内运行的 Windows x64 Minecraft。不能因为宿主为 Linux 就将它重标记为 `linux-x86_64`。Linux 原生 `.so` 不得直接用 BLoader 的 Windows `LoadLibraryExW` 加载；如需 Linux 原生组件，应通过独立的 BMCBL Native Bridge 在 Linux 宿主侧管理。GDK/UWP 等具体启动形态与 BLoader 是否能在相应阶段启动必须逐个验证，未验证时返回不支持。

## 2. 现有实现与尚未实现的边界

截至本文编写时，可核对的代码：

- BMCBL `src/plugins/game_mods.rs`：解析声明、限制主 DLL 32 MiB、验证 PE x64、计算 SHA-256，并按版本保存 `plugin_id/module_id → sha256` 授权。
- BMCBL `src/plugins/manifest.rs` 和 `crates/bmcbl-plugin-api/src/pack.rs`：每插件最多 32 个声明；打包时将 `game-mods/` 文件加入包。
- BMCBL `src/plugins/runtime.rs`、`src/ui/views/manage/mod_tab.rs`：只枚举启用且已加载插件的 Mod 并提供管理页开关。
- BLoader `src/loader/mod.rs`：既有 `mod-preload` / `mod-native` 从其管理的磁盘目录发现；新 `src/virtual_mods/` 另接收启动计划并按插件公共路径加载，**不扫描或移动插件文件**。需要 Windows 与 Wine 双端联调才能宣布可用。
- BMCBL `src/core/inject/legacy.rs` 与 BLoader `src/bootstrap/xuser_bridge/ipc.rs` 存在专用 XUser 单次管道，**不是** Mod IPC，不可直接复用其载荷、消息类型、密钥或授权逻辑。

**尚需完成和验证**：BMCBL 会话型双向 IPC、启动计划快照和交接、原地文件复验、插件业务消息路由、取消与诊断回传；BLoader 现有代码必须经过持续集成与 Windows/Wine 实际加载测试。管理页显示“已授权”不等于 Mod 已在游戏中加载成功。

## 3. 清单与授权

### 3.1 现行清单（保持已实现字段）

```toml
[[package.metadata.bmcbl-plugin.game_mods]]
id = "example-mod"
name = "Example Mod"
path = "game-mods/example.dll"
platform = "windows-x86_64"
phase = "ready"
game_versions = ["1.21.120", "26.10"]
```

打包器转换为 `plugin.toml` 中的 `[[game_mods]]`。字段约束：

| 字段 | 规范 |
| --- | --- |
| `id` | 每插件唯一，匹配当前插件 ID 校验规则：3～64 字节、小写 ASCII 字母开头，其余小写 ASCII 字母/数字/单个中划线，不以中划线结尾 |
| `name` | 非空 UTF-8 展示名称，最多 128 字节；**不参与安全标识** |
| `path` | 使用 `/` 的包内相对文件路径，必须位于 `game-mods/` 下并以 `.dll` 结尾；拒绝绝对路径、`..`、设备路径、UNC、空组件、反斜线与逃逸链接 |
| `platform` | 目标**游戏进程 ABI**；v1 的游戏内 DLL 仅 `windows-x86_64`，含 Linux Wine/Proton/UMU 运行的 Windows Minecraft；未知值拒绝。`linux-x86_64` `.so` 是未来 Linux **宿主原生组件**场景，不应直接加入 BLoader 游戏内 `game_mods` |
| `phase` | `preload` 或 `ready`；未知值拒绝 |
| `game_versions` | 1～32 个非空点分数字前缀，单项最多 32 字节；只按**组件边界**匹配，`26.10` 可匹配 `26.10.2`，不能匹配 `26.100` |

每包最多 32 个 Mod，主 DLL 不超过 32 MiB。清单解析与解压都必须设资源上限；压缩包索引路径经过规范化后不得重复、大小写碰撞或映射到同一磁盘文件。不得因为未声明 `game_mods` 而扫描其余 DLL。

**宿主 OS 与目标 ABI 必须分离。** 现有 Rust `Declaration` 使用 `#[serde(deny_unknown_fields)]`，尚无 `host_platforms` 字段；下列方案为**待两端同步实现的清单扩展**，不能在未修改解析器时直接用于已发布插件：

```toml
[[game_mods]]
id = "example-mod"
name = "Example Mod"
path = "game-mods/example.dll"
platform = "windows-x86_64" # 游戏进程 ABI，不是宿主 OS
host_platforms = ["windows", "linux"] # 待实现：可选宿主限制
phase = "ready"
game_versions = ["26.10"]
```

- 推荐 `host_platforms` 为**可选限制条件**，未设置不意味着只允许 Windows：只要游戏二进制 ABI 匹配、BLoader 可达、IPC 安全可用、用户已授权即可在 Linux 运行 Windows DLL。若插件有特定宿主 OS 限制，再显式填写。
- Windows 原生、Wine、Proton、UMU 是**运行器能力**而非 DLL ABI。运行器能力应由宿主在启动时探测，不能通过插件的任意声明自行开启。宿主可记录 `runtime_kind = native-windows | wine | proton | umu`，避免重复打包同一 DLL。
- 如未来真的出现 Linux 原生 Minecraft 与原生 Linux Mod 加载器，再通过新平台值和对应本机加载后端扩展；不能将其等同于运行在 Wine 中的 Windows 游戏。

### 3.2 授权的安全语义

- 用户对具体游戏版本显式执行“授权并启用”；授权键为 `(version_id, plugin_id, module_id, sha256)`，**默认拒绝**。禁用 Mod 加载、插件禁用/卸载、版本不匹配、载荷变化或授权撤销均使模块不进入**下一次**启动快照。
- SHA-256 对**实际 DLL 字节**计算，不对名称、`.bmcblx` 路径或文件修改时间计算。其用途是确认用户授权了哪些字节；SHA-256 **不是数字签名，也不证明发布者可信**。
- BMCBL 必须在每次启动准备阶段重新读取并核验载荷；不能无条件信任插件安装时缓存的 `sha256`。清单、插件状态、版本状态和授权表必须形成同一个一致性快照，避免读取期间更新或撤销导致“新文件、旧授权”混用。
- 授权显示必须区分“已授权/未授权/本次计划已接收/游戏已加载/加载失败”。加载失败不能自动授予另一份 DLL；热修改授权不触发原生 DLL 的卸载或热替换。
- 插件 `generation` 变化立即撤销其业务消息路由；若 DLL 已被加载，其机器码仍存在于游戏进程直到进程退出。**不能宣称即时禁用可撤销已执行的原生代码**。

## 4. 威胁模型与不可跨越的边界

**假设**：BMCBL 和经过用户信任的 BLoader 是宿主可信组件；插件 WASM 比宿主低信任；插件所携原生 DLL 一旦加载即与 Minecraft 同进程，具备该进程的原生代码权限。

1. 本地管道的进程身份校验可阻挡其他 PID 意外接入，但**不能隔离同一 Minecraft 进程内的恶意 DLL**。不能将“消息声称来自哪个 Mod”当作强加密级别的 DLL 身份认证。
2. 沙箱权限只约束 WASM→BMCBL 的宿主接口；原生 DLL **不受 WASM 沙箱、WASM 网络白名单或资源配额约束**。首次授权 UI 必须明确告知“运行原生代码”及其风险。
3. 不能保障对已拥有同一用户级调试、注入或磁盘写权限的恶意本地进程的绝对隔离。不得借用令牌、XUser 密钥或账户桥接来实现 Mod 身份确认。
4. 不允许协议透传 `LoadLibrary(path)`、`GetProcAddress(symbol)`、任意文件读写、启动程序、运行系统命令或通用内存访问等指令。Mod RPC 是由已授权模块的**已注册业务接口**处理的非特权数据通道。
5. 认证失败、协议不匹配、载荷复验失败时**本次虚拟 Mod 默认跳过（fail closed）**；Minecraft 本身及既有原生 Mod 的错误处理链不应被虚拟模块失败阻断，除非有单独、明确的产品级启动策略。

## 5. 本地 IPC 传输与启动顺序

### 5.1 Windows 首选传输

使用独立的双向 byte-mode Named Pipe，名称格式：`\\.\pipe\BMCBL.PluginMods.<minecraft_pid>`。和 `BMCBL.XUser.<pid>` **物理隔离**。

- BMCBL 在目标进程已创建、但尚未放行游戏入口点前建立唯一管道实例；设置 `FILE_FLAG_FIRST_PIPE_INSTANCE`、`PIPE_REJECT_REMOTE_CLIENTS`，仅允许预期用户 SID 和 SYSTEM 访问的显式 DACL，并使用有界异步 I/O。禁止默认宽松 DACL、匿名 TCP 监听或共享的全局固定端点。
- 宿主对已连接管道使用 `GetNamedPipeClientProcessId` 验证目标 Minecraft PID，并将进程句柄/创建时间与**本次启动**绑定，检查用户身份/会话。不单独依赖 PID 字符串、管道名称或客户端自报身份。
- BLoader 对已连接管道使用 `GetNamedPipeServerProcessId` 验证预期 BMCBL 宿主 PID，并检查可验证的进程关系/身份。直接子进程路径可以核对受信任的父进程；代理/间接启动必须提供另一个**可由 OS 验证**的宿主绑定手段，不能静默放宽为“同用户即可”。
- 身份验证通过之后才交换 `hello/welcome`、启动计划和任何载荷路径。`session_id` 与随机挑战/一次性连接材料仅用于防止跨启动混淆；**不能单靠自报 nonce、PID 或 SHA-256 当作认证**。任何握手材料不写入环境变量、命令行、日志或插件存储。
- 每次启动仅接受一次经认证的连接；重复连接、超时或会话关闭后不自动复用旧计划。支持重连需另行定义重新认证、已加载模块与请求状态恢复，**不属于 v1**。

### 5.2 Linux 宿主 ↔ Wine/Proton/UMU 中的 BLoader

BMCBL 是**Linux 原生进程**，BLoader 是 Wine/Proton 中运行的 **Windows PE DLL**。两个进程不共享 Windows Named Pipe 命名空间；Linux 原生宿主也不能直接把 Wine 内部 Windows PID 当作 Linux PID 使用。**保持第 6 节的消息格式与第 8 节 ABI 不变，仅替换传输和启动引导层。**

#### 5.2.1 传输优先级

| 宿主/运行器 | BMCBL 端 | BLoader 端 | 使用条件 |
| --- | --- | --- | --- |
| Windows 原生 | Windows Named Pipe | Win32 Named Pipe | 必须双向校验目标与服务端进程身份 |
| Linux + 已验证支持 AF_UNIX 的 Wine/Proton/UMU | `tokio::net::UnixListener` | Wine Winsock `AF_UNIX/SOCK_STREAM` | 必须在**实际使用的 Wine/Proton 构建**上验证 socket/connect/路径映射；Linux 侧校验 `SO_PEERCRED`；仍执行独立会话认证 |
| Linux + 未验证/不支持 AF_UNIX 的 Wine/Proton/UMU | `tokio::net::TcpListener` 仅绑定 `127.0.0.1:0` | Winsock `AF_INET/SOCK_STREAM` | **一次性高熵秘密 + 双向挑战认证通过后**才允许进入应用协议；没有安全引导材料时拒绝连接 |

Wine 对 Windows Winsock `AF_UNIX` 的支持存在不同版本/补丁集差异。不得根据宿主内核支持 Unix Socket 或 Wine 版本号直接宣告支持；需要真实运行时探测。**不允许未经认证的 loopback TCP**。如果使用网络命名空间/沙箱，必须验证宿主与游戏可达同一个 loopback；不能将端点自动发布到 `0.0.0.0`、公网或 LAN。

这两种 Linux 传输**都应使用相同的会话认证**，`SO_PEERCRED` 仅作为补充身份信息。与 Wine 的其他内部 socket/wineserver 私有协议保持隔离，不依赖私有 Wine ABI。**无需额外启动一个 Windows 桥接进程**，除非具体 Proton/UMU 隔离机制使两种直连传输均不可用，并通过单独验证后才引入代理。

#### 5.2.2 Linux 一次性启动引导

1. BMCBL 先完成每版本授权快照及**原地** DLL 校验（`BMCBL/plugins/<plugin_id>/game-mods/`），**不创建或复制 DLL 暂存文件**，然后创建游戏会话。使用操作系统加密安全随机源产生至少 **256 bit** 的 `session_secret`、随机 `session_id` 和单实例端点；不得复用旧启动秘密。
2. 为游戏会话建立 `0700` 的私有目录，启动描述符文件权限 `0600`，以安全创建、禁用符号链接/重解析逃逸、原子写入方式存放：`session_id`、`endpoint`、`transport`、随机 `session_secret`、过期时间和允许的目标游戏/运行器身份绑定材料。BMCBL 不接受插件提供的描述符路径。描述符最长存活至本次握手完成/超时；断开和异常时清理。不能仅靠 Unix 文件权限声称防御**同 UID 已被入侵**的进程。
3. BMCBL 通过使用**相同 WINEPREFIX/Proton 运行环境**的路径转换，将这个**描述符的 Windows 可见绝对路径**传给 BLoader。建议只把路径写进启动参数以外的私有启动环境变量 `BMCBL_VMOD_BOOTSTRAP`，绝不在环境变量/命令行中传 `session_secret`；同时验证 UMU/Proton 启动器不会清除该路径变量。若运行器隔离导致 Windows 侧无法打开此路径，则安全禁用虚拟 Mod，不扫描磁盘尝试发现其他会话。
4. BLoader 只有在**退出 loader lock 后**才打开、限长读取并关闭描述符。检查时效、文件类型/路径、协议版本和运行器关联；成功后及时清除内存中的敏感明文并由宿主删除启动描述符。不允许 XUser Bridge 或普通 Mod 读取/重用该秘密。游戏进程内的不可信 DLL 可读到进程内数据，因此仍不构成同进程沙箱。
5. 两端在发送 `hello/welcome/launch_plan` **之前**完成双向挑战认证：双方各生成独立的 256 bit 随机 nonce；用 `HMAC-SHA256(session_secret, domain || session_id || client_nonce || server_nonce || role)` 分别生成服务端与客户端证明，`domain` 为固定协议域分隔符（例如 `BMCBL-VMOD-IPC-V1`），`role` 分别为 `host`/`loader`；固定长度、固定顺序、常数时间比较。挑战/响应不得把原始秘密传输在连接上。**完整消息顺序、字节编码与测试向量必须由双方共享一份协议实现固定后，才能开启 Linux 传输**。
6. 握手上下文绑定当前启动会话、协议主版本、端点和游戏身份；只有认证成功的连接可发送应用协议帧。单连接、2 秒累计超时；拒绝重放/错误 nonce/错误 role/过期会话/多余连接，密钥和失败详情不写日志。
7. `127.0.0.1` 的传输不具备 Unix socket 的内核对端 PID 认证能力；安全性依赖私有引导材料和**密码学握手**，不是“localhost 就安全”。未来若应用消息可能包含敏感数据，应把两种 Linux 传输统一升级为经过审查的 **AEAD 加密会话**（例如有标准测试向量的 Noise PSK 模式），而非自行设计流加密。v1 明确禁止通过这个 RPC 传送 Xbox 登录令牌/隐私凭据。

#### 5.2.3 Linux 文件路径与生命周期

- BMCBL 用 Linux 原生路径核验 `BMCBL/plugins/` 下 DLL，使用**当前运行器与 Wine Prefix**将**插件根目录**转换为 Windows 可访问的绝对路径 `plugins_root`，不复制 DLL。必须验证该 Prefix/沙箱确实能访问原目录；不假定 `Z:` 一定存在或映射宿主根。
- BLoader 仅可按 `windows-x86_64` PE DLL 加载，使用同第 7 节的校验与依赖安全要求；Wine 的文件共享/重解析规则与原生 Windows 不完全等同，必须测试“校验后替换”的竞态。
- 运行中模块只依附当前游戏会话；Wine/Proton 进程退出或断链后，Linux 宿主取消 RPC、释放端点、清理**一次性连接描述符**并更新诊断。公共插件 DLL 保持原位置，不在游戏退出时删除。无法保证 DLL 在游戏退出前被强制卸载。
- Linux UWP/GDK 需要由实际 Proton-GDK 启动链验证 BLoader 注入/加载时序；如只能晚期注入，不可声称已支持 `preload`。根据实际能力声明 `phases`，可能仅有 `ready`。

#### 5.2.4 Linux 可执行时序

```text
BMCBL (Linux, native Rust)                  Minecraft (Wine/Proton PE, BLoader)
授权快照、DLL 预校验
建立一次性 IPC bootstrap（仅认证材料；无 DLL 复制）
监听 Unix Socket / 127.0.0.1:随机端口
使用当前 Wine Prefix 转换 bootstrap 路径与 BMCBL/plugins 根路径
启动 Minecraft，传入非秘密描述符路径 ──────> BLoader 退出 loader lock
                                           读取 bootstrap，连接对应端点
Linux 端点核验 + 双向 HMAC challenge <──────> 验证认证证明
hello/welcome → launch_plan ───────────────> BLoader 安全复验、执行阶段加载
插件 WASM → 宿主校验 → RPC ─────────────────> DLL BLoaderModEntryV1
插件 WASM ← 宿主过滤 ← response/event <────── DLL 原生回调
游戏退出 → 撤销转发、关闭 IPC、清理会话描述符（DLL 不动）
```

### 5.3 时序与阻塞预算

```text
BMCBL：插件/版本/授权快照 → 核验原位置 DLL → 创建平台传输端点
                                      ↓ 放行 Minecraft
BLoader：退出 loader lock → 完成平台端点验证/会话认证 → hello
BMCBL：校验目标 PID/进程 → welcome → launch_plan（一次性）
BLoader：校验计划和载荷 → plan_ack
       ├─ preload：在 OEP 前依既定 BLoader 阶段加载
       └─ ready：在 OEP 释放并满足 Process ready 后加载
BLoader：module_status / rpc_response / mod_event
BMCBL：按启动 session + 插件 generation 路由、撤销与诊断
```

- 启动计划必须在游戏放行前准备完成，**不能**在 OEP 关键路径等待网络、下载、插件 WASM 执行或磁盘解包。BLoader 禁止在 `DllMain` 或 Windows loader lock 下做 IPC、等待、线程加入、Mod 回调与加载链。
- 默认预算：连接与协议协商累计最多 **2 s**；单次收发读写操作须有独立截止时间；到期只放弃本次虚拟 Mod，不无限阻塞 Minecraft。后台 RPC 默认 **5 s**，可在以后通过显式能力协商调整。
- 宿主应区分计划成功接收与模块最终加载；`plan_ack` 不代表 `LoadLibraryExW` 或 ABI 初始化成功。

## 6. Wire Protocol v1（固定可互操作格式）

### 6.1 帧编码

每个帧为 `u32 little-endian` 的**载荷字节长度**，紧接该长度的 UTF-8 JSON 对象。长度不包含 4 字节头；v1 硬上限 **65,536 字节 JSON 载荷**，`hello` 双方还须协商更小上限，最终取最小值。拒绝零长度、超过上限、截断帧、非法 UTF-8、非 JSON 对象、重复 JSON 键、非法数字、未知 `type` 与必需字段缺失；收到协议级错误即关闭本次连接并撤销未完成请求。

- 所有消息有 `type`。除首次 `hello` 外，消息携带 `session_id`（标准 UUID 字符串）；`session_id` 必须精确匹配当前连接会话。`hello.transport` 必填，允许 `named-pipe`、`unix-stream` 或 `authenticated-loopback`，必须与实际建立的端点一致，不允许客户端自报后切换传输类型。
- `seq` 为本方向单调递增的 64 位正整数，用 **16 位小写十六进制字符串**编码（例如 `"0000000000000001"`）。同一连接禁止重复、倒序及回绕；不能直接用可能丢失精度的 JSON 数字表示 `u64`。
- `request_id` 与 `seq` 使用相同编码，但关联一个待处理 RPC；每一方向分别分配，不在同一未完成集合中复用。响应必须携带发起方的原 `request_id`。
- 未定义扩展字段、类型及枚举值不得被推断为已授权行为。v1 接收者拒绝未知控制字段；未来的可选扩展必须通过 `capabilities` 显式协商，新增不兼容语义升级主版本。
- JSON `body_b64` 统一采用 RFC 4648 标准 Base64（带必要 padding）；解码前后都执行长度限制。载荷是非特权业务字节，不得包含账号令牌；大文件/流式传输不属于 v1。

### 6.2 协商与启动计划示例

BLoader → BMCBL（连接建立并完成对应平台的端点验证与会话认证后）：

```json
{
  "type": "hello",
  "seq": "0000000000000001",
  "protocol_major": 1,
  "protocol_minor": 0,
  "target_pid": 12345,
  "transport": "named-pipe",
  "max_frame_bytes": 65536,
  "phases": ["preload", "ready"],
  "capabilities": ["module_abi_v1", "rpc_v1"]
}
```

BMCBL → BLoader（已核对 PID、进程实例和协议）：

```json
{
  "type": "welcome",
  "seq": "0000000000000001",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "protocol_major": 1,
  "protocol_minor": 0,
  "max_frame_bytes": 65536,
  "game_version": "1.26.60.5",
  "game_instance_id": "example-instance"
}
```

BMCBL → BLoader（`launch_plan` 仅允许发送一次，不接受游戏进程提出加载新模块的请求）：

```json
{
  "type": "launch_plan",
  "seq": "0000000000000002",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "plugins_root": "C:\\BMCBL\\plugins",
  "modules": [
    {
      "plugin_id": "example-plugin",
      "module_id": "example-mod",
      "plugin_generation": "47bb476c-e755-4719-b32c-f6a6c3fdc2c9",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "size_bytes": 102400,
      "platform": "windows-x86_64",
      "phase": "ready",
      "relative_path": "game-mods/example.dll"
    }
  ]
}
```

`plugins_root` 是 BMCBL 本次启动会话**认证后交付的公共插件根目录的 Windows 绝对路径**，即安装后的 `BMCBL/plugins`；Linux 上由 BMCBL 按当前 Wine/Proton Prefix 转换。`relative_path` 必须严格取自已验证的插件清单（如 `game-mods/example.dll`），不得是任意绝对路径。BLoader 只按 `plugins_root/<plugin_id>/<relative_path>` 组成 DLL 路径，再检查目录归属、重解析点、文件 SHA-256 与架构；**不复制、不移动文件**。普通 Mod/WASM 不能直接设定 `plugins_root` 或模块路径。

BLoader → BMCBL（`plan_ack` 对每个条目报告是否**接受调度**，拒绝项给出确定的 `error_code`）：

```json
{
  "type": "plan_ack",
  "seq": "0000000000000002",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "accepted": ["example-plugin/example-mod"],
  "rejected": []
}
```

两端对计划施加上限：单游戏会话最多 **256 个**条目、唯一 `plugin_id/module_id`、单插件至多 32 个、所有 ID 和十六进制摘要按清单规则校验；`plugins_root` 必须是有效绝对路径且以 `plugins` 目录为根，`relative_path` 必须在当前插件的 `game-mods/` 目录内。协议帧仍限 64 KiB，超出时**拒绝整个计划**，不能截断。单模块文件不兼容时报告独立失败状态，不波及其他模块。

### 6.3 状态、请求、响应、事件

BLoader 发出的 `module_status` 结构：

```json
{
  "type": "module_status",
  "seq": "0000000000000003",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "module": "example-plugin/example-mod",
  "state": "loaded",
  "phase": "ready",
  "rpc_available": true,
  "error_code": null
}
```

- 非终态：`planned`、`validating`、`loading`。终态：`loaded`、`loaded_no_rpc`、`rejected`、`unsupported`、`verify_failed`、`load_failed`、`skipped`。
- `loaded` 必须表示操作系统加载成功且可用的 `BLoaderModEntryV1` 已初始化；`loaded_no_rpc` 表示 DLL 加载成功但没有注册业务 ABI；不得将“已计划/已尝试/已找到 HMODULE”误报为可 RPC。
- 失败时使用机器可判别 `error_code` 和安全的简要 `detail`；由 BMCBL 关联正确游戏会话，展示在管理页和崩溃诊断中。禁止把未经脱敏的认证材料或任意 Mod 提供的日志正文直接作为可信 HTML。

BMCBL → BLoader（仅授权插件可通过 BMCBL 宿主 API 创建；原生侧由已登记的模块回调处理）：

```json
{
  "type": "rpc_request",
  "seq": "0000000000000003",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "module": "example-plugin/example-mod",
  "plugin_generation": "47bb476c-e755-4719-b32c-f6a6c3fdc2c9",
  "request_id": "0000000000000001",
  "method": "example.ping",
  "body_b64": "cGluZw=="
}
```

BLoader → BMCBL（仅可对正在等待的 `request_id` 回应；失败用统一错误码）：

```json
{
  "type": "rpc_response",
  "seq": "0000000000000004",
  "session_id": "e64a40d8-47d4-49dc-85bb-68588a40ef1b",
  "module": "example-plugin/example-mod",
  "plugin_generation": "47bb476c-e755-4719-b32c-f6a6c3fdc2c9",
  "request_id": "0000000000000001",
  "ok": true,
  "body_b64": "cG9uZw==",
  "error_code": null
}
```

- `rpc_cancel`：与 `rpc_request` 采用相同的会话、模块、generation 和请求 ID，用于**尽力**取消。取消成功/超时后宿主不再向已终止的插件传递迟到结果；**不能保证**撤回已在原生 DLL 中产生的副作用。
- `mod_event`：BLoader → BMCBL，包含 `module`、`plugin_generation`、`event_id`（同 `seq` 格式）、`name` 和 `body_b64`；只能路由到该模块的拥有者，不允许原生消息指定其他插件 ID。
- `error`：包含 `error_code`、可选 `request_id` 和脱敏 `detail`。`shutdown`：宿主通知会话停止接受新 RPC 并撤销路由；管道意外断开具有相同的路由撤销效果，但不隐式卸载 DLL。
- v1 仅由 BMCBL 主动发起插件→Mod 的 RPC；Mod→插件是响应和事件。BLoader 不能把来自游戏进程的原始消息转换为启动器特权命令。

### 6.4 统一错误与资源配额

建议的稳定 `error_code`：

| 类别 | 代码 |
| --- | --- |
| 认证/协议 | `UNAUTHORIZED_SESSION`、`PROTOCOL_MISMATCH`、`INVALID_FRAME`、`INVALID_MESSAGE` |
| 计划/权限 | `INVALID_PLAN`、`PERMISSION_DENIED`、`MODULE_NOT_FOUND`、`GENERATION_EXPIRED` |
| 载荷/加载 | `PLATFORM_UNSUPPORTED`、`PHASE_UNSUPPORTED`、`HASH_MISMATCH`、`FILE_UNSAFE`、`DEPENDENCY_UNSAFE`、`LOAD_FAILED`、`ABI_UNSUPPORTED` |
| 运行中 | `MODULE_UNAVAILABLE`、`BUSY`、`TIMEOUT`、`CANCELLED`、`SESSION_CLOSED`、`INTERNAL_ERROR` |

每模块最多 **32** 个未完成请求、**64** 个待派发请求；全会话最多 **256** 个未完成请求、累计待派发数据不超过 **4 MiB**。达到上限立即返回 `BUSY`，不得无限排队。读取循环、原生模块调用和回调投递必须解耦；不得在持有全局状态锁时调用不可信 DLL。每个模块可独立失败，不得因单个卡死模块阻塞控制帧、状态上报或其他模块的队列。超时与断开须清理映射、完成等待任务并停止向旧 generation 投递。

## 7. 公共插件 DLL 的原地加载与安全要求

**语义**：虚拟 Mod 不产生新的 DLL 副本，始终从 BMCBL 已安装的 `plugins/<plugin_id>/game-mods/` 文件路径装载。每个游戏版本可以有不同授权快照，但各版本引用同一 DLL；不是内存加载、独立进程隔离或 Wine 原生 `.so`。

1. **BMCBL** 将 `plugins_root` 固定为当前插件注册表的安装根（Windows 的原生绝对路径，Linux 的真实目录需转换成当前 Wine/Proton Prefix 的 Windows 绝对路径），仅把已安装、启用且按当前版本 `SHA-256` 授权的 Mod 纳入计划；`relative_path` 来自已经核验的插件清单，禁止让 WASM 调用方传入 DLL 绝对路径。
2. **路径合成**：BLoader 用 `plugins_root / plugin_id / relative_path` 合成绝对路径。要求 `relative_path` 以 `game-mods/` 开头、以 `.dll` 结尾、只含安全路径组件；禁止盘符、UNC、设备路径、`..`、`.`、反斜杠、ADS、保留设备名称和空组件。规范化后 DLL 必须仍位于**对应插件目录**；不得仅凭字符串包含 `plugins` 就认定路径可信。
3. **原地复验**：在 `LoadLibraryExW` 前拒绝插件目录内任何重解析/符号链接逃逸，核对实际文件类型、文件大小、PE x64 DLL 头、授权的 `SHA-256`。Windows 使用不允许写入/删除共享的打开方式，在复验到装载窗口内保持文件句柄；Wine 的文件共享语义和文件替换竞态必须单独实测。若无法提供可靠的加载期字节稳定性，拒绝宣称具备 TOCTOU 防护保证。
4. **装载**：使用校验后的完整绝对路径调用 `LoadLibraryExW`，限制依赖搜索目录（例如经过验证的 `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`）；不退回当前目录或通过任意文件名搜索。查询模块实际加载位置，不匹配则失败并报告。仅核验主 DLL 不保证依赖安全；额外非系统依赖必须另有白名单/摘要校验，不应默认信任目录内的全部依赖。
5. **更新与生命周期**：公共插件 DLL 可能同时被多个游戏进程使用。BMCBL 更新、卸载插件时应避开活跃游戏会话（延迟更新/拒绝覆盖已加载文件），防止 Windows 共享冲突及 Linux/Wine 下的路径替换竞态；禁用授权只影响后续启动，不应强制卸载已执行的 DLL。游戏退出不删除公共插件文件。

只有传输已认证、权限快照通过、真实路径及载荷复验成功后才进入对应加载阶段。不要因为某个 Mod 校验失败而阻塞 Minecraft 的正常启动。

参考：
- https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-security
- https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw

## 8. 原生通信 ABI（v1 新增接口；不影响旧 DLL）

BLoader 通过**可选**导出符号 `BLoaderModEntryV1` 识别支持 RPC 的原生 DLL。旧式 DLL 未导出时可按已授权计划加载，但状态为 `loaded_no_rpc`；不应调用任意未知导出、自动搜索 Hook 或解释其私有 `manifest.json`。

以下是需要同步实现的 C ABI 契约，Windows x64 使用 C ABI；Rust 侧须采用 `#[repr(C)]`、`extern "C"` 和对应的指针生命周期约束：

```c
#include <stdint.h>

typedef struct BLoaderHostV1 {
    uint32_t struct_size;
    uint32_t abi_version;       /* 1 */
    void *host_context;
    int32_t (*reply)(void *host_context, uint64_t request_id,
                     const uint8_t *bytes, uint32_t length);
    int32_t (*emit)(void *host_context, const uint8_t *name, uint32_t name_length,
                    const uint8_t *bytes, uint32_t length);
} BLoaderHostV1;

typedef struct BLoaderModV1 {
    uint32_t struct_size;
    void *mod_context;
    int32_t (*on_request)(void *mod_context, uint64_t request_id,
                          const uint8_t *method, uint32_t method_length,
                          const uint8_t *bytes, uint32_t length);
    void (*on_close)(void *mod_context);
} BLoaderModV1;

/* Export name: BLoaderModEntryV1; return 0 on success. */
int32_t BLoaderModEntryV1(const BLoaderHostV1 *host, BLoaderModV1 *out_mod);
```

- `struct_size` 供 ABI 尺寸校验；v1 要求精确版本和至少 v1 所有必需字段，指针非空；未知 ABI 拒绝 RPC 注册并报告 `ABI_UNSUPPORTED`。`BLoaderModEntryV1` 必须在 `LoadLibraryExW` 返回后、loader lock 外调用，不能在 `DllMain` 内做同步回调。
- `reply` 的 `request_id` 是 BLoader 派发的有效未完成请求，且绑定当前模块；`emit` 的事件名称只允许受限 UTF-8 字节。BLoader **通过内部保存的归属上下文**填入会话/模块/generation，不采信 DLL 传入的任意“目标插件”。
- 传入缓冲区只在函数调用期间有效；若异步消费，模块必须自行复制。BLoader 的回调可在模块线程调用，因此需要线程安全、边界检查、非阻塞入队和关闭状态处理。回调返回 `0` 表示接受入队；非零为拒绝/忙/已关闭，不能跨 FFI unwind 或抛出 C++ 异常。
- `on_request` 返回 `0` 表示接受并会异步答复；负值表示立即拒绝，BLoader 应终结该 `request_id`。回调必须在专用有界执行器上调度，不能把不可信代码放在 IPC I/O 线程或启动主线程。`on_close` 只在显式安全停机阶段、确认不再有并发回调时调用；不要从 `DllMain(DLL_PROCESS_DETACH)` 调用或依赖其完成。
- DLL 的 C ABI 本身**不是沙箱**。在同一游戏进程中，其他恶意原生代码仍可能干扰内存与回调；`module` 归属是由 BLoader 分派时建立的路由约束，不应宣传为进程内部的安全隔离。

## 9. 插件 Host API、撤销与可观测性

- BMCBL 应提供受限的插件宿主能力 `game_mod.call(module_id, method, bytes)`，只接受**调用插件自己**清单中、当前游戏版本已授权且已加载、`rpc_available=true` 的模块。插件不能传 `plugin_id`、路径、PID 或任意 `session_id`。运行时校验必须和安装/授权校验**同时存在**，不能只由 UI 开关守卫。
- 宿主为每次调用附加当前 `session_id`、`plugin_generation` 与全局唯一未完成请求 ID；按模块 ID 双向关联。插件重载/禁用、会话退出、进程崩溃、版本权限撤销或通道异常时，停止路由、关闭等待、忽略旧响应。取消不卸载已执行 DLL。
- 对外错误按稳定 `error_code` 呈现；诊断记录必须有 `session_id`、目标游戏 PID、`plugin_id/module_id`、阶段、操作、耗时、错误码、已脱敏详情。拒绝记录原生消息体、未经脱敏的路径、令牌、Pipe 私有数据、Xbox 凭据；日志输出有速率限制。
- BLoader 与 XUser Bridge、Cosmetics、LeviLamina、PreLoader 等现有链路保持模块化和职责隔离；仅可抽取通用无认证业务语义的底层读写、长度前缀、取消等基础设施，不共享会话令牌或特权回调。
- UI 至少分别显示：插件声明数、已授权数、当前启动计划数、已成功加载数、无 RPC ABI 数和失败数，不得以“插件已启用”推断“Mod 已运行”。

## 10. 必须通过的安全与联调测试

| 测试 | 通过标准 |
| --- | --- |
| 两个 Minecraft 版本共享一份插件 DLL | 独立授权；相同 SHA-256；会话互不串扰 |
| 插件禁用、卸载、升级、重新启用及 `generation` 更新 | 新启动计划正确变化，旧请求/事件不能回流 |
| 修改 DLL、摘要不符、替换同名 DLL | 禁止加载且报告 `HASH_MISMATCH` 或 `FILE_UNSAFE` |
| 同名不同 DLL、依赖劫持、符号链接/接合点、更新时文件替换竞态 | 不从未授权来源加载；公共插件目录归属、路径与文件身份复验 |
| 伪造 PID、复用旧 session、抢占管道、同用户其他进程连接 | 端点验证失败，计划与 RPC 不泄露 |
| 超长/零长/截断帧、重复 JSON 键、未知 type、倒序 seq、错误 generation | 受控拒绝，内存/CPU 有界，不崩溃 |
| 无 Mod、全未授权、BLoader 缺失、不同主版本或握手超时 | 原游戏可继续启动，虚拟 Mod 明确标记跳过 |
| `preload` / `ready` 与 PreLoader/LeviLamina 并存 | 正确时序，无 loader-lock 等待，无重复加载 |
| 请求 5 s 超时、断链、取消、事件洪泛、模块阻塞 | 及时完成等待、背压生效，不拖死其他模块 |
| 多实例、多插件同名 `module_id`、最多 256 个条目 | 完整逻辑 ID 隔离，超限拒绝 |
| Windows 原生、Wine、Proton、UMU、GDK/UWP 各启动方式 | 有端点认证与真实启动阶段的验证证据才宣告支持，否则安全降级 |
| Linux AF_UNIX 可用、不可用、路径不可见、loopback 不通、错误启动密钥 | 按已验证能力选择安全传输，认证失败或不可达时跳过虚拟 Mod |

## 11. 落地顺序与验收门槛

1. **BMCBL**：启动时重验授权与 DLL 字节、固定插件清单/授权快照，发送真实安装目录的 `plugins_root` 和模块 `relative_path`；先实现宿主 IPC、消息模型和错误规范。**不复制 DLL、不创建额外 Mod 目录**，不要向插件开放无权限的 `native.load` 或任意路径调用。
2. **BLoader**：新增独立虚拟 Mod 模块（建议 `src/virtual_mods/{protocol,transport,session,artifact,loader,rpc,abi}.rs`），在现有引导流程中插入一次性计划获取，并复用**验证过的**阶段调度/依赖诊断，不重写 PreLoader/LeviLamina 的归属逻辑。
3. **双端**：实现 `hello`、`welcome`、`launch_plan`、`plan_ack`、`module_status` 与失败路径；完成 Windows 端原地路径验证、共享文件句柄和更新替换竞态测试，随后才允许加载真实 DLL。
4. **双端 + 插件 API**：实现可选 `BLoaderModEntryV1`、RPC/事件、配额和 generation 撤销；完善管理页、诊断及模组加载结果展示。
5. **跨平台**：逐条验证 Wine/Proton 安全端点、进程关联和路径转换，再声明能力支持。无安全验证的目标平台**保持关闭**。

**完成定义**：不能只凭“管道连接成功、DLL 返回 HMODULE”标记支持。必须具备启动快照的授权正确性、传输端点身份、完整载荷/依赖校验、正确阶段、可回溯的最终状态、RPC 所属插件隔离（在宿主路由层）、取消与断线处理及对应负向测试。

参考文档：
- Windows Named Pipe 安全： https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
- Windows 端点进程身份： https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid
- DLL 安全装载： https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-security

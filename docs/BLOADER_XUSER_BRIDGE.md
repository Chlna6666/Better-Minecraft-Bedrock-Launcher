# BLoader 0.2.7 XUser Bridge

BMCBL 的 Win32 GDK 启动链路支持通过 BLoader 0.2.7 切换 Xbox 用户。

```text
BMCBL
  → 为目标 Minecraft PID 创建一次性安全管道
  → BLoader 验证会话、PID、有效期和摘要
  → 仅接管系统 xgameruntime.dll!QueryApiImpl
  → CLSID_XUserImpl 返回内置 Rust XUser / XAsync
  → XUserGetTokenAndSignatureAsync 返回 Token + Xbox PoP Signature
  → 其他 XGameRuntime 接口继续调用微软官方实现
```

## 启用条件

只有同时满足以下条件时才启用自定义 XUser：

- 启动目标是 Win32 GDK Minecraft；
- BMCBL 当前存在有效 Xbox 登录会话；
- 当前 Minecraft PID 对应的一次性管道验证成功；
- BLoader 成功从 `System32` 定位微软官方 `xgameruntime.dll`；
- `QueryApiImpl` Hook 安装成功。

没有有效会话时，BLoader 不主动加载 Runtime、不创建 MinHook、不修改 `QueryApiImpl`，游戏继续使用微软官方登录。

## 用户切换

BMCBL 选择不同 Xbox 会话后，每次启动都会为新的 Minecraft PID 创建独立的一次性管道。BLoader 只消费本次启动对应的会话，因此不会复用上一次游戏进程的账号状态。

当前只支持 Win32 GDK。UWP/AppContainer 版本不使用该链路。

## 安全边界

- Token、设备密钥和预认证载荷不通过环境变量、命令行、注册表或普通临时文件传递；
- 管道绑定目标 PID，并验证发送端与接收端进程；
- 会话载荷带有效期、长度和 SHA-256 校验；
- BLoader 在加载普通 Mod 前消费并清零原始会话缓冲区；
- 日志只显示经过清理的 Xbox Gamertag、系统 Runtime 路径和 Hook 状态；
- 日志不得输出 XUID、Token、私钥、Authorization、Signature、请求正文或原始管道载荷。

同进程恶意 Mod 仍可能主动扫描 Minecraft 进程内存或 Hook XUser 返回缓冲区。当前设计防止的是环境变量、磁盘、注册表、非目标进程和普通 Mod 抢先读取等泄漏路径，不能对同进程恶意代码提供绝对隔离。

## BLoader 资源

BMCBL 应使用：

```text
BLoader version: 0.2.7
BLoader.dll size: 1,344,000 bytes
SHA-256: de046e7ef2518856dbd04ca8786b2234c593aa2c51a8a76913270afff8257344
```

仓库内版本元数据位于 `assets/bin/BLoader.version`。

## 诊断日志

有效会话下应出现：

```text
XUser Bridge 入口已执行
已从 BMCBL 安全一次性管道接收并验证 Xbox 会话
系统原生 xgameruntime.dll 已就绪
已定位系统原生 QueryApiImpl
XUser Bridge 已启用；仅接管官方 QueryApiImpl
QueryApiImpl Hook 已首次命中
QueryApiImpl 已请求 CLSID_XUserImpl；返回 BLoader 内置 Rust XUser
```

无会话时应出现：

```text
XUser Bridge 入口已执行；未检测到 BMCBL 安全一次性管道；
不主动加载系统 Runtime、不安装 QueryApiImpl Hook；
继续使用微软官方 XUser 登录
```

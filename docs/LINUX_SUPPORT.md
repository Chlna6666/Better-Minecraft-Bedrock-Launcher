# BMCBL Linux 支持设计

## 目标

BMCBL 的 Linux 版本复用配置、下载、任务、插件和绝大多数 GPUI 界面，只替换操作系统集成与游戏启动后端。Linux 不提供 UWP 注册或启动能力，Minecraft Bedrock 游戏进程通过用户选择的 Wine/Proton 运行器启动。

BedrockBoot 的 [Linux 项目](https://github.com/Round-Studio/BedrockBoot/tree/2.0-develop/src/BedrockBoot.Linux) 和 [Proton 项目](https://github.com/Round-Studio/BedrockBoot/tree/2.0-develop/src/BedrockBoot.Proton) 仅作为平台隔离、运行器目录与 Proton 子进程环境的实现参考。BMCBL 不复制其页面或业务功能，仍沿用现有任务、下载、配置和 GPUI 架构。

首个可发布版本需要满足：

- Wayland 与 X11 均能创建窗口、提交首帧、恢复被遮挡窗口并正确退出；
- Linux 构建不编译 Windows UWP/AppX 系统操作、注册表和 Win32 注入实现；
- 启动器始终以普通桌面用户运行；
- 用户目录内的 Wine/Proton 下载、安装和游戏运行不请求管理员权限；
- 只有安装系统软件包时才通过 Polkit 请求一次性授权；
- 依赖检测和安装失败能够传递到任务系统和 UI，不静默退出。

## 当前实现状态

Linux 现在运行与 Windows 共用的 `lib`、`startup`、`app` 和 GPUI 主窗口，不再使用临时测试窗口。应用层会把自动渲染后端解析为 Linux 的 Vulkan 后端，并已通过真实 Wayland 和 X11/XWayland 主窗口首帧验证。

Linux 构建在父模块层跳过 UWP/AppX 注册、AUMID 启动、注册表、Windows 开发者模式、Win32 注入和 Windows 依赖安装代码。AppX 清单与 PE 解析工具仍作为通用的下载/版本扫描能力编译，但不编译其 WinRT 包查询函数。

已实现第一版 Linux 游戏启动任务：优先读取 `BMCBL_PROTON_RUNNER`，然后检测 Steam 兼容工具、`proton` 和 `wine`；为每个实例建立独立 XDG prefix，只向游戏子进程注入 Proton/Wine 环境，并把 stdout、stderr 和启动错误接入现有任务系统。

root 主进程会在创建 GPUI 之前被拒绝。系统依赖检测、安装计划预览和 Polkit helper 仍是后续工作；当前未找到 runner 时会在启动任务中显示可操作错误，不会静默失败。

## 平台边界

应用层保留一个跨平台入口，在模块声明与启动任务层按目标平台选择实现：

```text
src/main.rs
  -> src/startup.rs
  -> src/app.rs
  -> src/core/minecraft/launcher/mod.rs
       -> task.rs (Windows UWP/GDK/Win32)
       -> task_linux.rs (Linux Proton/Wine)
  -> src/ui/hooks.rs
       -> use_launcher.rs (Windows 启动前置)
       -> use_launcher_linux.rs (Linux runner 任务)
```

`src/app.rs` 仍负责所有平台的 GPUI 初始化。窗口、路由、主题、国际化和任务系统不得因 Linux 而复制一份。后续新增的平台能力只暴露应用所需接口，不向公共业务层泄漏 Win32 handle、注册表类型、Polkit 或发行版包管理器细节。

建议的平台能力接口包含：

- 当前平台和可用启动后端；
- 单实例与前台激活；
- 文件关联和打开路径；
- 系统信息与内存信息；
- 游戏运行环境检测；
- 受控的系统依赖安装请求。

不支持的能力应以 `UnsupportedCapability` 返回到 UI。不要提供看似成功的空实现。

## 条件编译

Windows-only 模块在其父模块声明处使用 `#[cfg(target_os = "windows")]`，对应依赖放入 `target.'cfg(windows)'.dependencies`。Linux 代码不依赖 `windows` crate，也不使用覆盖真实 Windows API 的 stub crate。

Linux 需要跳过编译的实现至少包括：

- UWP/AppX 注册、移除、AUMID 启动与 UWP 调试；
- Windows 注册表、开发者模式和 Windows App SDK；
- Win32 注入、鼠标锁和窗口查找；
- Windows 单实例 mutex 与文件关联；
- 只适用于 Windows 的更新器行为。

公共数据模型可以保留 UWP 枚举值，以便读取已有配置，但 Linux UI 必须根据平台能力隐藏或禁用相应操作。持久化格式不能因目标平台改变而无法解析。

## Linux 游戏运行后端

Linux 使用明确的 `LinuxProton` 后端，不把 Proton 命令拼接进 Windows UWP 启动函数。

运行器目录遵循 XDG：

```text
$XDG_CONFIG_HOME/bmcbl/settings.toml
$XDG_DATA_HOME/bmcbl/runners/<runner-id>/
$XDG_DATA_HOME/bmcbl/prefixes/<instance-id>/
$XDG_CACHE_HOME/bmcbl/downloads/
$XDG_STATE_HOME/bmcbl/logs/
$XDG_STATE_HOME/bmcbl/diagnostics/
```

未设置对应变量时分别回退到 `~/.config`、`~/.local/share`、`~/.cache`
和 `~/.local/state`。该布局仅用于 Linux；Windows 继续使用可执行文件旁的
`BMCBL` 目录。旧版 Linux 的 `~/.local/share/bmcbl/config/settings.toml`
会在新配置不存在时复制到 XDG 配置目录，原文件保留作为回退。

环境变量按单个子进程设置，不修改 BMCBL 全局环境：

- `STEAM_COMPAT_DATA_PATH` 指向实例独立 prefix；
- `STEAM_COMPAT_CLIENT_INSTALL_PATH` 来自用户配置或已检测的 Steam 路径；
- runner 要求的兼容变量由 runner profile 生成；
- `LD_LIBRARY_PATH` 仅在运行器确实要求时设置，不覆盖用户已有值；
- 命令和参数使用 `std::process::Command::arg`，不经过 shell。

下载后的运行器先校验摘要和解压路径，再以原子目录替换完成安装。删除、覆盖和迁移 prefix 必须是显式用户操作。运行器及游戏进程始终属于当前普通用户。

## 依赖检测与安装

检测结果使用结构化状态，而不是一个总的布尔值：

```text
Ready
MissingUserRunner
MissingSystemPackages
UnsupportedDistribution
PermissionRequired
InstallInProgress
Failed
```

检测分三层：

1. Vulkan、Wayland/X11 和音频等启动器运行条件；
2. Wine/Proton runner、Steam runtime 和 prefix 等用户级条件；
3. 发行版提供的 32 位图形库、音频库等系统级条件。

用户级条件由 BMCBL 自己下载或修复，不提权。系统级安装优先使用 PackageKit/Polkit；没有 PackageKit 时，发行版适配器生成固定的包管理器参数，并通过 `pkexec` 启动一个最小 helper。helper 只接受枚举化操作和经过校验的软件包标识，禁止接收任意 shell 字符串。

UI 在授权前必须展示：

- 缺少哪些依赖；
- 将调用哪个系统服务或包管理器；
- 将安装哪些软件包；
- 哪些操作不需要管理员权限；
- 取消后仍可使用哪些启动器功能。

授权取消、认证失败、包管理器锁冲突、网络失败和不受支持的发行版都返回可重试错误。BMCBL 主进程不得通过 `sudo` 或 `pkexec` 重启自身。

## GPUI Linux 验证

GPUI 的最小窗口测试必须在 BMCBL 主窗口接入前通过。测试窗口使用不透明背景、可见文本和固定 Vulkan 后端，并记录：

- 选择的 compositor 和 GPU adapter；
- 窗口创建结果；
- 首帧是否进入 `NovaRenderer::draw`；
- swapchain present 是否成功；
- 窗口遮挡、最小化、恢复和 resize 后是否再次 present。

至少覆盖以下矩阵：

| 会话 | GPU | 场景 |
| --- | --- | --- |
| Wayland | AMD/Intel RADV 或 ANV | 首帧、resize、遮挡恢复 |
| X11/XWayland | AMD/Intel | Expose、resize、最小化恢复 |
| Wayland | llvmpipe | 明确提示软件渲染或可接受的降级 |
| 无显示会话 | 任意 | 返回可理解的错误，不进入空白 headless 窗口 |
| root | 任意 | GPUI 初始化前拒绝，并说明正确授权方式 |

## 实施顺序

1. 完善用户级 runner 安装、版本管理与校验。
2. 实现依赖检测、安装计划预览和 Polkit helper。
3. 为单实例、更新器与发行包补齐 Linux 平台行为。
4. 接入设置页、Linux 启动前置页与国际化。
5. 在 CI 和发布流程持续验证 Windows 与 Linux。

每一步都应保持 Windows 构建可用，并在 CI 中分别执行 Windows 与 Linux 的 `cargo check`。Linux 发布前还需要真实 Wayland/X11 的运行时 smoke test，单独的交叉编译不能证明窗口能够显示。

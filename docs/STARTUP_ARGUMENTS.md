# 启动参数

启动参数是**启动程序时**传给 `BMCBL.exe` 的参数，用于桌面快捷方式、文件关联、更新器子进程
和内部辅助进程。它与终端 CLI 是两套接口：

- 启动参数：进程创建时传入，面向操作系统集成；见本文。
- 终端 CLI：用户在终端输入的子命令；见 [CLI](CLI.md)。

同一份程序根据第一个参数自动区分：`launch`、`import`、`list`、`info`、
`help` 或帮助/版本选项属于终端 CLI；`--launch` 等启动参数走本文接口。

## 1. 规范参数

```text
BMCBL.exe --launch <VERSION> [--silent | --gui] [--game-args <ARGS>]
BMCBL.exe --import <FILE>
BMCBL.exe --updater <SRC> <DST> [TIMEOUT]
BMCBL.exe --terminal-host <COMMAND> <RESPONSE>
BMCBL.exe
```

| 参数 | 说明 |
| --- | --- |
| `--launch <VERSION>` | 启动指定版本。未提供 `--silent/--gui` 时遵循实例的“快捷方式静默启动”设置。 |
| `--silent` | 不打开主窗口，直接启动游戏。 |
| `--gui` | 打开主窗口并自动启动游戏。与 `--silent` 互斥；已有实例运行时提示后退出；首次运行会等待协议/新手引导完成后再继续。 |
| `--game-args <ARGS>` | 覆盖传给游戏的启动参数；只能与 `--launch` 一起使用。按命令行字符串传递，平台按空白/引号拆分；编辑器模式下优先于自动生成的 Deeplink。 |
| `--import <FILE>` | 导入受支持的文件并打开启动器。 |
| `--updater <SRC> <DST> [TIMEOUT]` | 内部使用：更新器子进程。`TIMEOUT` 默认 60 秒。 |
| `--terminal-host <COMMAND> <RESPONSE>` | 内部使用：Windows Terminal 控制台宿主。 |
| 无参数 | 打开启动器主窗口。 |

版本解析和 `--launch` 的 `VERSION` 规则与终端 CLI 的 `launch` 相同：先匹配版本目录名，
再匹配显示名或游戏版本号。

示例：

```powershell
BMCBL.exe --launch "1.21.0 stable" --silent
BMCBL.exe --launch "1.21.0 stable" --gui
BMCBL.exe --import "C:\Downloads\pack.mcpack"
BMCBL.exe --updater "C:\temp\updater.exe" "C:\Games\BMCBL.exe" 60
```

## 2. 兼容参数（0.3.0 起标记弃用）

以下旧参数在 0.3.0 之前保持可用，0.3.0 起标记弃用；新调用应使用规范参数。

| 旧参数 | 等价规范参数 | 备注 |
| --- | --- | --- |
| `--launch-version <VERSION>` | `--launch <VERSION>` | 语义相同。 |
| `--import-file <FILE>` | `--import <FILE>` | 旧参数遇到不支持的类型时打开主窗口。 |
| `--run-updater <SRC> <DST> <TIMEOUT>` | `--updater <SRC> <DST> <TIMEOUT>` | 超时必填；非数字超时回退为默认 60 秒。 |
| 直接传入 `<FILE>` | `--import <FILE>` | 系统“打开方式”；不支持的类型打开主窗口。 |

`--launch-version` 与 `--launch`、`--import-file` 与 `--import`、
`--run-updater` 与 `--updater` 不能同时使用。`--silent`、`--gui`、
`--game-args` 只能与启动模式一起使用。

0.2.x 的内部调用 `run-updater`、`terminal-host` 子命令以及 `--bmcbl-terminal-host` 会被
重写为 `--updater` / `--terminal-host`，在 0.3.0 前保持可用。

## 3. 系统集成

- 桌面快捷方式：`BMCBL.exe --launch "<版本目录>"`。是否显示窗口由实例设置决定。
- 文件关联：`BMCBL.exe --import "%1"`，只注册受支持的扩展名；关联版本号变更时在下次启动
  刷新注册表项。
- 更新器：复制当前可执行文件后以 `--updater <源> <目标> <超时>` 启动。
- Windows Terminal 宿主：以 `--terminal-host <command> <response>` 启动轻量辅助进程。

## 4. 与终端 CLI 的差异

| 项目 | 启动参数 | 终端 CLI |
| --- | --- | --- |
| 触发方式 | 快捷方式、文件关联、更新器、内部进程 | 终端输入子命令 |
| `--launch` 默认窗口 | 遵循实例设置 | 默认静默 |
| JSON 输出 | 不支持 | `launch --json` 支持 |
| `list` / `info` | 不支持 | 支持 |
| 更新器/终端宿主 | 内部参数 | 不暴露 |

## 5. 平台说明

Linux 上 `--launch` 和 `--import`（包括 GUI 与静默模式）会拒绝 root 用户；
`--updater` 和 `--terminal-host` 不受此检查。

## 6. 退出码

- 启动参数解析失败：`2`。
- 启动、导入或更新失败：`1`；更新器子进程失败固定为 `2`。
- 正常启动（GUI 或静默启动完成）：`0`。

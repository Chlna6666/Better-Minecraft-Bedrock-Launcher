# 终端命令行接口

BMCBL 提供两套独立的入口：

- **终端 CLI**：本文描述。用户在终端输入 `BMCBL launch`、`BMCBL list` 等子命令。
- **启动参数**：快捷方式、文件关联、更新器在启动程序时传入的参数，见
  [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.md)。

同一份 `BMCBL.exe` 根据第一个参数自动区分两者：第一个参数是 `launch`、`import`、
`list`、`info`、`help` 或帮助/版本选项时按终端 CLI 解析；否则按启动参数解析。
没有参数时打开主窗口。

## 1. 命令

```text
BMCBL launch [VERSION] [OPTIONS]
BMCBL import <FILE>
BMCBL list [--json]
BMCBL info [--json]
BMCBL help
```

`BMCBL --help` 输出终端命令帮助，`BMCBL --version` 输出构建版本。

### 1.1 launch

终端启动默认**静默**：不打开主窗口，直接启动游戏。需要界面时显式使用 `--gui`。

| 参数 | 说明 |
| --- | --- |
| `VERSION` | 可选。版本目录名、显示名或游戏版本号；省略时使用排序后的第一个版本（优先总启动次数，其次是版本号）。 |
| `--silent` | 静默启动；与默认行为相同，保留用于显式表达意图。 |
| `--gui` | 打开主窗口，并走正常 UI 启动流程（启动前检查、弹窗和错误提示）。已有实例运行时提示后退出；首次运行尚未完成协议/新手引导时，会在完成后自动继续。 |
| `--game-args <ARGS>` | 覆盖传给游戏的启动参数；按命令行字符串传递，平台按空白/引号拆分。编辑器模式下该参数优先于自动生成的 Deeplink。 |
| `--json` | 以 JSON 输出启动结果；与 `--gui` 互斥。 |

`--silent` 与 `--gui` 互斥。终端 CLI 不使用实例的“快捷方式静默启动”设置；该设置只影响
启动参数。

版本解析顺序：

1. 版本目录名精确匹配（Windows 不区分大小写，路径语法会归一化）。
2. 显示名或游戏版本号匹配（不区分大小写）。
3. 匹配到多个实例时返回错误，并提示改用版本目录名。

示例：

```powershell
BMCBL launch "1.21.0 stable"
BMCBL launch --gui
BMCBL launch --game-args "-server 127.0.0.1" --json
```

静默启动命令会等待启动任务进入终态（成功拿到游戏 PID 或失败）后退出；游戏进程不会随
命令退出。需要等待游戏退出的脚本应自行跟踪输出中的 PID。`--gui` 会保持窗口事件循环，
直到主窗口关闭。

### 1.2 import

导入资源包、世界、附加包或安装包，并打开导入窗口。

支持：`.mcpack`、`.mcworld`、`.mcaddon`、`.mctemplate`、`.zip`、
`.appx`、`.msixvc`。其他扩展名会返回用法错误。

```powershell
BMCBL import pack.mcpack
```

### 1.3 list

列出本地版本。每行四列：版本目录名、显示名、游戏版本号、类型。排序规则与主界面一致。

```powershell
BMCBL list
BMCBL list --json
```

JSON 输出为数组，元素包含 `folder`、`name`、`version`、`kind`、`path`、
`sessions`。

### 1.4 info

显示构建信息、可执行文件位置、数据目录和版本目录。

```powershell
BMCBL info
BMCBL info --json
```

## 2. 输出与退出码

- `0`：命令成功。
- `1`：运行期失败，例如版本不存在、配置读取失败、启动任务失败。
- `2`：命令行用法错误（clap），或更新器子进程失败。

`launch --json` 成功时输出：

```json
{"ok":true,"taskId":"...","folder":"...","name":"...","version":"...","pid":1234}
```

未拿到 PID 时 `pid` 为 `null`。失败信息写入 stderr；JSON 模式下不会混入 stdout。

## 3. 控制台与日志

Release 构建使用 Windows GUI 子系统。终端 CLI 会附加到父控制台并接管 stdout/stderr；
从资源管理器直接双击启动时没有父控制台，行为与普通 GUI 启动一致。

`launch`、`import` 和 `list` 与 GUI 一样写入 `latest.log` 与归档日志；`info` 在日志初始化前
处理，不写日志。静默启动、`list` 和 `info` 不向控制台输出 tracing 日志；`--json` 输出不会
被日志污染。

## 4. 平台说明

Linux 上 `list` 和 `info` 允许以 root 运行；`launch` 和 `import` 会拒绝 root 身份。

## 5. 与启动参数的区别

| 场景 | 接口 |
| --- | --- |
| 终端手动执行 | 终端 CLI：`BMCBL launch`、`BMCBL list` 等 |
| 桌面快捷方式 | 启动参数：`BMCBL.exe --launch "<版本目录>"` |
| 文件关联 | 启动参数：`BMCBL.exe --import "%1"` |
| 更新器子进程 | 启动参数：`BMCBL.exe --updater <源> <目标> <超时>` |

启动参数默认遵循实例的“快捷方式静默启动”设置，并且不提供 JSON 输出；终端 CLI 默认静默。
详见 [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.md)。

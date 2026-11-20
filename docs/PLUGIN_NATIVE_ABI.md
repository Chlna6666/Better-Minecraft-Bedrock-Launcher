# 插件 Native Bridge ABI

`.bmcblx` 仍安装 Wasm 组件。插件使用 SDK 的 `native` 模块提交宿主任务，结果通过现有的 `JobReady` 事件返回。BMCBL 只管理动态库、资源、任务和消息字节，业务命令和事件格式由插件自己定义。

## 动态库入口

动态库导出 `bmcbl_native_entry`，返回静态 `NativeModuleApi` 表的 `AbiHeader` 指针。类型定义在 `bmcbl_plugin_api::native::abi`。当前 ABI 是 1.0。

| 字段 | 语义 |
| --- | --- |
| `abi_major`、`abi_minor` | 主版本必须匹配，宿主接受满足最小次版本且保留已知函数表前缀的库 |
| `struct_size` | 以字节计，验证后才读取已知前缀 |
| `create_instance` | 创建原生实例，返回库私有 `u64`，并保存事件唤醒回调 |
| `submit_command` | 复制输入，向有界队列提交，并立即返回提交状态 |
| `read_events` | 把一批事件写入宿主缓冲区，容量不足时返回需要的长度并保留该批事件 |
| `cancel_command` | 请求取消一个原生命令 |
| `destroy_instance` | 停止实例并等待工作线程及回调结束后返回 |

缓冲区由调用方持有，借用仅在当次 C ABI 调用内有效。函数使用固定宽度的状态码、长度与 ID。回调仅唤醒宿主事件读取，不能重入 Wasm。原生实现负责在 FFI 边界内处理 Rust panic。

宿主分配模块和实例句柄，运行时按插件代次验证所有权。请求 ID 用于异步关联。每个实例只允许一个批量事件读取。命令和事件批次分别限制为 64 KiB 和 256 KiB。销毁失败时宿主保留动态库和回调存储，直到进程退出。数据传输和协议状态保存在动态库内部。

## 安装和授权

SDK 可查询宿主 OS、架构和工具链环境。插件使用通用下载服务获取匹配的单文件模块，再用自己的下载任务 ID 和预期 SHA-256 调用 `native::stage_download`。宿主校验并原子写入按摘要区分的目录。

需要交互安装时，插件先用 `download_resource` 创建下载任务，再把任务 ID 和预期 SHA-256 传给 `native::download(task_id, sha256)`。BMCBL 等待下载完成，校验并暂存文件，然后在窗口中请求用户授权。

多个线路可使用 `app::download_verified`。插件声明 `download.start` 和来源 URL 允许列表，按优先顺序传入 HTTPS 来源、文件名、字节数和 SHA-256。任务依次尝试来源，GitHub URL 继续使用启动器的 GitHub 加速设置。领取任务 ID 后可交给 `native::download` 请求授权。

`app::read_download_text` 可读取本代次提交的小型 UTF-8 下载。`app::fetch_download_text` 从允许的 HTTPS 地址直接获取文本，并校验每次重定向，适用于二进制完整性校验所用的发布清单。

用户在插件设置的「权限」页选择并批准可信动态库。授权按插件 ID 和文件摘要存储。`native::load` 只接受摘要，并在动态加载前重新校验本机已授权文件。保留旧版本目录可让插件继续选择旧摘要。卸载插件时，宿主在所有动态库释放后清理本地副本。

对已暂存的组件，可在文件选择器中打开 BMCBL 状态目录下的 `plugin-native/<插件 ID>/<SHA-256>`，并选择其中的平台动态库。

动态库与 BMCBL 处于同一进程，拥有宿主的进程权限。Wasm 执行隔离继续适用于 `.bmcblx` 组件。发布者应分发每个受支持的平台产物、摘要及对应的许可证和源码说明。

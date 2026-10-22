# 工作区 crate 目录

本文列出 BMCBL 工作区中每个 crate 的职责、状态与文档入口。新增/删除 crate 时必须同步更新
本表与根 `Cargo.toml` 的 `[workspace].members`。

## 1. 应用与框架

| crate | 路径 | 职责 | 状态 | 文档 |
| --- | --- | --- | --- | --- |
| `bmcbl` | `/` | 启动器二进制与库：UI、核心业务、下载、插件宿主 | 主应用 | [README](../README.md)（根） |
| `egpui` | `crates/egpui` | 桌面应用 host、运行时、任务 scope、UI bridge、i18n、持久任务 | 活跃 | [README](../crates/egpui/README.md) |
| `egpui-build` | `crates/egpui-build` | `App.toml`/资源索引/平台打包计划 | 活跃 | [README](../crates/egpui-build/README.md) |
| `egpui-manifest` | `crates/egpui-manifest` | 应用 manifest schema 与校验 | 活跃 | [README](../crates/egpui-manifest/README.md) |

## 2. UI 核心

| crate | 路径 | 职责 | 状态 | 文档 |
| --- | --- | --- | --- | --- |
| `gpui` | `crates/gpui` | 窗口、实体、元素、布局、输入、文本、场景、呈现 | 活跃，pre-1.0 | [README](../crates/gpui/README.md) |
| `gpui-3d` | `crates/gpui-3d` | 3D 场景/相机/材质/动画/查询、glTF、离屏渲染、scene view | 活跃，pre-1.0 | [README](../crates/gpui-3d/README.md) |
| `gpui_tokio` | `crates/gpui_tokio` | GPUI/Tokio 任务集成 | 活跃 | [README](../crates/gpui_tokio/README.md) |
| `gpui-hooks` | `crates/gpui-hooks` | GPUI hooks 与状态复用 | 活跃 | [README](../crates/gpui-hooks/README.md) |
| `gpui-hooks-macros` | `crates/gpui-hooks-macros` | hooks 宏实现 | 内部 | [README](../crates/gpui-hooks-macros/README.md) |
| `gpui-macros` | `crates/gpui-macros` | GPUI 派生宏镜像 | 内部 | [README](../crates/gpui-macros/README.md) |
| `lucide-gpui` | `crates/lucide-gpui` | Lucide 图标资产适配 | 活跃 | [README](../crates/lucide-gpui/README.md) |

## 3. 图形（nova-gfx）

| crate | 路径 | 职责 | 状态 | 文档 |
| --- | --- | --- | --- | --- |
| `nova-gfx` | `crates/nova-gfx` | 图形库聚合入口与文档 | 活跃 | [README](../crates/nova-gfx/README.md) |
| `gfx-core` | `crates/nova-gfx/gfx-core` | 后端无关描述符、句柄、能力 trait | 活跃，API 破坏性演进 | [README](../crates/nova-gfx/gfx-core/README.md) |
| `gfx-memory` | `crates/nova-gfx/gfx-memory` | GPU 分配封装、上传环、延迟释放、统计 | 活跃 | [README](../crates/nova-gfx/gfx-memory/README.md) |
| `gfx-shader` | `crates/nova-gfx/gfx-shader` | WGSL 校验与 SPIR-V/HLSL/MSL/GLSL 生成 | 活跃 | [README](../crates/nova-gfx/gfx-shader/README.md) |
| `gfx-shader-build` | `crates/nova-gfx/gfx-shader-build` | 构建期着色器编译与打包 | 活跃 | [README](../crates/nova-gfx/gfx-shader-build/README.md) |
| `gfx-dx11` | `crates/nova-gfx/gfx-dx11` | Windows D3D11 后端 | 活跃 | [README](../crates/nova-gfx/gfx-dx11/README.md) |
| `gfx-dx12` | `crates/nova-gfx/gfx-dx12` | Windows D3D12 后端 | 活跃 | [README](../crates/nova-gfx/gfx-dx12/README.md) |
| `gfx-vulkan` | `crates/nova-gfx/gfx-vulkan` | Vulkan 后端 | 活跃 | [README](../crates/nova-gfx/gfx-vulkan/README.md) |
| `gfx-opengl` | `crates/nova-gfx/gfx-opengl` | OpenGL 4.5 后端（WGL/EGL） | 活跃 | [README](../crates/nova-gfx/gfx-opengl/README.md) |
| `gfx-metal` | `crates/nova-gfx/gfx-metal` | Apple Metal 后端 | 部分实现（离屏未完成） | [README](../crates/nova-gfx/gfx-metal/README.md) |
| `gfx-webgl` | `crates/nova-gfx/gfx-webgl` | WebGL 后端 | stub | [README](../crates/nova-gfx/gfx-webgl/README.md) |

`examples/triangle*`、`examples/atlas-smoke*`、`examples/public-api-smoke` 是构建期 smoke
示例，不是库 API。

## 4. Bedrock 领域与存储

| crate | 路径 | 职责 | 状态 | 文档 |
| --- | --- | --- | --- | --- |
| `bedrock-world` | `crates/bedrock-world` | 世界/level.dat/NBT/玩家/区块读写与版本语义 | 活跃 | [README](../crates/bedrock-world/README.md) |
| `bedrock-leveldb` | `crates/bedrock-leveldb` | Mojang 修改版 LevelDB 存储引擎 | 活跃 | [README](../crates/bedrock-leveldb/README.md) |
| `bedrock-render` | `crates/bedrock-render` | 世界瓦片 CPU 渲染 | 活跃 | [README](../crates/bedrock-render/README.md) |
| `bedrock-block-model` | `crates/bedrock-block-model` | 资源包方块模型解析 | 活跃 | [README](../crates/bedrock-block-model/README.md) |
| `bedrock-brarchive` | `crates/bedrock-brarchive` | BRArchive v1 只读访问 | 活跃 | [README](../crates/bedrock-brarchive/README.md) |
| `bedrock-voxel` | `crates/bedrock-voxel` | 图片/OBJ → 方块放置计划 | 活跃 | [README](../crates/bedrock-voxel/README.md) |

## 5. 插件与网络

| crate | 路径 | 职责 | 状态 | 文档 |
| --- | --- | --- | --- | --- |
| `bmcbl_plugin_api` | `crates/bmcbl-plugin-api` | 插件 ABI、宏、`.bmcblx` 打包 | 活跃（API `0.6`） | [README](../crates/bmcbl-plugin-api/README.md) |
| `bmcbl_plugin_macros` | `crates/bmcbl-plugin-macros` | 插件属性宏 | 内部 | [README](../crates/bmcbl-plugin-macros/README.md) |
| `nethernet` | `crates/nethernet` | Bedrock NetherNet（WebRTC）传输 | 活跃 | [README](../crates/nethernet/README.md) |
| `easytier-bmcbl` | `crates/easytier-bmcbl` | EasyTier 的 BMCBL 封装（线程/实例管理） | 活跃 | [README](../crates/easytier-bmcbl/README.md) |

## 6. 外部/ vendored 代码

以下目录**不在**工作区 members 中，按上游方式维护，不适用本仓库的文档规范：

| 路径 | 说明 |
| --- | --- |
| `crates/easytier` | EasyTier 上游源码（excluded） |
| `crates/raknet` | RakNet 上游/移植代码（excluded） |
| `vendor/*` | `dtls`、`webrtc`、`webrtc-ice`、`gpu-allocator`、`gpui-router` 等补丁依赖 |

修改 vendored 代码时，应在 PR 中说明上游版本、补丁原因与升级策略。

## 7. 边界速查

- `nova-gfx`/`gfx-*` 不依赖 `gpui`/`egpui`/应用代码；`gfx-core` 不依赖窗口句柄。
- `gpui` 不依赖应用层；`gpui-3d` 依赖 `gpui` + `gfx-core`，不拥有窗口或后端。
- `egpui` 不拥有下载/地图/路由等产品语义。
- `bedrock-*` 不依赖 UI/GPU 呈现；`bedrock-voxel` 只产出计划，不直接写世界。
- 插件 ABI 变更必须同时更新 API 版本、manifest schema 与示例插件。

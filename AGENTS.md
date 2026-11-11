# AGENTS.md

本文件只保留适用于整个仓库的稳定约束和任务路由。按需打开对应文档，不要求每次修改都通读项目文档。

## 1. 项目概况

BMCBL 是基于 Rust + GPUI 的 Minecraft Bedrock 原生桌面启动器。Windows 为主平台，Linux 处于测试阶段，macOS 不受应用支持。

## 2. 按任务读取

- crate 职责、跨层依赖或 GPU/渲染改动：阅读 [系统架构](docs/ARCHITECTURE.md)；需要 crate 清单时再看 [CRATES](docs/CRATES.md)。
- Rust API 或标识符改动：阅读 [命名规范](docs/NAMING.md)。
- 构建、feature、后端、测试或排障：阅读 [构建与测试](docs/BUILD_AND_TEST.md)。
- 非局部变更、提交或文档流程：按需阅读 [贡献指南](docs/CONTRIBUTING.md)。
- 文档修改：查阅 [文档地图与规范](docs/README.md) 及对应主题文档。

小型局部修改只读理解该改动所需的代码和文档。

## 3. 不可违反的项目边界

- nova-gfx/gfx-* 只提供通用图形 API，不依赖 GPUI、应用代码或产品/场景策略。gpui-3d
  拥有游戏级 3D 场景、GPU Resident 数据、剔除、构网、RenderGraph、DrawIndirect、原生
  离屏 Pass 调度和资源生命周期。
- 纯 3D 引擎不得依赖 GPUI 或 BMCBL。GPUI 管理窗口、Swapchain、输入/布局、GPU Owner
  协调、Sidecar 消费、Present 与 Fence 跟踪；只依赖 gfx-core 能力 trait，不按 backend
  调用固有方法，也不得重编码、排序或合并 3D 绘制命令。
  BMCBL 使用 gpui-3d 的 Sidecar 时必须显式启用 gpui-sidecar feature。
- GPU 在途资源只能在相应提交/Fence 确实完成后释放；提交已接受不代表 GPU 已完成。
  不要在热路径加入 CPU Fence 等待，CPU 耗时与原生 GPU 时间戳分开判断。默认队列和
  资源生命周期细则见系统架构文档。
- egpui 不拥有 BMCBL 产品语义；bedrock-* 不依赖 UI/GPU 呈现，bedrock-voxel 只生成计划、
  不写世界。
- 除非任务明确要求并说明上游来源与补丁原因，不修改 crates/easytier、crates/raknet
  或 vendor/。

## 4. 修改与验证

- 先检查相关实现、调用方和当前工作区；保护已有用户修改，只改完成当前请求所需的文件。
- 小型局部修复不要求形式化计划。涉及跨 crate、公共 API、持久化、并发/生命周期或渲染
  链路时，先界定目标、影响范围、平台/feature 和验收方式。
- 根据改动风险运行最小且相关的检查；不要求每次运行完整工作区测试或所有后端矩阵。
  行为改动应有针对性验证；硬件相关结论需要相应设备/运行时证据。纯文档改动不运行
  Rust 编译。
- 命令以 [构建与测试](docs/BUILD_AND_TEST.md) 为准。只报告实际执行的命令和结果；
  区分静态检查、编译/测试与运行时验证。
- 任务做到用户要求的结果可检查、可交付后再结束。只有用户明确要求时才提交、推送、
  合并或发布。

## 5. 工作区与文档

- 不覆盖无关未提交修改；不得擅自使用 reset --hard、clean -fd 或强制切换分支。
- 改动带英文镜像的稳定文档时同步更新；文档中的命令、feature、版本和路径应与代码一致。
- 不把测试输出、benchmark 原始数据、截图、日志、改名历史或带日期计划写入 docs/。
- 最终说明改了什么、实际做了哪些验证，以及尚未验证的行为或风险。

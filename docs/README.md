# BMCBL 文档

本目录是 BMCBL 及其工作区 crate 的规范化文档入口。文档以**中文为第一语言**，
面向开发者的核心文档提供英文镜像；crate 的 `README.md` 使用英文，`README.zh-CN.md`
提供中文版本。

## 文档地图

| 文档 | 内容 |
| --- | --- |
| [ARCHITECTURE](ARCHITECTURE.md) | 系统分层、crate 边界、渲染栈、运行时与插件架构 |
| [BUILD_AND_TEST](BUILD_AND_TEST.md) | 工具链、构建命令、feature/后端矩阵、测试分级与排障 |
| [CONTRIBUTING](CONTRIBUTING.md) | 变更流程、边界规则、命名、测试、文档与提交规范 |
| [CRATES](CRATES.md) | 工作区 crate 目录：职责、状态、边界与维护方式 |
| [CLI](CLI.md) | 终端命令行子命令、输出格式与退出码 |
| [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.md) | 快捷方式/文件关联/更新器使用的启动参数与兼容参数 |
| [NAMING](NAMING.md) | 命名规范（wgpu 对齐、动词族、公共/内部接口） |
| [ADR 0001](adr/0001-nova-gfx-gpui-boundary.md) | gpui 与 nova-gfx 的边界决策 |

英文镜像：
[README.en.md](README.en.md) · [ARCHITECTURE.en.md](ARCHITECTURE.en.md) ·
[BUILD_AND_TEST.en.md](BUILD_AND_TEST.en.md) · [CONTRIBUTING.en.md](CONTRIBUTING.en.md) ·
[CRATES.en.md](CRATES.en.md) · [CLI.en.md](CLI.en.md) ·
[STARTUP_ARGUMENTS.en.md](STARTUP_ARGUMENTS.en.md)

## 推荐阅读路径

- **第一次接触项目**：根目录 [README](../README.md) → 本文档 → [ARCHITECTURE](ARCHITECTURE.md)。
- **修改 GPU/渲染代码**：先读 [ADR 0001](adr/0001-nova-gfx-gpui-boundary.md) 与
  [NAMING](NAMING.md)，再读 [BUILD_AND_TEST](BUILD_AND_TEST.md) 的后端矩阵。
- **修改应用功能**：根 README 的功能图 → `src/` 对应模块 → [CRATES](CRATES.md) 中相邻 crate。
- **AI/自动化代理**：先读根目录 `AGENTS.md`，再按任务类型选择上面的文档。

## 文档规范

1. **代码是事实来源**。文档中的命令、feature、版本号、路径必须能在当前代码/清单中
   对应；无法验证的指标不要写入文档。
2. **不收录临时产物**。测试输出、benchmark 原始数据、截图、日志、一次性验证结果不放
   在 `docs/`；确需保留的证据应进入测试或 CI 产物。
3. **不写改名历史**。迁移映射、旧名清单、分阶段命名审计不属于长期文档；需要时在提交
   信息或 PR 描述中说明。
4. **不写带日期的计划/草稿**。路线图放在 issue/PR 或项目看板；`docs/` 只保留稳定知识。
5. **边界优先**。跨 crate 的职责变化必须先更新 [ARCHITECTURE](ARCHITECTURE.md) 或新增
   ADR，再改代码。
6. **中英同步**。修改 `README.md`/`README.zh-CN.md` 或带 `.en.md` 镜像的文档时，必须
   同步另一语言，避免长期漂移。

## 文档检查

```powershell
pwsh -File scripts/check-docs.ps1
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
```

`check-docs.ps1` 会验证 crate README 双语、相对链接、以及禁止的文档类型（测试数据、
改名历史、带日期计划）。

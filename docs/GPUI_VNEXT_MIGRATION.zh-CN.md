# BMCBL 中的 GPUI vNext 迁移

本文记录 BMCBL workspace 如何接入 vendored GPUI vNext 迁移。

## 范围边界

GPUI 迁移只覆盖框架和 UI 适配工作，不涉及：

- 下载引擎行为；
- Minecraft 业务逻辑；
- 插件运行时逻辑；
- 网络工作流逻辑。

## 当前状态

当前仓库已经完成 phase 1 的框架结构拆分：

- GPUI 的 `element`、`render_pipeline`、`style`、`layout` 以及 `Div` 内部已经完成结构分离；
- 现有 BMCBL 代码仍然基于当前公开 API 编译通过；
- 由于后续 breaking API 阶段还没启用，因此 BMCBL 大范围 UI 迁移尚未正式开始。

当前 `style/layout` 命名边界为：

- `style/` 负责样式语义，包括面向布局的样式枚举；
- `layout/` 负责布局引擎、转换、缓存、fingerprint 与 metrics。

## 验证

当前仓库状态已经通过以下检查：

- `rtk cargo check -p gpui`
- `rtk cargo check -p gpui --examples`
- `rtk cargo check`

这些检查说明目前的结构拆分仍然能和整个 workspace 正常集成。

## 计划中的 BMCBL 迁移顺序

当 GPUI vNext API 变更真正启用后，BMCBL 计划按以下顺序迁移：

1. `src/ui/components/**`
2. `src/ui/main_window/**`
3. `src/ui/views/home/**`
4. `src/ui/views/download/**`
5. `src/ui/window/map_viewer/**`
6. 其余 settings/tools/tasks 页面

## 后续文档工作

后续版本应继续补充：

- 具体 API 映射表；
- 兼容层说明；
- 已删除 API 列表；
- BMCBL 关键页面和窗口的 smoke-test 检查点。

# `src/ui` 结构与放置规则

[English](README.md)

本文描述当前 UI 层的职责边界，是 GPUI 视图、UI 状态、组件、覆盖层与工具窗口放置位置的
本地事实来源。完整英文规则见 [README.md](README.md)。

## 职责

`src/ui` 只负责 GPUI 呈现与 UI 协调：

- 路由页面与页面组合；
- 主窗口 chrome、背景、覆盖层与页面注册表；
- 独立工具窗口；
- 可复用 UI 组件；
- 仅 UI 使用的全局状态；
- 主题与动画辅助；
- 渲染期格式化与小交互胶水。

`src/ui` 不拥有持久业务工作流。网络 IO、持久缓存、归档解压、下载引擎、解码管线、
Minecraft 领域逻辑与插件运行时内部实现属于非 UI 模块。音乐播放属于独立打包的音乐插件/
native sidecar。

## 后台状态桥

领域模块暴露带明确 lag/闭包/恢复行为的 owned 事件或快照流。UI Entity 通过
`Context::spawn_stream` 绑定；应用生命周期级全局桥在应用 setup 时用 `App::spawn_stream`
启动一次。

页面代码不得手写 `recv().await -> Entity::update -> cx.notify()` 循环；页面拥有的流要保存
返回的 `Task<()>`，render 只读本地稳定快照。render 方法不得获取 task-manager 或 service
的 `Mutex`/`RwLock`。若事件只是失效通知，应走统一的失效路径，而不是复制状态。

## 相关文档

- [工作区架构](../../docs/ARCHITECTURE.md)
- [贡献指南](../../docs/CONTRIBUTING.md)
- [crate 目录](../../docs/CRATES.md)

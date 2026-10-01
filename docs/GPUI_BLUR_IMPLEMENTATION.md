# GPUI/Nova 模糊实施记录

本轮范围：原生窗口材质、元素 `background_blur` / `filter_blur` API、retained 效果图、
Linux 独立 presentation owner。保留现有采样质量与应用视觉默认值。
本记录区分源码实现和实际验收；未完成项不能作为性能或跨平台支持承诺。

## 验证基线

修改前保存了完整 dirty diff、status 和 GPUI source 快照，位于本机
`%TEMP%/gpui-blur-20260930/`。这些文件用于区分本轮改动与已有修改。

定向命令：

```powershell
rtk cargo test --manifest-path crates/gpui/Cargo.toml --lib --no-default-features blur
```

最初结果为 45 通过、5 失败。四项生命周期失败的原因是测试宏 teardown 调用
`App::quit()`，但 `TestPlatform::quit()` 是空操作。现在直接调用
`TestAppContext::quit()`，执行测试上下文清理队列与 `App::shutdown()`；
没有清除测试中的实体，也没有跳过泄漏检测。修复后四项生命周期测试通过。
第五项失败还涉及初始化时在 map 前采样可见性，以及测试未确认启动帧呈现。
可见性改为从 map 后的 native state 初始化；测试通过真实 presentation 请求确认启动帧，
然后验证动画 damage，原有 `Partial` 断言保留。定向 suite 现为 **51 通过、0 失败**，
包含新增的窗口请求/透明降级查询用例。

## 源码实现状态

- [x] 元素 builder 与 `Style` 字段迁移至 `background_blur` / `filter_blur`。
- [x] 半径动画入口迁移至 `AnimationProperty::filter_blur` / `TransitionProperty::FilterBlur`。
- [x] 更新仓库内调用方、示例和相关文档；不保留旧 builder/动画别名。
- [x] 窗口增加 Mica/MicaAlt、能力查询、请求与生效模式查询。
- [x] Windows 使用 DWM system backdrop，透明模式清除材质，失败降级透明。
- [x] Wayland ext capability 协商、KDE fallback、global/capability 撤销处理。
- [x] X11 检查 compositor owner 与 KDE root 公告，监听公告和 owner 变化。
- [x] macOS Mica 系列映射已有原生 blur；测试平台保留请求模式。
- [x] 定向 blur suite 全部通过。
- [ ] 紧凑 ROI、稳定 visual identity、source/filter/composite 效果图。
- [ ] 64 MiB 持久缓存、LRU、fence 安全回收及临时资源峰值统计。
- [ ] 按源/区域/draw order 的动画损伤与独立 source/filter 失效。
- [ ] Linux 独立 native/presentation owner。

## 验收状态与环境

Windows 基础与 DX12/Vulkan feature 检查、模式降级及 DWM 映射定向测试已运行通过。
`cargo test --all-features --no-run` 与 `cargo bench --all-features --no-run` 已通过；
前者同时检查 examples。profiler feature 下窗口可见性统计的导出歧义已通过明确
性能统计入口修复。构建仍有既有 dead-code 与重复 glob re-export 警告，未扩展清理范围。

Windows 实测硬件为 AMD Radeon 780M、96 DPI。`animation_perf_lab --all-features`
以 `--copies=1 --seconds=4` 运行：DX12 持续帧检查为 PASS，动画呈现间隔 p95 10.138 ms。
该示例成功完成测量后继续保留交互窗口，测量后已清理本轮启动的 DX12 进程。
Vulkan 两次运行都记录为 FAIL，窗口 `active=false`，动画间隔 p95 约 78 ms；
200 ms UI 阻塞期间仅有约 3 个变化帧。此结果既不是有效前台性能比较，也不能作为
完整独立呈现验收通过的证据。失败记录保留，未放宽连续帧 gate 或修改后台 pacing。
这些数值是呈现间隔，不是 GPU blur shader 时间。

本机无 Linux runtime、WSL 发行版或已配置 Linux SSH host。Linux Rust target 下载
十分钟没有新增字节，已中止该安装；没有改依赖、系统镜像或项目 feature matrix。
Linux 源码目前只有协议/绑定静态核验，尚无 Linux 编译与运行结论。

尚缺实际图像对比、材质生命周期、200 ms UI 阻塞持续呈现、1080p/4K 与 1/16/64
效果节点性能矩阵。现有 perf lab 的 GPU submission wait 是 CPU 等待时间，不能当作
GPU shader 执行时间。100 帧预热、1,000 帧 GPU p50/p95 及资源峰值测量尚未建立；
不能据此声称达到 20% 优化目标或“完整模糊支持”完成。

原生材质契约见 [模糊文档](../crates/gpui/docs/backdrop_blur.zh-CN.md)。

## 交互模糊示例

新增 [`blur`](../crates/gpui/examples/blur.rs) example，展示 0/2/6/12 逻辑像素的
背景与元素过滤、嵌套玻璃、圆角裁剪、中英文/emoji、阴影，以及五种窗口材质的
切换和请求/生效/能力查询。已通过 Windows DX12/Vulkan feature 的 example check
与 all-features build；DX12、Vulkan 实际窗口均启动并响应。视觉对比与性能验收仍待完成。

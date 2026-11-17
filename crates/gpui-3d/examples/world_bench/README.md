# world_bench — 大世界渲染基准（GPUI 侧载 vs 直接渲染）

一个 gpui-3d 示例，构建同一个复杂大世界（光照、天空、实例化城市 + 可选下载的真实 glTF 大场景），然后通过两条可切换的渲染路径呈现，用于比较**性能**与 **API 调用流程**：

- `--mode=direct`：无窗口的 `OffscreenRenderer`，直接走 nova-gfx 后端，输出 render/capture 帧时间分位数与资源计数器。
- `--mode=sidecar`：GPUI 原生窗口 + `SceneView` 侧载元素，带可自由飞行的摄像机，采样 GPUI Present/FPS/GPU 等待指标。

两条路径共用 `scene.rs` 的场景与 `camera.rs` 的相机，因此几何、材质、灯光和取景一致，对比才有意义。

## 场景内容

1. **真实模型（主）**：默认尝试加载 `assets/Sponza.glb`（运行 `fetch_model.ps1` 下载，非 Draco 的 glTF-Binary 版本）。没有时回退到仓库自带的 `DamagedHelmet.glb`（>10k 三角、4 张 PBR 贴图），再回退到纯程序化世界。`--source=studio` 可强制跳过 glTF。
2. **程序化大世界**：`--instances` 控制的一片实例化建筑网格（默认 20000，全部共享一份 mesh/material，折叠为单次 instanced draw）、程序化 albedo/normal/AO 贴图、地面与发光地标。
3. **光影**：环境光 + 方向光（太阳，`--sun=方位,仰角`）+ 点光 + 聚光，四条光源齐全。
4. **天空**：
   - direct 路径用 `Scene::set_environment(EnvironmentMap::procedural_studio())`，即真正的天空三角 + IBL + 独立 shadow pass。
   - sidecar 路径没有 environment 概念。默认用平背景（没有任何覆盖）；加 `--screen-effects` 可启用 `SceneView::with_screen_effects(ScreenEffects::default())` 的解析式天空/天体 + 材质感知阴影 + 体积雾/云。
   
   `--screen-effects` 是**可选项**：它要求设备支持有序 compute + indirect draw，且只支持带完整 tangent 数据的几何。当前测试环境（DX12）下该 renderer 无法完成准备，GPUI 会静默丢弃帧（表现为 0 present），因此默认关闭以保持侧载路径可用。不支持时示例会诚实报告 0 present，不会伪造数据。
   
   **两条路径的天空/阴影实现不同**，比较数值时要分别标注，不能直接把两者当成同一实现。另外，程序化世界的实例化立方体使用 `Mesh::cube().generate_tangents()` —— 没有 tangent 的 normal-map 材质在 offscreen 下能渲染，但会让 sidecar 的 scene-view 准备静默丢帧。
5. **可移动摄像机**：sidecar 窗口里 `W/A/S/D` 平移、`Q/E` 升降、`Shift` 加速、鼠标左键拖拽转向、滚轮前后推进。`SceneView::with_camera_sampler` 让渲染线程直接从共享相机状态取样，输入不触发 UI 树重建。direct 路径用脚本化的每帧 orbit（`--orbit-degrees`，默认 3°）推进相机，避免静态帧复用造成的假低耗时。

## 下载主模型

```powershell
pwsh -NoProfile -File crates/gpui-3d/examples/world_bench/fetch_model.ps1
# 强制重新下载：加 -Force
```

下载失败不会阻断示例：会自动使用 `DamagedHelmet.glb` 或程序化世界，并在输出里打印实际来源。

## 运行

先确保工作区根目录，所有命令都带 `--features native`（侧载窗口需要 nova 后端）：

```bash
# 直接渲染（无窗口，推荐先跑这个验证场景与后端）
cargo run -p gpui-3d --example world_bench --features native -- \
  --mode=direct --backend=dx12 --frames=120 --instances=20000 \
  --source=model --json=direct.json --trace

# GPUI 侧载（真实窗口 + 飞行摄像机，auto-exit 便于自动化）
cargo run -p gpui-3d --example world_bench --features native -- \
  --mode=sidecar --backend=nova-dx12 --auto-exit-ms=8000 --trace

# GPUI 侧载 + 解析天空/材质阴影（需设备支持；不支持时会报告 0 present）
cargo run -p gpui-3d --example world_bench --features native -- \
  --mode=sidecar --backend=nova-dx12 --auto-exit-ms=8000 --screen-effects --trace

# 纯程序化、不依赖任何模型
cargo run -p gpui-3d --example world_bench --features native -- \
  --mode=direct --backend=vulkan --source=studio --frames=120
```

`--backend` 取值：sidecar 用 `nova-dx12|nova-vulkan|nova-dx11|nova-opengl`（与 `scene_view` 示例一致）；direct 用 `dx12|dx11|vulkan|opengl`（与 `scene_lab` 一致）。direct 未指定时默认 `dx12`，sidecar 默认 `nova-dx12`。

完整参数见 `--help`。

## 指标对照

| 指标 | direct（`--json`） | sidecar（`--json`） |
|---|---|---|
| 路径/后端 | `path=direct` | `path=sidecar` |
| 帧时间 | `render_p50/p95/p99`、`capture_p50/p95/p99`（ns，CPU 侧 `Instant`） | `present_fps_avg`（GPUI Present 驱动，非逐帧 CPU 计时） |
| 呈现计数 | `frames`（渲染帧数） | `presents`（native present 数） |
| GPU 等待 | `steady_submit_nanos`（提交耗时） | `gpu_wait_avg_us`（GPU 提交阻塞等待） |
| 绘制量 | `steady_draw_calls`/`steady_triangles`/`steady_instances` | 由 GPUI 内部诊断提供，示例不伪造 |
| 天空/阴影 | `sky=environment_map` + shadow pass | `sky=screen_effects`（加 `--screen-effects`）或 `sky=flat_background`（默认） |
| 场景来源 | `source` | `source` |

注意：direct 的帧时间是 CPU 侧 `Instant` 包裹的 `render`/`capture` 调用时长；sidecar 的 FPS 是 GPUI 呈现驱动。二者口径不同，应结合调用轨迹与硬件 GPU 时间戳一起判断，不要把单一数字当成等价对比。

## 调用轨迹（"调用问题"诊断）

`--trace` 会在每个关键 API 调用处打印带时间戳和序号的标签，直观展示两条路径的调用顺序：

- direct：`scene.build` → `direct.render` → `direct.render.frame`（每帧 render/capture）→ `direct.renderer.destroy`
- sidecar：`scene.build` → `sidecar.sceneview`（SceneView + element）→ 渲染线程的 camera sampler tick → GPUI Present

要看到库内部的硬件级阶段（DrawIndirect / GeneratedMesh / BufferUpload 的已完成 GPU 时间戳）以及 GPU Owner 的各阶段分解，额外设置环境变量：

```bash
GPUI_3D_TRACE_TIMINGS=1 cargo run ... --trace
```

库会打印 `gpui3d gpu_hardware_phase` / `gpu_hardware_percentiles`（`timing=native_gpu_ticks`）与 GPUI 的 `native_bridge_cpu` 轨迹。这些由 gpui-3d / gpui 自身发出，不是本示例伪造的数值。

## 运行前提与诚实说明

- direct 路径需要对应后端可用（Windows 上 dx12/dx11/vulkan/opengl；Linux 上 vulkan/opengl），与 `scene_lab` 的硬件后端要求相同。
- sidecar 路径需要真实桌面会话与 GPU；无 GUI 环境无法运行，此时应报告"未运行时验证"而不是伪造 FPS。
- 侧载的 CPU 准备 sidecar 在存在非空动画时被禁用（`scene_view/sidecar.rs`）；本示例默认不开动画以保持其生效。可用 `GPUI_3D_DISABLE_SIDECAR=1` 单独关闭 CPU 准备 sidecar 做进一步 A/B。
- 静态相机下 offscreen 会走捕获复用快路径，所以 direct 默认用 `--orbit-degrees=3` 让每帧真正重绘。
- 无头/不可见窗口下 GPUI 的每窗口 `present_fps` / `present_interval` 字段保持为 0（记录在 `record_frame_disposition`，要求 `metrics.visible`），所以示例用全局 `present_fps` 与 GPU Owner 的 `extension_prepare` / `total_p95` 采样判断 120 FPS 预算；不要被每窗口的 0 误导。

## 有 GPUI 侧载 vs 无 GPUI（直接渲染）的差距与 120 FPS 结论

用同一份程序化大世界，在同一台机器（DX12、1280×720、dev 构建）测得的对照（每档 120 帧 / 侧载 6 秒）：

| 实例数 | direct render p50 / p95 | sidecar 全局 FPS（未封顶） | GPU Owner 总 p95 |
|---:|---:|---:|---:|
| 2,000 | 0.52 / 0.91 ms | ~245 | 1.76 ms |
| 5,000 | 0.49 / 1.21 ms | ~245 | — |
| 10,000 | 0.49 / 1.65 ms | ~225-234 | — |
| 20,000 | 0.49 / 2.68 ms | ~116-130 | 9.06 ms (FAIL >8.333) |

**差距确实存在，但不在 GPU 绘制本身，而在 GPUI 侧的每帧 CPU 准备。** 证据：
- direct 的 20k 渲染 p95 只有 2.68 ms，远低于 8.33 ms 预算；sidecar 却掉到 ~120 FPS。
- sidecar 的 GPU Owner 分解显示 20k 时 `extension_prepare_p95 ≈ 8.2 ms`，`total_p95 ≈ 9.06 ms`，即瓶颈是 sidecar 的场景准备/命令编码，不是 GPU 提交（`frame_upload_p95 ≈ 0.29 ms`、`backend_present_p95 ≈ 0.57 ms`）。
- 关掉 CPU 准备 sidecar（`GPUI_3D_DISABLE_SIDECAR=1`）后 20k 从 ~116 回升到 ~129 FPS，说明 CPU 准备是其中一部分（约 10%），但主成本随实例数线性增长，来自 scene-view 的遍历/批处理本身。
- `--fps-cap=120` 在本无头环境下实际只交付约 60 FPS（窗口不可见时平台调度不是 120Hz），这属于测量环境限制，不代表窗口可见时的真实上限；判定应以 GPU Owner 的 `total_p95` 是否 ≤8.333 ms 为准。

**release 构建复测**（`cargo build --release ...`）：20k 未封顶 sidecar ~124 FPS；封顶 120 时 GPU Owner `total_p95=8.764 ms`、`ext_prepare_p95=8.263 ms`，仍 **FAIL >8.333 ms**。release 只把 dev 的 9.06 ms 降到 8.76 ms，**没有改变 20k 达不成 120 FPS 的结论**。

**120 FPS 结论（诚实版）**：
- **约 10k 实例以内：sidecar 稳定超过 120 FPS**（dev 未封顶 225-245 FPS，预算内）。
- **20k 实例：sidecar 不达 120 FPS** —— dev 与 release 的 GPU Owner 总 p95 分别为 9.06 / 8.76 ms，均超过 8.333 ms 预算；未封顶实测 ~116-125 FPS。瓶颈在 sidecar 的 CPU 侧 `ext_prepare`（随实例数增长），不在 GPU 绘制。
- **direct 路径即使 20k 也远低于 8.333 ms 预算**（render p95 2.68 ms），"有 GPUI vs 无 GPUI" 的差距确实存在，是 GPUI 每帧场景准备/批处理的 CPU 成本。
- `--fps-cap=120` 在本无头环境实际只交付约 60 FPS——这是窗口不可见时的平台调度产物，**不能解读为 sidecar 只能跑 60 FPS**；判定用 GPU Owner 的 `total_p95`。

要在真实桌面确认：
1. 在**可见窗口 + 120Hz 显示器**上用 `--fps-cap=120` 复测，确认平台呈现节奏（这是本环境无法覆盖的部分）。
2. 打开 `GPUI_3D_TRACE_TIMINGS=1` 收集 `gpu_hardware_phase` 硬件时间戳，确认 GPU 侧不是隐藏瓶颈。
3. 若要让 20k 达到 120 FPS，方向是降低 `ext_prepare`：减少独立节点数（用更粗粒度的实例化/合批）、或优化 scene-view 准备路径——这是 gpui-3d 源码层的工作，超出本示例范围。

# VerticalEdges 本地原生验证

日期：2026-10-19（Asia/Hong_Kong）。基线：`f797f573`，加本次几何校验与回归修复。

## 验证口径

- 硬件测试保留 `#[ignore]`，避免无 GPU 的默认测试环境误运行；验收必须显式传入
  `--include-ignored`，不能把编译成功或 ignored 数量当成设备验证。
- `*_vertical_edges_pixels` 比较 GPU Indexed 与生产 CPU fallback 生成的静态几何的完整
  BGRA 像素，覆盖 DPI 1/1.25/2、小高度、过冲、反向采样、终态、清除值和滚动位置。
- `*_vertical_edges_first_scroll` 使用真实 GPUI Window：捕获其提交的原始 Packet，
  连续采样动画、原生提交终态、回传完成 ACK，然后注入滚轮输入并交付该输入实际请求的
  platform frame。测试不替换 Window 生成的 Paint，不扩大 dirty region，也不强改其
  Partial 模式。验证实际 retained replay 计数、静态 cached sibling 不 Render、柱体坐标、
  固定祖先 clip，以及原生局部更新结果与独立 renderer 完整绘制的像素一致性。
  滚动的离散 UI commit 正常产生新 Paint publication；其静态 sibling 来自真正 retained
  replay，而不是测试根据滚动坐标手工另建 Painter/Scene。
- 普通 retained replay 允许为当前帧重新绑定 DrawOrder 并复制 primitive；CPU 指针相等
  不是其契约。本测试使用真实 replay 计数和静态 Render 计数证明执行路径。
- Quad 写入观察器仅在测试中编译，记录成功交给 backend buffer batch 的 Quad 写入字节。
  它不把静态字节相等当成无写入证据，也不声称包含驱动内部复制、动画表或 uniform 写入。
- Present damage 观察器记录成功提交实际使用的 native dirty rect；`None` 表示该次没有
  使用 native dirty rect。局部 retained source 更新和完整 swapchain Present 分开报告。

## 复现命令

在仓库根目录使用 PowerShell 7：

```powershell
rtk cargo test -p gpui --lib --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl,build-shader-validation,test-support vertical_edges -- --include-ignored --nocapture --test-threads=1
```

## 已执行结果

Windows / PowerShell 7.6.5 / rustc 1.98.1（48a229cea 2026-09-01）。
Windows 内核版本 10.0.26100；两块 AMD 设备的驱动版本均为 32.0.31007.1017。

本地 `vertical_edges` 测试：**13 passed, 0 failed, 0 ignored**，包含四后端像素和四后端
首次滚动测试。完整 stdout/stderr 保存于 [vertical-edges-native.txt](vertical-edges-native.txt)。
另有 [Packet 协议回归](vertical-edges-packet.txt)（7 passed）和
[四后端 popup 回归](vertical-edges-popup.txt)（4 passed），用于覆盖通用 damage 消费与
测试提交模式的改动。合计 24 项实际执行的定向检查通过。

| 后端 | 像素测试报告适配器 | 连续动画 Quad 写入 / 编码 | 首次滚动 native dirty rect |
| --- | --- | --- | --- |
| DX11 | AMD Radeon 780M | 0 B / 0 primitives | None（完整 Present） |
| DX12 | AMD Radeon 780M | 0 B / 0 primitives | x=40, y=16, w=12, h=40 |
| OpenGL | AMD Radeon RX 7600M XT | 0 B / 0 primitives | None（完整 Present） |
| Vulkan | AMD Radeon 780M | 0 B / 0 primitives | None（完整 Present） |

真实 Window 动画样本的 retained root source 更新为 80、400 像素，小于 4096 像素的完整目标。
这不是“背景完全不绘制”：重绘局部 root source 必须恢复该区域的背景及当前柱体。

首次滚动属于离散 Scene commit：本夹具仍写入 **74,304 B Quad 数据**，root Render 1 次，
静态 cached sibling Render 0 次，retained replay 至少 384 primitives；dirty area 480 像素，
source 更新 704 像素（含 source scissor 边界）。这项上传成本未优化，不能把连续动画的
0 B 写入结论外推到滚动或所有 UI commit。后续优化应沿现有 retained chunk/order proof
链路测量，不能另建 Scene/FrameUpload/cache owner 或放宽绘制顺序证明。

上述三份输出由同一个本地测试二进制生成：

```text
target/debug/deps/gpui-eff819eaf0f40abb.exe
SHA-256: 5A2254154179F03EB516413B51A9F377974584F13282867214197FAA30171347
```

`rtk cargo build -p bmcbl --bin BMCBL`：退出码 0，现有 510 条 warning。

```text
target/debug/BMCBL.exe
SHA-256: 28FA6680E94E0BE25A022D47056C3938711C3C583ACB3EA68DA1B8FB161B9BD0
```

修改范围内的属性/CPU fallback/原生测试文件通过定向 `rustfmt --check`，`git diff --check`
通过。未以全仓格式化、全工作区测试或 CI 替代上述原生执行证据。

公开 API 调用方编译门槛：

- `rtk cargo test -p gpui --all-features --no-run --locked`：退出码 0。
- `rtk cargo bench -p gpui --all-features --no-run --profile dev --locked`：退出码 0；
  frame_upload/images/layouts/memory/paths/renderer benchmark 调用方均编译。
  这里复用 dev 产物验证 API，没有执行 benchmark，也没有声称得到 release 性能数据。
- `rtk cargo check --manifest-path crates/gpui/Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --locked`：退出码 0。

## 边界

测试使用隐藏的原生 HWND、生产 shader/FrameUpload/renderer 和实际 native Present，
再读取生产 draw steps 及 retained source 的完整输出。它不是桌面截图或物理 scanout
时序证明，也不替代真实启动器统计页的人工交互与 CPU/GPU Trace。

本轮不据此宣称 FPS、帧延迟或显存改善；未对 Linux/macOS、透明窗口或不同显卡作外推。
DX11/Vulkan/OpenGL 的安全完整 Present 回退不能被描述成四后端局部 Present。

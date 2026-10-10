# GPUI / Nova 内存与性能验证（2026-10-09）

两帧槽静态版本共享与 Fence 边界见 [P2-A 实现说明](GPUI_STATIC_BUFFER_SHARING.md)。
Retained chunk 分段持有与上传见 [P2-B 实现说明](GPUI_PACKED_CHUNK_UPLOAD.md)。

本批先修改 Nova 资源生命周期，再修改 GPUI 的呈现、静态 PNG 解码和文字缓存压力回收。
保留工作区已有的 compositor 修改；没有升级依赖、改变画质、引入运行时配置开关或新线程池。

## 实现与边界

### P1-B / P1-C：R8 字形与全局图片驻留

单色字形保留单通道 coverage，直接写入 `R8Unorm` Atlas；彩色字形、图片和
Subpixel 掩码继续采用四通道 BGRA。各类型共用 tile 分配、padding、批量上传队列、
GPU owner 和 Fence 退休机制，按类型创建纹理并计算行距与内存指标。
同面积 Mono 页的逻辑纹理字节与上传 payload 为原来的四分之一；原生分配仍可能有
对齐或 allocator 开销，不能据此宣称应用 RSS 或总显存降低 75%。

文字变色和透明度由 sprite/animation 值在 shader 中乘以 R8 coverage 完成，
不会转换字形格式、复制 Atlas 或重新上传字形。初次上传仍需写入可靠的 staging
并填充 tile padding，因此这里的零拷贝指颜色/透明度变化时的纹理复用。
Vulkan 同批次对同一 mip 的重叠区域写入增加 transfer-write barrier，确保后写覆盖
先写；不增加 CPU Fence 等待，互不相交的 tile 不增加该 barrier。

`BoundedImageCache` 改为 App 共享解码的弱查找，普通图片元素也使用同一资源请求。
同一来源的 pending decode 和 ready `RenderImage` 不再按缓存实例重复持有。
`ImagePipelineConfig::idle_image_bytes` 默认 128 MiB，作用于 App 内所有解码缓存的
唯一分配成本；实例 `max_items/max_bytes` 只约束本地查找 working set。
全局 LRU 在加载完成、缓存/元素释放和 trim 时检查；元素释放通知按 App 合并，
等待当前窗口更新完成后再检查。热路径只更新 recency，避免每帧扫描
完整图片集。可见元素、pin、显式预加载租约和外部图片引用可以超过这个软预算。
诊断分别报告预算、唯一缓存成本和超额字节。

全局淘汰将退休通知交给各窗口；当前 Scene 仍引用的 tile 和 GPU 尚在使用的资源
继续由现有 Scene/Fence 管理。GPU Atlas 仍属于各窗口/device，本批不把共享 CPU
预算描述为跨设备物理 VRAM 硬上限，也没有实现活跃 Atlas compact/defrag。
Metal 已写入 R8 格式映射，按要求不编译、不测试；现有 Metal 原生纹理上传尚未
实现，这一限制保留。

Windows 原生像素验证通过 DX11、DX12、Vulkan、OpenGL 四后端：R8 奇数行距、
带 offset 的整图与重叠部分更新、Mono/Subpixel/BGRA/RGBA glyph/image shader、
相同 R8 纹理的红色/半透明绿色/全透明文字。缓存测试覆盖共享解码、过期重载、
全局 LRU、可见元素和显式租约保护。日志位于 `target/p1bc-memory/`；没有量化
BMCBL 连续页面切换的 RSS、显存峰值或帧时间改善。

| 路径 | 本批实现 | 边界 |
| --- | --- | --- |
| DX12 / Vulkan 上传页 | 新提交只退休本次使用的页，旧 busy 页保留自己的 fence；批量分配预检写入结果数组，消除 allocator 深拷贝和 aligned-size 临时数组 | 不复用尚未完成的页；错误输入仍在改变状态前拒绝 |
| Vulkan allocator | moderate/aggressive trim 释放完全空闲的原生内存块，包括最后一个缓存块；P0-B 改为按需小块起步与增长 | 不移动活跃 allocation、不改变 offset；普通帧和 light trim 保留块缓存，moderate 仍保留一个上传页 |
| DX11 / OpenGL 命令 | 成功或失败的提交都归还 CPU 命令数组以便复用 | 消费语义不变；moderate/aggressive trim 收缩空闲命令容量，aggressive 另收缩资源 registry，live ID 保持有效 |
| DX11 查询池 | light trim 保留查询；moderate 保留一个，aggressive 释放缓存 | pending 查询继续等待原生完成 |
| GPUI Windows 呈现 | 编译时跳过 Windows 不会启用的独立 deadline schedule 更新 | DWM / native tick、damage 与提交语义不变 |
| 静态 RGBA PNG | 复用现有 bitmap pool 的 decoder 输出，使用公共 SIMD 路径原地交换 R/B，再交给 RenderImage | alpha、尺寸、完整像素输出不变；不降低分辨率或采样质量；其它色型的 decoder 分配策略保持不变 |
| 文字 raster-bounds 压力回收 | 只暂存访问 epoch，选择淘汰阈值后原地 retain；处理相同 epoch，精确移除所需数量 | 本批不改变日常命中、shaping、glyph rasterization 或日常 aging 策略 |

Vulkan 空闲块回收沿用仓库现有 `vendor/gpu-allocator` patch 边界，复用其 native block
destroy 路径。`active_general_blocks` 随释放更新，保留 block slot 索引；后续 allocation
可以重新建立原生块。GPU backend 先完成 pending work，再做 moderate/aggressive trim。

空闲块回收并不等于搬迁活跃资源或消除所有显存碎片。DX12 实际 backend 仍使用 committed
resource；DX11 / OpenGL 的 native storage 由运行时/驱动管理。本批不创建另一套自定义 GPU
heap，也不对外宣称驱动内部碎片已经解决。相关边界见
[Microsoft 内存管理说明](https://learn.microsoft.com/en-us/windows/win32/direct3d12/memory-management-strategies)
与 [Khronos texture storage 说明](https://wikis.khronos.org/opengl/Texture_Storage)。

## 已测结果

主机：Windows，Ryzen 7 7840H，Rust 1.98.1，Balanced 电源计划。Vulkan headless allocator
基线使用 Radeon 780M；主机另有 RX 7600M XT。下面的 allocator reserved bytes 不是 OS
dedicated VRAM、应用 RSS 或物理显存碎片率。

- 上传分配：64 个既有 busy slot、8 个 256-byte item 的批次，Rust 分配操作从 4 次降到
  2 次，请求字节从 6464 降到 4352。计数不包含原生 driver/C allocation。busy slot
  只是逻辑 allocator 元数据，未为此测试建立对应原生 GPU page。
- 上传 CPU 时间：冻结 release baseline 与修改后程序按 A/B/B/A 运行，101 次/组合。
  busy=64、batch=8 的中位数为 baseline 300 ns，after 100–200 ns。单次计时粒度约
  100 ns，只用于检查这个 bookkeeping 用例，没有换算为应用 FPS。
- Vulkan 回收：活跃 2×2 RGBA texture 在 aggressive trim 后回读像素完全一致。
  销毁资源并 light trim 后，allocated=0、reserved=335544320；aggressive trim 后
  reserved=0。两次创建/上传/回读/释放循环都通过，确认回收后可以重新分配。
  以上为固定 256/64 MiB 块策略的历史结果。P0-B 改为 4 MiB device、1 MiB
  host/readback 起步并按每个 memory type 的压力增长；64 张 256×256 RGBA
  texture 的后续测试 allocated=16 MiB、reserved=28 MiB。Moderate 与 Aggressive
  空块回收均从 5 MiB 到 0，存活纹理在两种 trim 后像素读回一致。
- 静态 PNG：确定性 1920×1080 RGBA fixture，Criterion release、20 samples、1 秒
  warmup、2 秒 measurement。before 45.699–47.473 ms，after 31.406–32.301 ms；
  Criterion 报告耗时变化 -33.231% 至 -30.028%，中心估计 -31.605%。存在 2 个 mild
  outlier。该路径少分配一张 8294400-byte（约 7.91 MiB）的 BGRA 图片；不是应用 RSS
  实测。单测另检查 pointer reuse、透明/半透明像素和错误尺寸。
- PNG decoder 输出接入 bitmap pool 后，同一 Criterion 用例为 29.609–30.375 ms；相对
  原地标量版本，变化区间 -11.863% 至 -3.7322%，中心估计 -7.2058%，p<0.05。
  32 次顺序同尺寸 RGBA 解码的独立检查中，旧数组策略使 pool 保留 8 个数组、66355200
  bytes（约 63.3 MiB）；新策略保留 1 个数组、8388608 bytes（8 MiB）。检查完整像素
  一致、后续数组地址复用。这是 pool idle capacity，不是 OS RSS 或总峰值；采用现有
  size bucket，比单张精确大小增加约 92 KiB，换取后续复用。
- 最终源码重新构建后的完整 PNG 对照为 29.581–30.511 ms（中心 30.047 ms），相对
  最初 45.699–47.473 ms 的 baseline，Criterion 耗时变化 -36.985% 至 -33.828%，
  中心 -35.454%，p<0.05。最终包含输出复用、SIMD 和 decoder 数组池复用。
- 文字 raster-bounds 压力淘汰：31 轮交替 release 对照，4096 项的中位数从 207600 ns
  降到 63100 ns，16384 项从 890500 ns 降到 325300 ns。临时数组从 163840/655360
  bytes 降到 32768/131072 bytes（减少 80%）。准备 cache 在计时外；没有实际 shaping
  或 rasterization，不能把这项结果当成文字绘制帧率。

## 跨平台 SIMD 与图片分段对照

PNG 转换复用 `foundation/color/rgba.rs` 的现有 Fearless SIMD 实现：x86 运行时检测
SSE/AVX2，ARM64 使用 NEON，无法使用 SIMD 时回退标量；短行保持标量，行尾不足一个
SIMD 块的完整像素按标量处理。沿用现有 x86 AVX2 选择，不引入 AVX-512 默认路径。
硬件分派与 OS/GPU renderer backend 无关，没有新增依赖、unsafe 或额外线程池。
临时测试另外强制执行本机支持的 SSE2/AVX2 和检测出的 level，14 组行宽/行数（含
不完整像素尾部和额外 buffer tail）均与标量结果一致；这不等于所有 ISA 实机测试。

提取同一转换内核后，`aarch64-apple-darwin` 的 `cargo check --lib` 通过；这只是内核
编译验证，不是 ARM64 性能测试或整个 GPUI 的 macOS build。新增 PNG 测试覆盖
17×3 的 SIMD 行、标量尾部和任意 alpha，逐字节比较并检查原输出地址保持一致。

同一 1920×1080 fixture 的 SIMD-only 端到端对照为 30.767–31.988 ms；标量原地转换
版本为 31.244–33.918 ms。Criterion 的变化区间为 -7.9392% 至 +0.9919%，p=0.27，
未检测到显著差异，因此不单独宣称 SIMD 让整条解码链提速。

另用临时独立 release（opt-level=3）程序拆分 decode / channel conversion，并比较
Windows WIC 输出。31 轮交替，初始化 COM/factory 在计时外，保留每次 stream / decoder /
converter 创建和像素分配，不进行颜色变换或 premultiplication：

| fixture | Rust RGBA decode 中位数 | WIC BGRA decode 中位数 | scalar R/B swap | SIMD R/B swap |
| --- | --- | --- | --- | --- |
| 17×3、可变 alpha | 2.5 µs | 9.9 µs | 0.1 µs | 0.1 µs |
| 1920×1080、不透明 | 22.0252 ms | 42.2419 ms | 1.0742 ms | 0.3228 ms |
| 1920×1080、可变 alpha | 27.0815 ms | 51.2589 ms | 1.1046 ms | 0.3169 ms |

三组 WIC / Rust 像素逐字节一致，但 WIC 耗时更高，本批不引入 WIC decoder。
独立程序与仓库 release 的 opt-level=s 不同，上表用于同一程序内部比较；不把 22 ms
和 GPUI 的 31 ms 作为代码变更收益。系统 decoder 也不代表 GPU 解码；没有 Qt/Flutter
同 fixture A/B 证据，不能断言其它框架更快。

批量对照使用现有 `background_executor()`，8 次未缓存的同 fixture 直接 render，
10 samples、1 秒 warmup、3 秒 measurement。最终源码的 caller 串行 238.51–247.75 ms，
后台 1/2/4 个任务分别为 237.36–245.39 / 122.65–127.98 / 67.398–70.335 ms。每个任务
串行处理分配给它的图片，立即释放输出；没有创建额外 pool。单张 decode 的耗时没有
因此减少，同时进行的 decoded buffer 会增加。本批增加跨平台可复现 benchmark，
没有把所有图片请求改为无界并行，也没有启用第三方全局 Rayon pool；OS RSS 峰值未测。

## 多格式所有权、解码与池复用

本批继续覆盖通用图片链路，不把 PNG 单格式结果作为整个图片管线完成的证据：

| 路径 | 本批行为 |
| --- | --- |
| 文件、网络、内联压缩图片 | `Vec<u8>` 通过 `Arc<Vec<u8>>` 分享原分配，不再先复制到 `Arc<[u8]>`；静态与已有 shared slice 保持原存储 |
| `RenderImage::from_raw_pixels(Vec)` | 直接移入 `BitmapBytes`，最后一个帧 owner 释放后直接释放；不再整图转存到 Arc slice，也不把外部冷缓冲转成全局池驻留 |
| JPEG | RGB/灰度直接写入池化 BGRA，省去后续整图 R/B 交换；CMYK 原地转换，保留原公式；EXIF 与缩放滤镜不变 |
| BMP | RGBA decode output 原地 SIMD 转换；其它颜色写入池化 BGRA；缩略图只解码一次再按原位置采样 |
| GIF、APNG、动画 WebP | 通用 RGBA frame 转换使用已有跨平台 SIMD，保留帧 Vec、alpha、delay 与 sequence；上游解码器自己的分配仍存在 |
| 静态 WebP | 保持 libwebp 向池化 BGRA 外部缓冲直接写入的现有路径 |
| SVG | 保持 `pixmap.take()` 转移像素所有权；内联路径补上资源路径已有的原地反预乘/RB 交换；压缩源共享同样覆盖 SVG |

BMP 的旧 `read_rect` 路径并不是增量行解码：image 0.25.9 每次为 rect 解码完整图像，
GPUI 又对每个源行调用它，1920×1080 会执行约 1080 次整图解码。新路径调用一次
`read_image`，按相同 `scaled_axis` 从借用源行产生输出；随后保持原中间尺寸和 Lanczos
缩放。旧路径已经有整图临时缓冲，新路径复用这一级缓冲并在采样后归还。诊断路径名称
由 `bmp_rect_sample` 改为 `bmp_decoded_sample`，避免继续把它描述为行解码。

位图池现在从实际请求长度开始查找可用容量，而不是从向上取整的 bucket 开始。
这样上游解码器返回的精确容量 Vec（例如 8,294,400 bytes 的 1080p RGBA）也能由符合
条件的后续请求取用；保持原尺寸类别、最大两倍 bucket 的复用限制、空闲预算与 trim。
这减少漏用和分配 churn，不代表 CPU 堆已被压缩，也不能让没有外部输出 API 的解码器
自动使用调用方缓冲。

零拷贝会保留输入 Vec 的剩余容量。压缩弱缓存、streaming source 成本、decoded frame
缓存成本与框架图片内存快照因此计入 capacity；`resident_byte_len()` 和解码/队列有效
字节指标继续保留 payload 语义。queued/delivered 容量按同一入队、失败回退、过期出队
和最后呈现帧替换路径更新。缓存预算在 load 时刷新，不计外部消费者持有的旧帧、decoder
工作内存及反压时 worker 的 pending frame，仍是缓存成本估算，不能宣称为进程硬内存上限。

`EncodedImageBytes::new` 统一接受 `CompressedImageBytes` 可转换的输入，Vec、Arc slice、
static slice 直接保留存储；非 static 临时切片需显式 `.to_vec()` 或 `Arc::from(...)`。
缓存 Eq/Hash 仍按格式和内容计算，element identity 则按 payload 地址和长度计算。

内联 SVG 的回归测试先在修复前复现：opaque red 实际 `[255,0,0,255]` 被标为 BGRA，
半透明 red 实际 `[128,0,0,128]`；正确的 straight BGRA 分别是 `[0,0,255,255]`
和约 `[0,0,255,128]`。复用 `swap_rgba_pa_to_bgra_buffer` 原地修正，不新增整图像素分配；
既有浮点反预乘截断会有 1 LSB 量化误差，本机半透明红色为 `[0,0,254,128]`，与资源
SVG 共用转换路径。测试逐字节匹配既有转换结果，并拒绝未反预乘的 128。
这一项是颜色/alpha 正确性修复，不能把遗漏必要转换的旧代码当作性能基线。

GPU atlas 仍需要 BGRA 编码/边缘 padding、mapped staging 写入及 GPU-only texture copy。
本批消除的是上述 CPU 所有权边界的冗余副本，不把必要的 GPU 传输称为零拷贝。Atlas
矩形和空纹理页继续按已有机制复用/释放，活动页搬迁压缩没有在本批实现。

最终 raw-pixel 所有权对照在同一个 release executable 内比较：旧路线将 Vec 转为
Arc slice，再调用 shared raw-pixel constructor；新路线保留 Vec，调用 owned constructor。
Vec clone 在 Criterion setup、计时之外，constructor 与结果 Drop 在计时内；20 samples、
1 秒 warmup、2 秒 measurement。外部 raw Vec 保持最后一个 owner 释放后直接释放的
生命周期，避免把原本直接释放的冷数据转为全局 decoder pool 空闲驻留。

| raw pixels | Arc slice copy 中心估计 | owned Vec 中心估计 | 同轮减少 |
| --- | --- | --- | --- |
| 1×1 | 152.52 ns | 111.59 ns | 26.8% |
| 64×64 | 684.35 ns | 164.46 ns | 76.0% |
| 1920×1080 | 1.8137 ms | 237.70 µs | 86.9% |

这是构造/释放边界的结果，不能当作完整 decode 或 GPU 上传耗时。指针保持、过量
capacity 的缓存成本、最后 owner 生命周期、GIF/APNG 像素和 delay 都有单独测试。

多格式静态基准使用同一 1920×1080 确定性 fixture、原编码配置与既有解码/滤镜路径，
冻结改动前/后的 executable，计时期间不运行 Cargo。每项 20 samples，最终整图结果：

| 整图格式 | before 中心估计 [95% CI] ms | final after 中心估计 [95% CI] ms | 最终轮结论 |
| --- | --- | --- | --- |
| PNG | 30.618 [30.084, 31.192] | 29.263 [28.873, 29.691] | -4.43%，p<.01 |
| WebP | 45.384 [44.803, 46.120] | 45.417 [45.045, 45.800] | +0.07%，p=.94，未检测到变化 |
| JPEG | 47.978 [47.398, 48.593] | 45.371 [44.899, 45.837] | -5.44%，p<.01 |
| BMP | 25.486 [25.048, 25.923] | 6.556 [6.396, 6.736] | -74.33%，p<.01 |
| GIF | 33.246 [32.794, 33.727] | 36.891 [35.599, 38.203] | 首轮 +10.96%；追加两轮未复现，详见后文 |

BMP target-size 对照：

| BMP 目标 | before 中心估计 | final after 中心估计 | Criterion 变化 |
| --- | --- | --- | --- |
| 320×180 | 7.9617 s | 15.126 ms | -99.807% |
| 1280×720 | 7.9510 s | 112.36 ms | -98.587% |

两项目标保持原抽样位置和 Lanczos 滤镜；大目标 remaining cost 仍包含原滤镜运算。
动画 WebP 24×320×180 resident / streamed 各 10 samples：39.863→38.664 ms（p=.29） /
43.687→43.220 ms（p=.30），均未检测到显著变化。没有新增无界 decode 并发或线程池。

GIF 整图首轮与原 baseline 比较出现 +10.964% 的显著回退；没有直接忽略该结果。
追加 before→after 对照为 33.164→32.765 ms、p=.26；再执行 after 先采样、before
随后比较：after 32.092–33.083 ms（32.560），before 32.552–33.453 ms（32.992），
before 相对 after 的变化为 +1.3277%、区间 -0.8151% 至 +3.2730%、p=.23。首轮回退
在这两轮没有复现，不能据此普遍排除所有条件的回退。追加 warmup 1 秒、measurement
3 秒；CLI 虽传 sample-size=50，但 benchmark group 固定为 20，实际各 20 samples。
GIF 320×180 target-size 在最终轮显著降低约 6.29%，1280×720 未检测到显著变化。

## Windows 独立呈现

多格式/所有权修改后的 Windows 实窗 UI Render 阻塞测试：

| backend | Render 阻塞 | presents / distinct samples（前 → 中 → 后） | Render 调用 |
| --- | --- | --- | --- |
| DX12 | 200.5016 ms | 27 → 44 → 60 | 5 → 5 → 5 |
| Vulkan | 200.376 ms | 30 → 47 → 63 | 5 → 5 → 5 |
| DX11 | 200.2548 ms | 29 → 46 → 62 | 5 → 5 → 5 |
| OpenGL | 200.4422 ms | 27 → 44 → 60 | 5 → 5 → 5 |

## 原生上传回退检查

复测与 10 月 8 日历史 Criterion baseline 对比时出现 10–26% 耗时增加，不能直接归因
到本批代码。随后在临时目录从 HEAD 恢复 gfx-memory 与 Vulkan device 的改动前实现，
使用与仓库一致的 opt-level=s、fat LTO、codegen-units=1、同一 Radeon 780M，交替
运行 before / after；普通纹理写入保留原有同步、wait 与 GPU timestamp 测量：

| backend / case | 当前条件 before 中心估计 | after 中心估计 | Criterion 比较 |
| --- | --- | --- | --- |
| DX12 / 1 tile | 144.20 µs | 143.01 µs | 未检测到显著变化 |
| DX12 / 8 tiles | 179.50 µs | 173.38 µs | 未检测到显著变化 |
| DX12 / 64 tiles | 531.69 µs | 526.35 µs | 未检测到显著变化 |
| DX12 / mip-chain | 168.18 µs | 162.91 µs | 本轮耗时降低，p=0.01 |
| Vulkan / 1 tile | 121.27 µs | 121.78 µs | 初次 +2.85%，Criterion 归为 noise threshold 内；追加反向对照 |
| Vulkan / 8 tiles | 133.55 µs | 129.83 µs | 未检测到显著变化 |
| Vulkan / 64 tiles | 168.98 µs | 166.82 µs | 未检测到显著变化 |
| Vulkan / mip-chain | 144.29 µs | 131.08 µs | 未检测到显著变化，before 有 severe outlier |

各 10 samples、1 秒 warmup、2 秒 measurement。Vulkan 单 tile 追加 B/A 反向顺序，
50 samples、1 秒 warmup、3 秒 measurement：after 118.77–121.70 µs（120.06），
before 119.06–122.67 µs（120.78）；p=0.35，未检测到显著变化。初次的小幅波动未
稳定复现。CPU 分配次数减少有独立证据；不能把本轮 native wall time 当成 GPU 指令
执行提速，也不能据此承诺所有资源规模/硬件永远没有回退。

## 撤回与未采用的候选

`DeferredFreeQueue` 的 VecDeque 版本在未就绪队列扫描中回退；连续数组的 first-ready
fast path 在部分就绪场景也有回退。两者都已撤回，队列实现保持原样。扩展对照使用
64/256/4096 个 pending payload、0/25/50/100% ready、31 轮、每轮 128 个队列，交替
执行原实现和候选实现；不是只看 zero-ready 分配数。

没有缩小 Vulkan 默认共享块：小块会使更大的资源进入 personal allocation 路径，
需要额外的大纹理 churn 对照才能决定。也没有把 DX12 buffer 上传强行改为保留 4 MiB
staging page 的方案，以免小 buffer 工作负载增加长期驻留量。

Atlas/Heap 活跃整理的策略、参考实现和限制见 [Nova GPU 整理](GPUI_GPU_COMPACTION.md)。

## 复现入口与验证限制

```powershell
rtk cargo test -p gfx-memory --features vulkan,dx12 --lib
rtk cargo test -p gfx-dx11 -p gfx-opengl --lib -- --include-ignored --test-threads=1
rtk cargo test -p gfx-dx12 -p gfx-vulkan --test texture_transfer -- --include-ignored --test-threads=1
rtk cargo bench -p gpui --bench images --features bench-support -- image_render/full_size/png --warm-up-time 1 --measurement-time 2
rtk cargo bench -p gpui --bench images --features bench-support -- image_render/png_batch_8 --warm-up-time 1 --measurement-time 3
rtk cargo bench -p gpui --bench images --features bench-support -- image_render/full_size --warm-up-time 1 --measurement-time 2
rtk cargo bench -p gpui --bench images --features bench-support -- image_render/contain --warm-up-time 1 --measurement-time 2
rtk cargo bench -p gpui --bench images --features bench-support -- image_render/ownership --warm-up-time 1 --measurement-time 2
rtk cargo test -p gpui --lib --features test-support,bench-support assets::png::tests::profile::decode_buffer_reuse_profile -- --ignored --nocapture --test-threads=1
rtk cargo test -p gpui --release --lib --features test-support,bench-support text_system::system::tests::profile::raster_bounds_eviction_profile -- --ignored --nocapture --test-threads=1
rtk cargo build -p gpui --example presentation_lane_block --features nova-gfx-dx11,nova-gfx-opengl,windows-vulkan
# 分别执行 nova-dx11 / nova-dx12 / nova-opengl / nova-vulkan
rtk proxy target/debug/examples/presentation_lane_block.exe --backend=nova-dx11
```

前批 PNG/text 的 GPUI assets 57 项测试、render-owner 17 项测试、foundation/color
19 项测试通过。
扩大文字测试得到 75 passed、
2 failed；改动前的已编译测试程序也复现了相同失败：
`raster_bounds_lru_keeps_interleaved_hot_entries` 和
`classifies_direction_bidi_and_shaping_independently_of_script_region`。压力淘汰的
热点保留与新增 equal-epoch 精确计数测试通过，不能据此宣称整个文字测试集通过。

Nova `gfx-memory` 的 24 项测试、DX11/OpenGL 的 14 项原生/registry 测试、DX12/Vulkan
的 4 项 texture-transfer 测试通过；GPUI 的 bitmap-pool 独立检查另外单独执行并通过。
GPUI no-default-features + windows-manifest,mimalloc-collect 检查通过，仍存在仓库既有
warnings，不能宣称 warning-free 或整个 workspace 测试通过。

本批没有 OS RSS/显存碎片率的长期实测，也没有新增 Linux 实窗验证。Linux native
compositor 的验证限制继续以 [GPUI 渲染事实源](GPUI_VENDOR_RENDERING.md) 为准。

多格式后续批的最终 assets 测试为 70 passed / 0 failed / 2 ignored；两项 ignored 是
需要隔离全局 bitmap pool 的计数/profile，已分别以 `--ignored --test-threads=1` 执行
并通过。最终普通并行资产测试同为 70/0/2。图片
loader 4 项和最终 source 5 项通过；新增 SVG 测试先复现旧颜色错误，再验证原地修复。
整图、target-size、owned pixels 和两个动画 WebP 对照使用冻结 executable。直接调用
Criterion executable 必须传 `--bench`，否则只是 test mode，不计为性能采样。

最终冻结的 before / after executable 与日志位于 `%TEMP%/gpui-image-profile/`：

- `images-multiformat-before.exe` SHA256 `947B4CB8DED7BB1C7DE39DE7104401E4F6A884DF2E5497520C0AB80E44D7A9EC`。
- `images-multiformat-after-finalraw.exe` SHA256 `034C5EE2E142D39D37FBEA78AEDCDF7FF2865AA197B3ADE6FE1CCFA6242C3E0B`。
- `finalraw-full_size-vs-before.log`、`finalraw-contain-vs-before.log`、`finalraw-ownership-six.log`、`gif-reverse-*.log`。
- `finalraw-assets-parallel.log`、`finalraw-gpui-check-no-default.log`、`finalraw-image-rustfmt-check.log`、`presentation-gate-*.log`。

# Nova Atlas 与 Vulkan Heap 整理

普通分配继续复用资源；活跃内容仅在明确的内存维护入口中搬迁。这里的“零拷贝”指
共享已有资源和复用空闲空间。改变颜色、透明度只更新 sprite 参数，R8 coverage 不扩成
四通道。物理释放仍有活跃内容的页/块，需要一次 GPU 内部复制。

## 参考与取舍

- [Skia Graphite DrawAtlas](https://skia.googlesource.com/skia/+/refs/heads/main/src/gpu/graphite/DrawAtlas.cpp)
  优先填前面的页，结合使用 token 和老化回收稀疏尾页。其 compact 主要淘汰缓存内容，
  之后重新生成字形；Nova 的不可变 Scene 仍可能引用 tile，因此采用稳定逻辑 ID 和
  物理 placement 映射，不照搬会使现有 Scene 失效的淘汰方式。
- [VMA 增量整理](https://gpuopen-librariesandsdks.github.io/VulkanMemoryAllocator/html/defragmentation.html)
  限制每次 pass 的字节与 allocation 数；重建原生资源、复制、等待完成、更新绑定、
  释放旧资源。Nova 保留现有 allocator，增加只在已有块中分配的严格入口，不引入
  第二套 allocator 或完整 arena 重建。
- [glyphon Atlas](https://raw.githubusercontent.com/grovesNL/glyphon/main/src/text_atlas.rs)
  将 mask 与 color 分开保存，按 generation 保护正在使用的字形。Nova 继续使用
  Mono R8 / Color BGRA 和现有 Scene、Submission 生命周期，不为整理保留 CPU 像素副本。

这些是设计参考，不能作为 Nova 已达到相同性能的证据。

## 当前实现边界

Atlas 每次 moderate trim 最多处理一页、128 个 tile、8 MiB padded texel。仅考虑
利用率不超过 50% 的非 dedicated 大页；先试已有同类页的空洞，再尝试更小的新页。
所有 trial 分配使用 allocator 副本，提交失败不发布新 placement。待上传的源页暂缓。
拷贝覆盖 padding。logical TileId 不变，旧 Scene 在编码时解析新坐标并重新分批；
缓存命中、同尺寸 refresh、pending retirement 同步更新。

源 GPU 页通过原有资源销毁和 Fence 机制退休。DX11、DX12、Vulkan、OpenGL 使用各自的
原生区域复制；Metal 提供 Blit 代码，未编译/测试。当前 Metal 原生纹理创建/上传仍有
既有未实现路径，因此不能声明 Metal Atlas 搬迁已可运行。

Vulkan 每次只选一个 general block：占用不超过 25%，活跃 allocation 不超过
8 MiB / 128 项，块容量至少 4 MiB，且同 memory type 的其他现有块有足够空闲字节。
准确的 alignment、buffer-image granularity 与连续空洞由严格分配入口检查；放不下
则回滚，不创建新的 VkDeviceMemory。未提交 encoder 存在时跳过。idle staging page
仍占用源块时也跳过，避免只搬一部分却不能释放整块。

Heap 搬迁在 GPU owner 的显式维护边界等待旧提交；复制完成后原位替换原生 buffer、
image 和 view，重写 descriptor set，保留公开资源 ID。复制等待失败时，目标资源和
命令池进入 Fence 退休队列，不提前释放。最后释放空块。此路径有明确等待，不能声明
任何负载下零 stall，也不进入普通逐帧上传或 draw 路径。

`MemoryCompactReport` 区分 moved bytes/resources、reserved before/peak/after。
reserved 是 allocator backing，不等同于进程 RSS、驱动驻留或 OS 显存预算。

## 验证入口

四个 Windows 后端共用 `production_glyphs_and_images_reach_native_pixels` hardware gate：
R8/BGRA 区域复制、重叠目的区域、源退休、Atlas padding、旧 Scene 搬迁后的 native pixels。
Vulkan gate 另构造碎片块，检查资源 ID、绑定、预算、实际释放与 backing 峰值。
CPU 单测覆盖 trial 回滚、pending upload、已有页合并和 compact 后同尺寸 refresh。

测试日志在 `target/p2c-memory/`。这些是定向 correctness/allocator 检查；实际窗口的
长期碎片率、RSS/VRAM 峰值与 frame p95/p99 仍需独立测量。

2026-10-09 Windows 定向验证中，四个后端的原生 gate 均通过。Vulkan 混合资源
样例搬迁一个 1 MiB Buffer 与一张 Atlas 纹理，backing 从 48 MiB 降到 15 MiB，
峰值仍为 48 MiB；随后以现有 view/set 绘制验证纹理内容。独立 Buffer 样例逐字节
读回 1 MiB 内容，backing 从 40 MiB 降到 8 MiB，峰值仍为 40 MiB。

独立样例最初释放了所有目的块，只留下源块，因此整理正确跳过；保留一个目的块的
活跃 allocation、提供真实已有空洞后通过。没有通过放宽“不新建 backing”来使测试通过。
完整命令、设备与结果以 `target/p2c-memory/summary.md` 为准。

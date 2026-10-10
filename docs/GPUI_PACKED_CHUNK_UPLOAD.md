# Nova Retained Packed Chunk 分段持有与上传（P2-B）

## 数据所有权

当前 Scene 只晋升满足条件的静态 Quad chunk：至少 32 个同类 Quad，无动画记录。
本批让这条已有链路使用真实的分段 CPU storage；其它 packed streams 保持现有布局。

chunk 首次编码直接写入其独立 Vec，缓存和当前帧共用 `Arc<Vec<u8>>`。重放只增加引用，
不再把 chunk 复制回整帧 Quad Vec。相同 ID 的脏 chunk 若 backing 已无其它引用，复用其 Vec；
存在其它引用时创建新内容，不修改旧版本，也不通过 `Arc::make_mut` 复制旧 payload。

非 retained、动画和 backdrop tint 的 Quad 写入可复用的 owned staging。分段描述保存
全局逻辑 offset，GPU Buffer 仍连续，batch `first/count` 和 shader ABI 保持不变。
动画 patch 用逻辑 offset 定位 owned 段，不能写入静态 chunk 的共享 backing。

## Hash、上传与 Fence

静态段复用编码时的 hash；可写段继续对真实字节计算签名，包括 GPU 动画槽号。
GPU Dirty Range 与各 source segment 求交后，通过统一 Batch 的 `push_at` 借用切片上传。
源引用只需活到 batch 调用返回；后端须按已有契约复制到可靠 staging 或消费数据。
不同 source 不为合并调用而先做 CPU flatten。这里减少 CPU 副本，不宣称 GPU 上传零拷贝。

原有固定位置的 clean chunk 跳过上传；移动位置、代次/内容变化、扩容或 P2-A 创建的新
Buffer 都按已有驻留规则处理。新版本第一次上传必须覆盖全部 segments 和 gaps，不能
跳过尚未初始化的 clean chunk。P2-A 的在途 Buffer、资源集和 Fence 生命周期继续生效。

## 回收与指标

Scene working set 裁剪缓存；当前帧仍持有的段在重置时释放引用。Moderate/Aggressive trim
可以清缓存，但保留当前呈现所需的共享 payload。后续新 encode 不会再重放被清除的缓存。

`buffers[quads].cpu` 只报告 owned staging；`cpu_retained_chunks` 对 cache 和当前帧的
共享 backing 去重。`cpu_frame_capacity_bytes` 加入段描述容量及唯一共享 backing，
不再把 chunk 的缓存和帧引用当作两份 bytes。GPU 请求量仍按唯一 Buffer ID 统计。

`retained_chunk_reused_bytes` 表示无需重新编码、无需复制回整帧 staging 的共享字节量；
hash bytes 只计未缓存 payload，`quad_upload_bytes` 继续计实际 dirty writes。

## 验证边界

测试覆盖缓存命中与同代脏重编码、源 backing 身份、共享内存去重、清缓存后当前段存活、
逻辑顺序、动态 patch、dirty range 跨段求交及独立源的目标 offset。
Windows 原生 gate 覆盖 DX11、DX12、Vulkan、OpenGL；Metal 接入同一数据与上传路径，
按要求不编译、不测试。真实 BMCBL 窗口的 RSS、多窗口峰值和帧 p99 尚未测量。

日志入口：`target/p2b-memory/summary.md`。

本批验证：Nova 单测 182 项通过，统一 Batch 单测 8 项通过；四后端 Chunk 与
glyph/image 原生 gate 各 4 项通过，GPUI minimal check、定点格式与 diff 检查通过。
Chunk gate 中两段共用 12,288 B payload，变化 gap 使用 192 B owned staging
（容量 256 B），重放 payload 复制为 0 B；该场景 Quad 上传从 12,288 B 降到
192 B，COW 新版本完整首次填充为 12,480 B。分段源可能产生多个后端写入调用，
这些结果不代表调用次数减少，也不能外推为真实窗口的内存或延迟收益。

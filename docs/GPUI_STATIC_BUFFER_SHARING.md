# Nova 两帧槽静态 Buffer 共享（P2-A）

## 所有权与写入边界

两个 frame slot 在初始化时共用静态 Buffer，分别持有 ResourceSet。每槽的
Global（含 presentation clock）和 Animation Values 独立；共享静态记录中的动画槽号
读取当前 ResourceSet 绑定的动画值表，不修改静态记录。

共享覆盖 Text Raster、Quad、Shadow、Path Vertices、Path Sprites、Mono/Poly Sprites、
Underline、Blur Pass 和 Blur Records。含 CPU fallback `animated_primitives` 的流不共享；
CPU 重写 Blur Pass 时也按槽隔离。曾被 CPU patch 的版本不能仅因本帧没有 patch 就重新
视为静态，必须重新上传静态内容后才允许继承。

## 内容版本与 Fence

在现有 submission 流程选出 Fence 已完成的槽后：

1. 当前内容 token 与另一槽已上传、可共享的版本相同：借用该 Buffer 和 capacity，
   重绑当前槽的 ResourceSet，继承 resident signature 和 Quad chunk layout，跳过上传。
2. 同一 Buffer 的所有引用槽均已空闲：原地更新，保留原有 Dirty Range 上传；写入前失效
   其它槽的旧 token 与 Quad layout，防止回到旧 Scene 时误用已覆写的内容。
3. 内容变更而另一槽仍在途引用该 Buffer：为当前槽分配新版本，全量首次上传；另一槽继续
   引用旧版本。新版本不能沿用旧 Quad resident ranges，否则未写入的间隙无有效内容。
4. 扩容与重绑成功后才发布新 ID。创建或重绑失败时回滚新资源，不改变已有槽。

不为每次 Scene 变更新增全局 GPU drain。每流最多有两个槽持有的 resident 版本；
旧版本失去最后一个槽引用时，相关槽已经通过现有 Fence 检查，可以释放。
窗口销毁仍先等待 submission，再对唯一 Buffer ID 去重销毁。

提交成功后，已空闲的另一槽直接继承最新可共享版本，释放失去最后引用的旧版本。
这不产生 GPU copy 或重复上传；即使快速 GPU 长期只选择槽 0，也不会让空闲槽永久保留
过时的静态内容。仍在途的槽保持原版本，CPU fallback 动画流仍独立。

Atlas、Path Mask 和 Blur targets 的资源集都必须随当前槽版本一起更新；Path Rasterization
也使用新顶点 Buffer 的真实容量。DX11、DX12、Vulkan、OpenGL 和 Metal 走同一套版本选择，
保持各后端已有的上传机制和提交生命周期。

## 指标

`BufferMemoryProfile.gpu_capacity_bytes` 按唯一 resident Buffer ID 统计请求容量。
在途旧版本也计入；`gpu_slot_capacity_bytes` 记录每槽绑定容量，共享时其总和大于唯一容量。
两者均不是驱动实际驻留显存；实际 allocation、reservation 和预算继续由 Device Profile 提供。

按当前初始容量，原来的两个槽请求 2,882,672 B。共用十个静态流后，请求 1,445,456 B，
减少 1,437,216 B（约 49.9%）；动态、在途内容切换和增长会影响运行时实际收益。

## 验证边界

静态继承测试须验证第二槽不再上传静态记录；版本切换测试须在旧帧已提交后更新另一个槽，
检查不同 Buffer ID、旧/新像素、Fence 后回收，以及稳定后再次合并。另覆盖 CPU patch、
扩容、ResourceSet 重绑和唯一容量统计。Windows 原生测试覆盖 DX11、DX12、Vulkan、OpenGL；
Metal 仅接入代码，不执行编译或测试。原生小场景验证不能代替真实多窗口的驻留显存和帧尾延迟测量。

2026-10-10 Windows 验证：Nova 单测 180 项通过；四后端的在途版本/空闲原地更新测试
及文字/图片原生像素测试分别 4 项通过。测试确认 14 个唯一 Buffer、1,445,456 B 初始请求，
第二槽同内容上传从 248 B 降到 24 B（仅 Global）；内容变更上传 192 B，旧/新像素、失败回滚、
Fence 后合并、空闲别名失效与旧 Scene 重传均通过。完整日志入口见
`target/p2a-memory/summary.md`。此数据来自原生测试夹具，未测真实 BMCBL 窗口的长期显存和帧 p99。

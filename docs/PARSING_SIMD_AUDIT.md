# 文件解析与 SIMD 验证记录

2026-10-13/14，Windows x86_64，AMD Ryzen 7 7840H。测量使用当前 workspace
`dev` profile（`opt-level = 3`）及 `release` profile（`opt-level = "s"`、thin LTO）。
这些是暖缓存测量，不是冷盘性能结论。所有游戏资源只读；
世界测量使用独立快照。SIMD 统一使用 workspace 精确固定的 `fearless_simd = 1.1.0`。

## BRArchive

自行维护的 [`bedrock-brarchive`](../crates/bedrock-brarchive/README.md) 解析 v1
描述表，按名称读取单个 payload，不解包整份资源、不解码图片。格式资料来自
[原始格式分析](https://gist.github.com/santiago046/ffc40925c5a78eb88f7b597a72aa6f0a)
和[独立格式记录](https://github.com/bedrock-crustaceans/brarchive/blob/main/FORMAT.md)，
并与本机 26.40 数据交叉核对；没有引入其实现库。v1 容器自身没有压缩算法。

Fixture：`target/debug/BMCBL/versions/26.40/data/resource_packs/vanilla/__brarchive`。
434 个容器、10,273 个唯一条目、4,962 个空注册项，索引/范围错误为 0。
哈希见 crate README。小于 32 个记录的容器及小于 16 字节的名称保留标量路径；
较大名称的保留字符检查使用安全 SIMD，x86 选择上限为 AVX2。

同进程交替 Scalar/Auto、每组 61 轮，5 次测量的全体预加载索引解析收益为
4.32%、5.09%、4.80%、5.63%、5.35%；完整元数据逐项相等，校验标识
`5e2baf900d154f99`。文件打开/索引阶段变化在 -2.93% 至 +2.57% 收益之间，
没有稳定的整体 I/O 提速证据。不能据此保证所有 CPU、输入或冷盘均无回归。
release 对照同样核对完整元数据，available backend 为 AVX2；一次 61 轮
中位数为标量 2.0803 ms、Auto 1.8909 ms（9.10% 收益），另外两次同配置
复测收益为 9.62%、8.80%。文件打开阶段
41.8088/41.4071 ms（约 0.96%），仍不足以确认整体 I/O 收益。

```powershell
cargo run -p bedrock-brarchive --example profile -- PATH_TO_PACK/__brarchive 61 --io
```

## 已拒绝的 JSON 快速路径

尝试先用 SIMD 保守扫描注释开头和尾逗号，大型标准 JSON 直接借用原文本，
其它内容回到现有两遍预处理。115 个资源 JSON（261,987 字节，其中 5 个大于
4 KiB）的纯解析测试收益约 8.26%；包含字符串 URL 或靠近末尾注释的 64 KiB
输入增加约 1.4%–2.4% 开销。整包 31 轮暖缓存加载，三组 baseline/candidate
中位数分别为 54.643/55.247、63.708/66.302、56.632/53.175 ms；没有稳定收益。
输出校验标识均为 `0362358582c608cb`。

候选预扫描及直接 SIMD 依赖已移除，生产 JSON 预处理保持原实现。
[`profile_pack`](../crates/bedrock-block-model/examples/profile_pack.rs) 保留为
完整块定义、纹理表及 geometry 加载的可复现测量入口。

## 真实世界快照

用户提供的 `Bedrock level` 被复制到
`target/resource-parse-check/fixtures/bedrock-level-20261013`，只包含 `level.dat`
和 `db`。237 个文件，共 462,438,905 字节；逐文件 SHA-256 在复制前、复制后、
源文件复核及只读扫描后保持一致。快照清单 SHA-256：
`9d8703c6e9dc258f35300b0f278f1feecaa7d2abcab331a85d0b3b790d69cbd7`。
不将 Minecraft 世界数据加入 Git。测量通过 `open_read_only` 打开快照，
扫描时借用记录，只复制 SubChunk；重复解析计时排除数据库访问。

全库 1,769,478 条可见记录、111,883 个区块、900,840 个 SubChunk。
区块数使用 `BedrockDbKey` 分类；直接尝试 `ChunkKey::decode` 会多计 4 个非区块位置。
版本分布：v8 6,531，v9 894,309；palette 存储位宽分布：

| 位宽 | 存储数 |
|---|---:|
| 0 | 15,009 |
| 1 | 79,092 |
| 2 | 105,526 |
| 3 | 310,535 |
| 4 | 360,732 |
| 5 | 97,914 |
| 6 | 15,116 |
| 8 | 533 |

全体 NBT/palette 解析及索引范围验证错误为 0。每 64 个 SubChunk 取一个，
得到 14,076 个缓存 payload；原实现 17 轮中位数 FullIndices 278.345 ms、
CountsOnly 224.711 ms、PackedIndices 123.074 ms。这是不同输出工作量的比较，
不能当作同一功能的加速比；3D 随机访问已使用 PackedIndices 时并不执行完整
4096 项解包。

最终实现仅在 AVX2 CPU、完整 4 位索引的 FullIndices 路径启用；其它位宽、
CPU、CountsOnly 与 PackedIndices 保持原算法。CPU 检测通过 LazyLock 缓存，
可关闭 `simd` feature。每批两个 packed word 解出 16 项，使用 Fearless 的向量
`narrow` 和 `store_slice`，不依赖标量数组转换的自动向量化，无新增 unsafe。
截断输入及越界索引保留标量实现的第一个错误。

同进程 Scalar/Auto 交替，对每种位宽取最多 256 个真实 palette（31 轮），
完整解码用 14,076 个缓存 SubChunk（17 轮）。全库每一个 palette 的标量/Auto
结果逐项相同，抽样 SubChunk 的完整 states、indices、counts 结构也逐项相同。
固定样本 Xxh3-128 标识：`3310342e88890926d718a2e3c5808afc`。
最终 release 首轮：4 位 1,319.8/136.5 μs（89.66% 收益），FullIndices
266.373/241.794 ms（9.23% 收益）；CountsOnly 194.375/196.993 ms，
PackedIndices 104.721/104.739 ms。后两条不执行新增内核；计时差异不应被解释为
功能收益。此轮存在并行编译活动，绝对耗时不能用于跨机器比较。
编译结束后的最终复测：4 位 997.8/90.1 μs（90.97% 收益），FullIndices
255.829/235.224 ms（8.05% 收益）；CountsOnly 188.179/188.268 ms，
PackedIndices 102.159/99.382 ms。样本标识、记录数量、错误数与输出一致。

最初的 SIMD 候选用 `array::map` 缩窄数据，在 dev 有收益，release 的 4 位路径
却慢约 6%；已删除该转换方式。最终是否保留以 release 实测为门槛。

```powershell
cargo test -p bedrock-world --lib --no-default-features --features bedrock-leveldb,simd --release --target-dir target/resource-parse-check profile_world_snapshot -- --ignored --nocapture
```

## 其它候选的边界

| 路径 | 已确认事实 | 本次决定 |
|---|---|---|
| NBT IntArray/LongArray | 连续数值读取有逐项 endian 转换 | 尚缺数组长度与占比测量，保留实现 |
| LevelDB CRC32C | 现有 crc32c 依赖已做硬件分派 | 不重复实现指令分派 |
| LevelDB zlib | 现有 zlib-ng 已支持 CPU 优化 | 不另建解压器或 GPU 上传路径 |
| 地图导出资源加载 | model 与 OBJ resolver 部分 JSON 存在重复解析 | 可继续测量复用收益，未增加全局缓存 |
| 插件 WASM SHA-256 | 安装校验与 cache key 可能重复哈希 | 只读调查确认调用，未测量占比、未改变完整性校验 |
| ZIP/导入检查 | 部分流程重复枚举/解压的可能性 | 缺少完整调用与耗时证据，不删除验证步骤 |

本次不改变图片解码器、线程池、UI 渲染、世界写入协议或其它无测量证据的解析器。

## 最终定向检查

- BRArchive：all-features 和 no-default-features 各 11 项测试通过；
  all-targets Clippy `-D warnings` 通过。
- block-model：119 项测试通过；BRArchive/block-model bench targets 编译通过。
- SubChunk：all-features dev 及最终 release 各 25 项通过、1 项大 fixture 测量忽略；
  no-default-features 回退矩阵 1 项通过。真实世界测量单独显式执行并通过。
- 世界快照 237 个文件在所有最终测量后 SHA-256 均未改变。
- world lib Clippy 退出码 0；现有库有 577 个警告，新 `indices` 模块无诊断。
- 依赖树只包含 `fearless_simd 1.1.0`；`git diff --check` 通过。
- 定向 rustfmt：新增文件及其它受影响文件通过；`obj.rs` 现有测试内一处
  长 `format!` 表达式有格式建议，位置未属于本次 diff，未做无关格式化。

未运行完整工作区构建、GUI 3D 预览或其它 CPU/平台的性能验证。MCB payload
保持原 bytes；模型 JSON 消费者显式报告不支持 MCB，不声称解析其编译格式。

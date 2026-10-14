# 解析与网络资源的内存所有权

## 本轮实现

- 资源文本读取：无 BOM 的有效 UTF-8 直接通过 `String::from_utf8(Vec)` 接管文件读取
  缓冲区。原来先复制整个字节数组再构造 String；UTF-8 BOM、UTF-16 和损坏输入的
  处理规则保留。解析产生的最终字符串仍按领域结构持有，不把整个原文件留在 UI。
- 导入图标：`PreviewIconData.bytes` 改为 `Arc<Vec<u8>>`，解析完成时只包装原 Vec，
  不复制载荷。父/子预览的图标 clone，以及 UI 的 `EncodedImageBytes` 构造，
  都复用同一压缩字节分配。Serde 的字节数组表示不变；不增加缓存、线程池或 allocator。
- UI 预览：删除单独保存的首个预览副本，从 `batch_previews` 借用第一个成功结果。
  保留旧的“第一个成功预览”规则，而非简单使用第一个文件；失败、清空和批量顺序不变。
  结果到达时记录其索引，读取仍为 O(1)，不在 render 中重新扫描列表。
- GPUI HTTP 适配：reqwest 的响应 `Bytes` 直接移入现有 `AsyncBody::from(Bytes)`，
  删除中间 `to_vec()`。HTTP 状态、headers、错误传播和现有运行时不变。

这里的零拷贝指已有用户态缓冲区在上述交接点不再复制。ZIP 解压、UTF-16 转码、
图片解码、GPUI 读取 HTTP body、GPU staging 和上传仍可能需要分配或复制。
共享 Vec 的 spare capacity 会保留到最后一个拥有者释放；没有为了缩容再做一次复制。
GPUI 对 `Owned` 图像已有按 capacity 计费的缓存预算，本轮不改变其生命周期策略。

## 可验证结果

`cargo test -p bmcbl --features bench-support --test resource_parsing --locked`：
7 项通过。新增验证覆盖 UTF-8 BOM、UTF-16 LE/BE 与无 BOM 回退；图标 clone 的
`Arc::ptr_eq`、图标 JSON 字节数组不变、UI source 释放后最后一个强引用归零；
HTTP `Bytes` 移交后指针不变。原 CRC、CP437、语言路径和 EXE 顺序测试继续通过。
24 个真实包的 48 条 scan/preview 结果，加上用相同路径参数复测的 1 条版本结果，
全部结构化输出与本轮修改前一致，原包 SHA-256 未变。
最终索引版本重新编译后 7 项测试仍通过，dev 基准 target 编译和 `git diff --check` 通过。
未改动的导入窗口代码有既有 rustfmt 建议，未为本轮做无关格式化。

基准使用实际资源包的 77,799 字节图标，在 `dev opt-level=3`、mimalloc 下，
每批 1,000 次、每项 9 轮，比较原交接表达式与共享表达式，单位 ms：

| 交接操作 | 第 1 次复制 → 共享 | 第 2 次复制 → 共享 |
|---|---:|---:|
| EncodedImageBytes 构造 | 1.083 → 0.003 | 1.077 → 0.003 |
| AsyncBody 构造 | 1.076 → 0.011 | 1.075 → 0.010 |

每次原表达式复制 77,799 字节；新表达式只传所有权/共享句柄。
这是根据已验证存储类型和指针相等得出的载荷复制量，不是 allocator 采样或进程内存测量。
1,000 次累计避免约 74.2 MiB 的载荷复制；这不表示峰值内存下降 74.2 MiB。
HTTP 对照使用同样大小的已有 Bytes，排除网络、TLS 和 body 收集；共享侧为了重复
测量做 Bytes clone，生产响应移交直接 move。图标对照只构造 source，不解码图片。

完整 Actions & Stuff 预览的两组交替顺序对照为 115.28→116.71、115.92→116.48 ms；
没有显示整个预览的提速。未进入本轮修改的 scan 对照也从 110.30/111.48 波动到
114.57/117.45 ms，因此不把毫秒级波动归因于所有权优化，也不宣称整包解析加速。
小型 Skins 包预览为 0.38→0.32、0.33→0.31 ms；不以微小绝对差值作总体结论。

复现方式是在已有解析基准参数后增加 `--handoff`；默认关闭，普通输出不变：

```powershell
cargo bench --features bench-support --bench resource_parsing --profile dev -- PACK_DIR VERSION_DIR 9 --handoff
```

本机原始输出：`target/resource-parse-check/memory-pair-*.jsonl`、
`memory-paired-summary.json`；基准读取原包，不安装或改写它们。

## 未扩大修改的部分

CurseForge 响应进入缓存前也存在 `Bytes→Vec→Arc<[u8]>`，磁盘缓存读写还有拼接复制。
目前保留：内存缓存按 payload 长度计费，共享底层网络缓冲可能额外保留其容量；
磁盘 Bytes 切片也可能连同 metadata 一起持有。需要测量实际响应大小、保留容量及
缓存失效后内存，再决定是否调整。JSON 结构字符串转换为 UI 类型的分配也未整体改写。

没有测量长时间运行的 allocator 碎片率、RSS、实际窗口峰值或发布 profile，不能声称
消除了碎片或实现全链路零拷贝。当前成果是移除确认的整份复制和重复 UI 数据持有。

# bedrock-nbt

不依赖世界、LevelDB、UI 或 GPU 的 Bedrock NBT 与经典 SNBT 编解码库。
bedrock-world 依赖本库，世界字段语义与 level.dat 文件头仍由上层管理。

~~~rust
use bedrock_nbt::{parse_root, serialize_root, snbt};

let tag = snbt::parse(r#"{LevelName:"Old world",RandomSeed:-42L}"#)?;
let bytes = serialize_root(&tag)?;
assert_eq!(parse_root(&bytes)?, tag);
# Ok::<(), bedrock_nbt::Error>(())
~~~

## 编码与兼容边界

- parse_root / serialize_root：无名小端 Compound 根节点。解析只读取第一个根；
  parse_root_with_consumed 返回消费字节数，parse_consecutive_roots 读取全部连续根。
- binary::parse / binary::serialize：保留根名称，支持任意根类型（含单字节 End），
  显式选择 LittleEndian 或 NetworkLittleEndian。网络 Int/Long 与容器长度采用
  ZigZag VarInt，字符串长度采用无符号 VarInt，Short/Float/Double 仍为定长小端。
- Pocket Edition 与现代 Bedrock 磁盘 NBT 使用小端编码；MCPE 0.16.0 起的网络
  VarInt NBT 是另一种编码，不从文件内容猜测。游戏版本不等于编码种类。
- 保留非 UTF-8 字符串载荷为 ByteString，并保留项目已有 ShortArray（tag 100）
  扩展。名称须为合法 UTF-8。未知类型 ID 明确报错，未知字段名正常保留。
- 空列表接受任意元素标签与非正长度。值模型只存元素，因此写出时规范化为 End/零长度。
  Compound 保持字段顺序，同名字段以最后值为准。Compound 根快捷接口丢弃根名称；
  需要保留时使用 binary::NamedTag。
- NbtReader / NbtView 提供借用字符串与事件载荷，仅支持磁盘小端。
  事件访问者提前退出时不校验剩余输入。
- 不处理 Java 大端/modified UTF-8、压缩或世界版本转换。
  NBT 出现前的 Pocket Edition 原始二进制文件需要单独的历史格式读取器。
  编码能力不等于已验证所有历史存档。

## SNBT

支持 Compound、同类型 List、B/I/L 数组、布尔值、数值后缀、单引号/双引号 UTF-8 字符串。
用反斜杠转义当前引号和反斜杠；含冒号的命名空间字符串需要引号。
无引号数值溢出时按字符串保留，类型化数组拒绝类型不匹配的元素。
parse 要求消费完整文本，错误带字节偏移。

to_string 明确拒绝 End、ShortArray 与原始 ByteString，避免发明非标准文本形式。
非有限浮点采用 Rust 的 NaN/inf 拼写并附带类型后缀，文本不保留 NaN 载荷位。
本库实现经典 SNBT，不包含新版 Java 命令表达式语言。

嵌套上限为 128，字符串为 65535 字节，ByteArray 和 SNBT 输入为 32 MiB。
二进制读取器先检查输入剩余字节再按声明长度分配数组或列表。
文件与批次总内存预算由调用方限制。

## 可选 CPU 加速

默认 feature：

- simd：fearless_simd 运行时分派扫描长引号字符串片段。先用标量扫描 64 字节前缀，仅在剩余输入至少 256 字节且前缀无分隔符时分派；
  不使用 unsafe 转换、native-only 编译选项或强制指令集，不支持的目标保留回退路径。
- parallel：独立文档批次使用 Rayon 共享线程池，保持输入顺序和逐文档错误。
  文档边界由调用方提供，不任意切割递归树，不为每个文档创建线程。

default-features = false 可关闭两者。小任务直接顺序调用。
本库不拥有 GPU、世界写入或全局线程池配置。

~~~powershell
cargo test -p bedrock-nbt --all-features
cargo test -p bedrock-nbt --no-default-features
cargo bench -p bedrock-nbt --bench snbt_scan --no-default-features
cargo bench -p bedrock-nbt --bench snbt_scan --no-default-features --features simd
~~~

## 参考

- [Bedrock Wiki NBT](https://wiki.bedrock.dev/nbt/nbt-in-depth.html)
- [Cloudburst NBT 编码变体](https://github.com/CloudburstMC/NBT)
- [nbtx 二进制与 SNBT 参考](https://github.com/bedrock-crustaceans/nbtx)
- [Fearless SIMD](https://github.com/linebender/fearless_simd)

磁盘编解码与值模型提取自本工作区的 bedrock-world。
第三方库用于核对格式和 API，不引入它们的值模型。

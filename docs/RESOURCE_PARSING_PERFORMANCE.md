# 启动版本扫描与资源包预览解析

后续所有权与缓冲区去重的验证见 [解析与网络资源内存](RESOURCE_MEMORY.md)。

## 范围与实现

本轮只改资源包元数据读取和已安装版本的 EXE 查找，不改图片解码、世界解析、
UI 调度、线程池或持久缓存。性能结论只覆盖下面测量的同步产品函数。

- ZIP：先用中央目录的 `name_for_index` 排除不相关条目，再打开匹配条目。
  manifest、语言、图标与嵌套包仍走原有读取、解压和 CRC 路径；安装解压的路径安全检查不变。
  文件名解码失败时保留原扫描的打开/错误处理顺序。
- 版本：只需要一个 EXE 时，按原优先级命中常用文件或 identity 文件后立即返回；
  不再先枚举整个目录并构造完整候选集合。需要全部候选的调用仍保留完整枚举。
- 未增加 SIMD：本轮已测瓶颈是无关条目的打开/seek 和目录枚举，尚无证据支持重写字符扫描内核。
- 撤回 `BufReader` 候选：虽能缩短单独 ZIP 扫描，但完整嵌套包预览的额外收益不稳定。
  配对复测还受并行编译干扰，不能认定缓冲有净收益；最终实现保留原 `File` 读取和 API。

## 样本与方法

2026-10-14，Windows 本机，`dev` profile（`opt-level=3`、mimalloc），热文件缓存。
**不是发布版 `opt-level=s`/fat LTO、冷缓存、整个启动耗时或 GUI 导入完成耗时。**

资源样本目录：`C:\Users\Administrator\Desktop\新建文件夹 (2)`。
24 个 ZIP/mcpack/mcaddon/mcworld/mctemplate，另一个 JPEG 不属于解析样本。
按文件名排序，把每行 `文件名 + 空格 + 文件SHA256` 用 LF 连接（末尾无 LF），
UTF-8 编码后整体 SHA-256：
`d74c8bf599597edd3ffdad4e56f5094f232fb9b549a495ceb021c0c330f0cd9c`。
每次测量前后核对原文件 SHA-256，没有安装或写入原样本。

基准调用产品 `scan_archive`、`inspect_archive(zh-CN)`，每个操作先预热一次，
再记录 3 轮中位数与 p90；完整预览的 JSON 序列化和结构比较在计时外执行。
24 个包的 48 条结果，加 1 条版本结果，与修改前的全部结构化输出相同；
每条均逐轮比较，包含 manifest、名称、描述、子包、图标字节等，解析错误为 0。
这些是元数据/预览对照，不能证明实际安装复制和解压速度。

基准会跳过需要持久提取 fallback 才能预览的包；本批 24 包均未跳过。
下表是修改前 → 最终保留方案，单位 ms；小于 1 ms 的差异易受调度噪声影响。

| 文件 | ZIP 条目数 | scan | preview |
|---|---:|---:|---:|
| Actions  Stuff 1.5 (resources).mcpack | 14161 | 146.350 → 92.357 | 290.339 → 96.487 |
| Actions  Stuff Skins (skin_pack).mcpack | 17 | 0.387 → 0.302 | 0.548 → 0.245 |
| Adventure Condiment Behavior v1.3.mcpack | 90 | 1.355 → 0.911 | 2.852 → 0.967 |
| Alleis Skin Pack 1.3.5.mcpack | 361 | 4.109 → 2.386 | 14.755 → 2.953 |
| Ashiatrix's 3D Skin Layer.mcpack | 18 | 0.545 → 0.247 | 1.522 → 0.307 |
| BE_BS_PvP(BETA 1.0)汉化.mcpack | 3643 | 55.106 → 33.200 | 99.187 → 34.995 |
| BestMemeFunnySkins (1).mcpack | 108 | 3.620 → 1.082 | 7.606 → 1.328 |
| BetterRTX.mcpack | 117 | 4.201 → 1.146 | 7.991 → 1.223 |
| Block_Model_Fixer.mcaddon | 2 | 0.170 → 0.083 | 22.521 → 7.476 |
| Craftable_Enchanted_Golden_Apple_ADDON.mcaddon | 18 | 0.704 → 0.305 | 2.881 → 0.444 |
| Durability Armor Viewer v2.1.0.mcaddon | 96 | 2.863 → 0.986 | 9.249 → 1.395 |
| FTRF-INFRA-Access_v1.20.0_mc1.21.120-latest.mcaddon | 2 | 0.201 → 0.087 | 79.048 → 25.139 |
| MultiPixel_Bedrock_1.21.93_rv4.mcpack | 9869 | 217.807 → 68.540 | 249.537 → 71.870 |
| Outline_Texture_Pack_128x.mcpack | 3400 | 79.612 → 31.443 | 7132.400 → 4225.982 |
| PrizmaVisuals 1.3.10.mcpack | 8897 | 186.257 → 80.795 | 423.744 → 86.889 |
| RealisCraft [v1.25.1] [Demo] [VV].mcpack | 4403 | 97.561 → 41.930 | 197.945 → 45.168 |
| [BP] Explorer's World Map  Minimap 1.2.2.mcaddon | 10 | 0.376 → 0.210 | 2.721 → 1.280 |
| [MCBE]机械动力完整版.mcaddon.zip | 943 | 16.815 → 6.085 | 69.789 → 12.281 |
| [RLCraft基岩版-地图模板]RLCraft BE v1_0_2.mctemplate | 6936 | 156.099 → 70.992 | 457.240 → 73.461 |
| [RLCraft基岩版-附加包]RLCraft BE v1_0_2.mcaddon | 6924 | 163.773 → 64.302 | 216.418 → 68.417 |
| [RP] Explorer's World Map  Minimap 1.2.2.mcaddon | 25 | 0.863 → 0.340 | 2.777 → 0.735 |
| n迷你世界.mcworld | 13 | 0.362 → 0.141 | 13.329 → 7.244 |
| 护眼大补丸25冬季v13H1.mcpack | 2891 | 61.645 → 26.192 | 5620.504 → 3444.613 |
| 服务器96294207.mcworld | 28 | 0.732 → 0.256 | 13.993 → 7.377 |

大型嵌套包仍需数秒，元数据筛选没有消除嵌套内容解压和解析本身的成本。
没有为此增加并行任务、内存缓存或删除完整性校验。

撤回缓冲候选并重新编译后，使用最终基准二进制补做 2 组交替顺序对照，每组每项 9 轮，
此时没有并行编译。两个代表包的结构化输出和 SHA-256 在所有运行中一致：

| 文件/操作 | 第 1 组修改前 → 最终 ms | 第 2 组修改前 → 最终 ms |
|---|---:|---:|
| Actions & Stuff resources / scan | 176.187 → 111.086 | 168.114 → 110.418 |
| Actions & Stuff resources / preview | 527.514 → 127.094 | 489.120 → 114.182 |
| Actions & Stuff Skins / scan | 0.292 → 0.227 | 0.291 → 0.226 |
| Actions & Stuff Skins / preview | 0.495 → 0.309 | 0.497 → 0.288 |

绝对耗时随运行环境波动，以上不与首轮数据混算；两次顺序下均保留收益。

版本样本：`target/debug/BMCBL/versions`，7 个候选目录，6 个有效版本输出。
调用实际 `get_appx_version_list_blocking`（通过既有 CPU runtime），
覆盖版本目录、manifest/PE 和构造扫描结果，不覆盖随后 Home 页配置预热与 UI 呈现。
单次样本中的版本耗时不作为主结论；另做 5 组、每组 31 轮的交替顺序对照：

| 组 | 修改前 ms | 提前返回 ms |
|---|---:|---:|
| 1 | 2.267 | 1.595 |
| 2 | 2.183 | 1.658 |
| 3 | 2.227 | 1.638 |
| 4 | 2.311 | 1.703 |
| 5 | 2.340 | 1.697 |

10 次输出完全一致；这条路径节省约 0.53–0.67 ms（24–30%），
不能将其写成整个 BMCBL 启动加速 24–30%。版本对照的候选快照包含后来撤回的
ZIP 缓冲试验，但该试验不进入版本扫描路径。

## 复现与验证边界

```powershell
cargo bench --features bench-support --bench resource_parsing --profile dev --no-run --locked
cargo bench --features bench-support --bench resource_parsing --profile dev -- 'C:\Users\Administrator\Desktop\新建文件夹 (2)' 'C:\Users\Administrator\Desktop\BMCBL\target\debug\BMCBL\versions' 3
cargo test -p bmcbl --features bench-support --test resource_parsing --locked
```

基准默认 9 轮，支持 3–101 轮。PACK_DIR 必须是目录；空目录可只测版本扫描。
日志含完整预览及图标字节，建议重定向到文件；工具只读样本，临时测试 fixture 在系统临时目录。

本机原始对照输出在 `target/resource-parse-check/import-baseline.jsonl`、
`import-candidate.jsonl`、`import-comparison.json` 和 `version-paired.json`；
这些是未跟踪构建产物，不是随仓库交付的固定 fixture。

定向集成测试覆盖 manifest CRC 损坏、CP437 文件名、语言路径大小写/斜杠/中文、
常用/identity/fallback EXE 优先级。库单测入口受已有 UI `#[test]` 宏歧义阻塞
（`src/ui/window/import/view.rs`），因此使用集成测试编译普通产品库来验证。
最终 4 项集成测试通过；dev 基准编译、受影响 Rust 文件的 rustfmt 检查、
`git diff --check` 均通过。最终再次核对 24 个原始包 SHA-256，全部未改变。

尚未验证发布 profile、其它 CPU/平台、冷缓存、真实窗口启动与安装完成延迟。
当前数据支持本机上述热缓存路径无观察到的耗时回退，不构成所有输入与环境的零回退保证。

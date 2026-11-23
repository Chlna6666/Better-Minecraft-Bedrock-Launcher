# Bedrock 方块状态、历史 ID 与版本数据维护规范

> 范围：BMCBL / `crates/bedrock-world`；面向用户、维护者和使用 PDB 的本地开发工具。本文是**数据格式、采集、核验及维护流程**规范，不是旧版数字 ID 与网络运行时 ID 可互换的声明。

## 1. 数据的含义与来源

| 数据 | 主要来源 | 必须保留的范围 | 能否当作其他 ID |
|---|---|---|---|
| 方块字符串标识符 `minecraft:stone` | 对应版本的 BDS、Script API | **完整 BDS 版本** | **不能**当作跨版本逻辑唯一身份 |
| 方块属性 `states` | BDS 保存的 Structure / SubChunk Little-Endian NBT | 完整版本、精确属性名、NBT 类型和值 | 不得丢掉 Byte/Int/String 类型 |
| 存储 `version` | 保存的方块 NBT 中的 `version` | BlockState 存储版本，**不等于**游戏构建号 | 不能用游戏版本推导 |
| 默认状态 | 对应版本 `BlockPermutation.resolve(id)`，再由 BDS 序列化 | 版本 + 字符串标识符 | 不等于第一个候选状态 |
| `legacy_numeric_block` | 旧版数字 ID + aux/meta 映射或已验证 PDB | 对应游戏/迁移端点 + `(id,metadata)` | 不能当作 BlockType 或网络 ID |
| `block_type_runtime` | 进程内类型/注册表序号，按具体可验证符号与运行版本 | 构建、平台、二进制与符号指纹 | 不能写入旧世界或跨版本复用 |
| `network_runtime` | 对应实际协议的运行时 Palette/握手序列 | 构建、协议/Palette 指纹、会话及算法 | **不能**脱离 Palette 指纹持久化 |
| PDB 符号地址/RVA | 与二进制严格匹配的 PDB | PDB SHA-256、二进制 SHA-256、架构 | 地址不是方块 ID |

**禁止**根据“两个方块的属性一样”“名字相近”“某个内部 index 数值一样”自动判定同一方块。缺少可复核证据时，写入 `unknown`/候选结果，不能制造映射。一个数字在**不同命名空间**使用是合法的；同版本同命名空间且同键冲突是错误。

## 2. 版本 API 的兼容性

依据微软发布的 [Script Module Versioning](https://learn.microsoft.com/en-us/minecraft/creator/documents/scripting/versioning?view=minecraft-bedrock-stable)、[Script API Changelog](https://learn.microsoft.com/en-us/minecraft/creator/scriptapi/minecraft/server/changelog?view=minecraft-bedrock-stable) 和历史更新日志，版本标识的重点如下。

| 基岩版本系列 | `@minecraft/server` 稳定版本 | 数据采集策略 |
|---|---|---|
| 1.19.70 | 1.0.0 | 仅能力检测；缺少 API 时用 BDS 自带文件作不完整 ID 候选 |
| 1.19.80 | 1.1.0 | 能力检测，绝不臆测 `getAllStates` 存在 |
| 1.20.0 | 1.2.0 | 仅使用存在的接口 |
| 1.20.70 | 1.9.0 | 可用的状态查询不保证 Structure 持久化 |
| 1.20.80 | 1.10.0 | Structure 相关 API 有重大扩充，必须实际能力检测 |
| 1.21.0 | 1.11.0 | 可尝试正式 Structure 流水线，但要求实测 |
| 1.21.20 | 1.13.0 | 采用 Structure + NBT；若缺失则降级为部分采集 |
| 1.21.80 | 1.19.0 | 注意 [1.21.80 中的 `BlockPermutation.withState` 修复](https://www.minecraft.net/en-us/article/minecraft-bedrock-edition-1-21-80) |
| 1.21.90 | 2.0.0 | Script API 2.x 迁移，确认 worldLoad、模块导入与调度行为 |
| 1.26.x | 按确切版本映射，不能套用 1.21.x | 定期采集最新稳定版 |

相关更新： [1.19.70](https://learn.microsoft.com/en-us/minecraft/creator/documents/update1.19.70?view=minecraft-bedrock-stable)、[1.20.80](https://learn.microsoft.com/en-us/minecraft/creator/documents/update1.20.80?view=minecraft-bedrock-stable)、[1.21.0](https://learn.microsoft.com/en-us/minecraft/creator/documents/update1.21?view=minecraft-bedrock-stable)。

**BDS 是否带 PDB 不是 API 能力条件**。如果游戏运行时能通过稳定 `@minecraft/server` 获取信息，就尝试；如果旧版没有必要的 Structure API，只导出 `script_api_probe`，**不得**写成完整状态目录。不同 API 版本是否有 `BlockStates`、`BlockTypes.getAll`、`getAllStates`/`getAllProperties`、`Structure.saveToWorld`，以 BDS 实际日志为准。

## 3. 如何获取多个 BDS 版本

工作流：

- [最新稳定版自动采集](../.github/workflows/bedrock-block-catalog.yml)
- [历史版本批量回填](../.github/workflows/bedrock-block-catalog-backfill.yml)

打开 GitHub Actions 对应工作流，选择 **Run workflow**，填写确切的 BDS 四段版本号。批量工作流允许空格或逗号分隔、每次 1～8 个已固定 SHA-1 的版本。默认代表性测试序列：

```text
1.19.70.02 1.20.80.05 1.21.20.03 1.21.80.3 1.21.100.7
```

获取流程：

1. 使用 [Bedrock-OSS/BDS-Versions](https://github.com/Bedrock-OSS/BDS-Versions) 提供的历史版本**文件名、日期和 SHA-1**作为归档参考；**只从 Mojang 官方 CDN 下载**服务端。
2. SHA-1 只用于匹配已知历史档案；额外计算下载文件的 SHA-256 以保留完整来源。不转存 Mojang 二进制。
3. 临时 Linux BDS 加载对应 `@minecraft/server`，探测接口；能保存 Structure 时写入世界 LevelDB、停止服务端并等待刷盘。
4. 用 `bedrock-world` 的 Rust CLI 解析真正的 NBT palette，保留严格类型、默认状态、`version` 并进行内容去重。
5. **完整有效**：更新 `data/block-catalog/index.json`、`manifests/`、`objects/`；**不完整**：只写 `probes/<version>.json`；**失败**：写 `historical-capture-report.json`，不得伪造覆盖率。
6. 每个版本单独运行并记录结果，不允许一个失败遮蔽其他成功版本。

若历史官方 CDN 已不可访问，保留失败事实。不得自动回退至未经核验的第三方镜像，或把其他版本的存档冒名标记为该版本。文件名、SHA-1、具体游戏版本必须严格匹配。

### 三类结果的含义

- `complete_structure_capture`：Script API/Structure 能采集并且通过**现有** Rust 结构一致性检验；不保证所有仅在运行时存在的状态都已收集。
- `partial_probe`：仅观察到部分名称与状态，没有权威 NBT `version`；不能用于破坏性转换。
- `capture_failed`：BDS 下载、脚本加载、启动、LevelDB/NBT、完整性检查等任何一步未通过；需要看报告和 Actions 日志。

## 4. 二进制去重：可以共用数据，但不共用语义身份

```text
data/block-catalog/
├── index.json                      # 游戏构建号 -> 完整 Manifest / NBT storage_version
├── manifests/<xxh3-128>.bin       # 版本内名称 -> 状态对象 -> 默认状态
├── objects/<xxh3-128>.bin         # 只包含类型化属性字典+状态组合
├── probes/<version>.json          # 不完整 API 观察值，不参与正式查询
├── historical-capture-report.json
└── id-registry/
    ├── index.json                 # 版本 -> ID 命名空间 -> 对象/证据
    ├── objects/<sha256>.bin       # 不含版本字段的去重 ID 映射表
    └── evidence/<sha256>.json.zlib # 可复核原始证据和每条 ID 来源
```

`BMCBLB01` 与 `BMCBLP01` 使用确定性排序、类型化属性及 LEB128 编码；同内容复用同对象。不同版本只更新指针、默认状态序号或不同对象。**存储对象相同，不等于方块语义相同**。校验哈希时必须字节比对，不能仅信任非密码学哈希。

对比两个已完整采集的版本：

```bash
python3 scripts/bedrock-block-catalog/diff_catalog.py \
  --from 1.21.20.03 --to 1.26.66.1 \
  --output /tmp/block-catalog-diff.json
```

报告的 `rename_candidates_not_authoritative` 只是候选；相同 `states` 和默认状态不能证明改名。需要同时核对方块行为、迁移规则或明确的符号/注册表演化记录后才能建立独立的**经审查的重命名表**。

## 5. 为 PDB 逆向准备的数据输入

本地 AI 或逆向工具只需产出**纯 JSON**，不直接修改二进制文件。通过同一个标准导入器验证再归档。

`scripts/bedrock-block-catalog/import_id_registry.py` 接受：

```json
{
  "schema_version": 1,
  "game_version": "1.21.20.03",
  "source": {
    "method": "pdb_symbol_inspection",
    "platform": "windows-x64",
    "binary_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "pdb_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "tool": "local-pdb-extractor v1 + symbol/RVA validation"
  },
  "registries": [
    {
      "namespace": "block_type_runtime",
      "entries": [
        {
          "id": 1,
          "name": "minecraft:stone",
          "evidence": "Exact symbol name, section, RVA, field layout, value and validation procedure"
        }
      ]
    }
  ]
}
```

**上例中的 `id=1`、SHA 字符串均为格式占位符，不表示 Minecraft 中石头的真实数字 ID。必须替换成实际测得的值。**

`source.platform`、二进制 SHA-256、PDB SHA-256、工具和每条 ID 的符号/RVA 证据均必填。导入源 JSON 的可机读规范见 [id-registry.schema.json](../scripts/bedrock-block-catalog/id-registry.schema.json)，并由 Python 导入器再次执行语义与重复性检查。PDB 必须与提取对象严格匹配；仅凭同一游戏发行版本号不等于 PDB 匹配。如果符号不能直接推导所需的值，应标记无法确认并**不要**填入 ID。

### 三个命名空间

| namespace | 导入键 | 必须字段 | 限制 |
|---|---|---|---|
| `legacy_numeric_block` | `id + metadata` | `id`、`metadata`、`name`、NBT 存储 `version`、精确类型化 `states`、`evidence` | 不能用注册表序号替代历史数字 ID |
| `block_type_runtime` | `id` | `id`、`name`、`evidence` | 不得携带 metadata、状态组合；仅对应具体进程/构建 |
| `network_runtime` | `palette_fingerprint + id` | `id`、`name`、NBT 存储 `version`、精确类型化 `states`、`evidence` | 必须记录运行时 Palette 指纹；不保证跨连接稳定 |

如果网络协议/哈希以有符号 32 位整数展示 ID（例如 `-1`），导入器将其规范化为同一 32 位无符号位模式（`4294967295`），不会把不同的有符号表现形式当成两个 ID；原始表示仍保留在压缩证据中。此归一化**仅适用于 `network_runtime`**，不得用于 `legacy_numeric_block`。

完整 BlockState 映射必须额外包含 `"version": 18168865`（仅为示例格式值，真实数据需要从目标 BDS/PDB 对应的 NBT 取得）。缺失版本一律拒绝，不得用游戏版本号推导。类型注册表 `block_type_runtime` 没有 NBT `states` 或 `version`，这两个字段均应省略。

`states` 使用明确的 NBT 类型：

```json
"states": {
  "open_bit": {"type": "Byte", "value": 1},
  "facing_direction": {"type": "Int", "value": 2},
  "stone_type": {"type": "String", "value": "stone"}
}
```

类型必须是 `Byte`、`Int`、`String`，不得把 `Byte(1)` 与 `Int(1)` 合并，也不得将属性名标准化成另一个拼写。`block_type_runtime` 不允许 `states`；`legacy_numeric_block` 和 `network_runtime` 必须具备准确的 `states`。

### 导入方法

```bash
python3 scripts/bedrock-block-catalog/import_id_registry.py \
  --input /path/to/my-pdb-ids.json \
  --catalog crates/bedrock-world/data/block-catalog
```

缺少对应**完整** BlockState 目录或发现未知名称时，默认拒绝。如果确实要保留尚待验证的 PDB 提取结果，可显式添加：

```bash
python3 scripts/bedrock-block-catalog/import_id_registry.py \
  --input /path/to/my-pdb-ids.json \
  --catalog crates/bedrock-world/data/block-catalog \
  --stage-unverified
```

暂存记录的 `verified_block_names=false`、`status=unverified_names`，绝不可被自动转换调用。相同版本+命名空间若输入值或证据不同，拒绝覆盖；只有完成人工差异审查后才使用 `--replace`。导入会复用内容相同的 ID 二进制对象，并以压缩的证据归档保留每条原始记录。

**重要**：`verified_block_names=true` 只表示“该字符串在同版本已采集的目录中找到”，**不代表数字 ID 的 PDB 证据经过独立验证**。因此导入器永远只输出 `identity_evidence_only`，不能自动升级为“authoritative mapping”。

## 6. 必须执行的验收与禁止事项

### 自动校验

1. 版本号、PDB 与目标二进制的匹配证据、平台和字节指纹齐全。
2. 原始 ID 的语义命名空间明确；同版本同命名空间内 `id`（或 `id,metadata`）不重复。
3. 同一个 `(version, namespace, key)` 若对应不同目标，必须报冲突，不能覆盖。
4. 跨版本名称变化单独记录，不自动把“同属性”判为“同方块”。
5. NBT 属性类型、值、`version`、默认状态严守完整保存；大小写与前缀不被私自替换。
6. 部分/不完整采集不能进入完整 Palette；ID 暂存不能进入迁移逻辑。
7. 更新不得破坏现有 1.26.66.1 基线；每一个历史版本都要求可审计的原始来源。
8. 高风险逆向输入不得包含真实用户存档、账号信息、网络密钥或不应传播的私有信息。

### 与现有 `bedrock-world` 的关系

- 已存在 `block/version/legacy_numeric.rs` 和已固定的 PocketMine 历史映射，不应被 PDB 注册表索引覆盖。
- 已存在 `block/version/authoritative.rs` 与 `block/version/target.rs` 的显式迁移/逆向验证流程；新版本目录仅提供更精确的目标状态集合，不能独自证明历史改名或转换关系。
- 现有静态 `block_state_schema.json` 不是跨版本的正确性基准；未来只能在保留版本 provenance 和验证后逐步使用新目录替代。
- `index.json` 中每个正式版本保持只读快照语义。发现版本内容不同必须通过人工审查进行显式替换，禁止静默改写历史事实。

### BDS 运行时调色板导出（`data/block-catalog/palettes/` 与 `runtime-palettes/`）

`bedrock-world::block::palettes` 提供来自 **BDS 运行时逆向**的方块调色板数据，与 PDB 静态符号证据互补：

- `palettes/*.nbt` 是 `pmmp/bds-mod-mapping` 在真实 BDS 中 dump 的运行时调色板（经 `opencollab-incubator/BedrockBlockPaletteArchive` 归档），共 51 个版本（1.09～1.26.60），**原始未加工**供开发者查看。`palettes/manifest.json` 记录每个文件的 SHA-256 与来源 URL。格式为拼接的 varint NBT root，**状态在文件中的索引 = 该版本的 network runtime ID**。
- `runtime-palettes/` 是由 `scripts/bedrock-block-catalog/gen_runtime_palette_catalog.py` 烘焙的紧凑查询 blob（CRC32 + zstd，50 个可解析版本），`scripts/.../parse_runtime_palette.py` 是 varint NBT 解析器。每条状态保留**精确类型化属性**（Byte/Int/String，实测数据只有这三种）与**持久化的 NBT storage version**（不是游戏版本号）。
- 查询 API：`runtime_palette_versions()`、`runtime_palette_for(label)`、`runtime_palette_state(label, runtime_id)`；版本 label 原样保留（大量是 beta 构建，如 `1.20.80.24_beta`），不是四段稳定版本号。
- 证据边界：这是运行时注册表观测，**没有默认状态字段、没有 legacy numeric ID**，不得用于写世界或破坏性迁移；早期（pre-1.13）dump 是磁盘 blockitem 调色板、无 NBT version，故不进入查询 blob。

## 7. 参考资料

- [微软 Script Module Versioning](https://learn.microsoft.com/en-us/minecraft/creator/documents/scripting/versioning?view=minecraft-bedrock-stable)
- [微软 `@minecraft/server` Changelog](https://learn.microsoft.com/en-us/minecraft/creator/scriptapi/minecraft/server/changelog?view=minecraft-bedrock-stable)
- [微软 1.19.70 Creator 更新](https://learn.microsoft.com/en-us/minecraft/creator/documents/update1.19.70?view=minecraft-bedrock-stable)
- [微软 1.20.80 Creator 更新](https://learn.microsoft.com/en-us/minecraft/creator/documents/update1.20.80?view=minecraft-bedrock-stable)
- [Minecraft 1.21.80 更新](https://www.minecraft.net/en-us/article/minecraft-bedrock-edition-1-21-80)
- [Bedrock-OSS/BDS-Versions（历史构建元数据）](https://github.com/Bedrock-OSS/BDS-Versions)
- [PocketMine / BedrockBlockUpgradeSchema](https://github.com/opencollab-incubator/BedrockBlockUpgradeSchema)
- [BedrockBlockPaletteArchive](https://github.com/opencollab-incubator/BedrockBlockPaletteArchive)
- [Bedrock level format（磁盘 NBT 与运行时 ID 的区别）](https://minecraft.wiki/w/Bedrock_Edition_level_format)


## 8. 已实测的历史 Linux BDS（2026-11-23）

在 [历史批量 Actions 运行 #45187970126](https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher/actions/runs/45187970126) 中，以下五个**官方归档版本**经过实际执行。详细机器可读结果见 [historical-capture-report.json](../crates/bedrock-world/data/block-catalog/historical-capture-report.json)。

| 精确 BDS 版本 | API | 已观测方块数 | 结果 | 关键原因 |
|---|---|---:|---|---|
| `1.19.70.02` | 1.0.0 | 未取得 | `capture_failed` | 脚本没有成功结束；尚无可用方块记录 |
| `1.20.80.05` | 1.10.0 | 未取得 | `capture_failed` | 缺少 `BlockTypes.getAll`；该 BDS 的候选文件未产生可用名称 |
| `1.21.20.03` | 1.13.0 | 1,189 | `partial_probe` | `StructureManager.createEmpty` 在能力探测时报 `EngineError` |
| `1.21.80.3` | 1.19.0 | 1,261 | `partial_probe` | `StructureManager.createEmpty` 在能力探测时报 `EngineError` |
| `1.21.100.7` | 2.1.0 | 1,269 | `partial_probe` | `StructureManager.createEmpty` 在能力探测时报 `EngineError` |

这三个 `partial_probe` 结果含对应版本实际通过 Script API 解析出的方块名与默认状态；**不包含权威持久化 NBT `version` 或所有合法组合**。现有正式 `index.json` 仍只有经过 Structure + LevelDB 编译校验的 `1.26.66.1`（1,480 种方块、23,953 个状态）；这些数字不能直接比较为新增/删除状态的总量。

`EngineError` 尚不能证明旧版完全不支持 Structure API：原因可能是 API 调用时序、服务端环境或保存模式。后续可做针对性修复验证，但在真实 NBT 保存成功且通过覆盖率校验前，禁止将 `partial_probe` 改名为 `complete_structure_capture`。

`1.19.70` 的失败在超时/启动阶段没有足够的结构化日志，需要检查原始 CI 诊断，不得反推其方块注册表内容。历史构建的 Script API 能力以**实际运行记录**为准，不以仅存在 API 类型声明作为成功证据。

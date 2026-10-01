# 区块性能热力图（离线评估）

地图的“性能热力图”使用已读取的存档记录，覆盖与当前地图相同的维度和查询范围。
橙色和红色是排查线索，不是实测 MSPT、TPS 或区块必然卡顿的结论。未读取、解析失败、
被查询上限截断或尚未验证的缓存都可能缺少记录；没有着色不等于低负载。

| 保存记录数量 | 橙色 | 红色 |
| --- | ---: | ---: |
| 实体 | 64 | 128 |
| 可参与刻更新的方块实体 | 32 | 64 |
| 计划刻记录 | 256 | 1024 |

任一数量达到阈值即显示对应等级，红色优先。这些是本项目可复现的筛查阈值，
不是 Mojang 的性能标准，也没有换算为毫秒。数量及规则显示在数据叠加面板中。
指针所在区块展示三项数量，便于判断着色原因。

实体按实际 Pos 所属区块计数，同一维度重复 UniqueID 只计一次，负坐标向下取整。
已知可参与刻更新的方块实体包括 Hopper、Furnace、BlastFurnace、Smoker、BrewingStand、
CommandBlock、MobSpawner、Beacon、Campfire。仅保存其存在并不证明它处于工作状态；
普通箱子和告示牌不计入这一项。计划刻按 PendingTicks 根 compound 的 `tickList` 长度计数，
不把一个根 NBT 当作一次更新；直接暴露的带 x/y/z 单条 tick 也可计数。
`currentTick` 在旧格式中可缺省，不参与条目数量判定。
计划刻记录也可能是正常未来更新，不能称为已到期积压。

实现沿用 map-info 后台记录查询、source fingerprint 校验、取消和过期结果拒绝。
缓存格式升级至 v5，保存刻方块实体计数，旧格式重新读取；绘制只消费 UI 拥有的稳定计数，
不读取数据库，也不在 render 中调度扫描。地图信息缓存是 BMCBL 的派生数据，不修改存档。

## 资料与限制

[mcbe-leveldb 的实测 PendingTicks schema](https://github.com/8Crafter-Studios/mcbe-leveldb/blob/v1.21.0/nbtSchemas.ts)
定义单个根 compound 的 `tickList`；该项目的
[格式修正记录](https://github.com/8Crafter-Studios/mcbe-leveldb/blob/v1.23.0/Changelog.md)
说明旧版本可以缺少 `currentTick`，并记录了实测后撤销连续根 tick 解析的修正。

[Mojang 的性能指南](https://learn.microsoft.com/en-us/minecraft/creator/documents/practices/improvingperformanceandresourceusage?view=minecraft-bedrock-stable)
说明密集实体、AI/pathfinding、命令和脚本工作会影响服务端刻预算；客户端渲染帧率是另一项指标。
[模拟距离指南](https://learn.microsoft.com/en-us/minecraft/creator/documents/simulationrenderdistanceguide?view=minecraft-bedrock-stable)
说明模拟范围和 ticking areas 决定哪些更新实际运行。
[脚本调度指南](https://learn.microsoft.com/en-us/minecraft/creator/documents/scripting/system-run-guide?view=minecraft-bedrock-stable)
说明每刻工作预算及分摊执行。

因此离线存档无法恢复实时区块 MSPT、玩家模拟范围、当前 AI/红石活动、脚本成本或设备性能。
热力图不扫描全部方块猜测电路运行量，也不把静态方块数量伪装成真实游戏刻耗时。

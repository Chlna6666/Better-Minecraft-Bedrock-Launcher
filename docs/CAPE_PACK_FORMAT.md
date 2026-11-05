# 披风包格式

披风包使用 `.mccapes` 或 `.zip`，也可以是解包目录。管理页导入归档时会将内容解压到游戏数据目录的 `capes/<包名>/`。ZIP 允许只有一层顶层文件夹。直接导入一张 64×32 比例的 PNG 时，启动器会自动生成完整的单披风包。

包根目录必须有 `manifest.json` 和 `cape.json`。一个包可包含 1 至 64 个披风。示例：

```text
MyCapes/
  manifest.json
  cape.json
  textures/blue.png
  textures/blue_elytra.png
  icons/blue.png
  texts/en_US.lang
  texts/zh_CN.lang
```

`manifest.json` 使用与 Bedrock 内容包相近的头和模块结构，模块类型固定为 `cape_pack`。头和模块 UUID 必须不同；版本为三个非负整数。

```json
{"format_version":1,"header":{"name":"My Capes","uuid":"11111111-2222-4333-8444-555555555555","version":[1,0,0]},"modules":[{"type":"cape_pack","uuid":"66666666-7777-4888-8999-aaaaaaaaaaaa","version":[1,0,0]}]}
```

`cape.json` 定义稳定的本地化键和纹理路径。`texture` 必需，尺寸为 64×32 的整数倍；`elytra` 可提供单独的鞘翅纹理；`icon` 可提供列表图标。省略 `icon` 时启动器会从披风纹理生成并缓存图标。可选字段还有 `rarity`（`common`、`uncommon`、`rare`、`epic`、`legendary`）、`start_date`（UTC `YYYY-MM-DDTHH:MM:SSZ`）、`item_uuid`、`persona_zip`。

```json
{"serialize_name":"My Capes","localization_name":"my_capes","capes":[{"localization_name":"blue","texture":"textures/blue.png","elytra":"textures/blue_elytra.png","icon":"icons/blue.png"}]}
```

本地化文件为 `texts/<语言>.lang`，每行 `键=显示名`。至少建议提供 `texts/en_US.lang`；缺少翻译时使用清单名称或本地化键。解析顺序为请求语言的文件、`texts/en_US.lang`、包内首个可用 `.lang`。

```text
capepack.my_capes=My Capes
cape.my_capes.blue=Blue Cape
```

纹理布局：披风纹理为标准 10×16×1 盒展开，外侧面（背面视角可见）位于 (1,1)-(11,17)，内侧面位于 (12,1)-(22,17)；鞘翅纹理为标准 `texOffs(34, 0)` 的 10×20×2 盒展开，前翼面位于 (36,2)-(46,22)，后翼面位于 (48,2)-(58,22)。图标为任意尺寸 PNG（建议 64×64）。早期开发期的 `pack.json` 布局与"裸 `cape.json` + `cape.png`"单披风布局不再被读取，遇到时按无效包跳过。

披风功能在版本设置中默认开启；关闭后管理页隐藏披风标签。1.21 以下 UWP 使用旧加载路径，管理页会提示不支持。

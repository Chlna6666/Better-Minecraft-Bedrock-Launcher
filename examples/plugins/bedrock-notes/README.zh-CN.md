# BMCBL Bedrock Notes

BMCBL Bedrock Notes 会在 BMCBL 首页显示 Minecraft Bedrock 最近的更新说明。

插件本身没有直接网络权限。BMCBL 会通过宿主管理的 HTTPS 白名单缓存请求 Mojang patch notes 接口，再把缓存文本快照提供给插件。

数据来源：

`https://launchercontent.mojang.com/v2/bedrockPatchNotes.json`

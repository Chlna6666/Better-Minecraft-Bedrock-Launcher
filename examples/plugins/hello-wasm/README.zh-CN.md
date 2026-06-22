# BMCBL Essentials

BMCBL Essentials 是 BMCBL WASM Component 插件系统的官方参考插件。

它演示了：

- 页面注册与导航入口
- 安全 UI 注入
- 宿主 toast 与插件窗口 API
- 全局事件订阅
- 通过 `.lang` 文件完成插件文本本地化
- WASM 侧只读插件配置
- 由 BMCBL 插件设置页统一渲染和保存用户配置

插件运行在 Wasmtime 沙箱中，不拥有文件系统、网络或进程执行权限。

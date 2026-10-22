# lucide-gpui

[English](README.md)

`lucide-gpui` 把 Lucide 图标集暴露给 GPUI 应用。构建脚本收集图标并生成嵌入的 `Assets`
资源源与索引字节；公共 API 是供 GPUI 资源系统使用的 `Assets` 类型。

## 边界

- 只做图标资源适配：不包含 UI 组件、布局或应用策略。
- 图标在构建期生成；新增图标应更新构建输入，而不是手写资源文件。

## 构建与测试

```powershell
cargo check -p lucide-gpui
cargo test -p lucide-gpui
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [crate 目录](../../docs/CRATES.md)

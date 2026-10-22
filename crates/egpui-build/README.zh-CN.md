# egpui-build

[English](README.md)

`egpui-build` 提供 egpui 应用的确定性构建期契约：加载/校验 `App.toml`、索引声明的资源，
并产出供原生工具链执行的平台 bundle 计划。

## 边界

- 不执行下载、归档、任务管理或平台打包工具；应用自身持有打包策略。
- 只做构建期规划，不进入运行时。

## 构建与测试

```powershell
cargo check -p egpui-build
cargo test -p egpui-build
```

## 文档

- [架构](../../docs/ARCHITECTURE.md)
- [English README](README.md)

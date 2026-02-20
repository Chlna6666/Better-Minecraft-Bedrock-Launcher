# Git Commit Message 规范

项目使用 Conventional Commits，并通过 `commitlint` + `husky` 在本地提交时强制校验。

## 格式

```
<type>(<scope>): <subject>
<BLANK LINE>
<body>
<BLANK LINE>
<footer>
```

常用最小格式：

```
<type>: <subject>
```

## type

- feat: 新功能
- fix: 修复 bug
- docs: 文档变更
- style: 格式调整（不影响逻辑）
- refactor: 重构（不新增功能/不修 bug）
- perf: 性能优化
- test: 测试相关
- ui: UI/交互改动
- install: 安装/导入/部署流程相关
- build: 构建/依赖/打包相关
- ci: CI 相关
- chore: 其他杂项

## 示例

```
feat(downloads): rename download.* after completion
fix(tauri): handle existing file overwrite on rename
docs: add commit message convention
chore: bump dependencies
```

## 校验失败怎么办

- 按上述格式修改提交信息后重新提交即可。
- 可用 `npm run lint:commit` 检查最近 20 条提交信息（适合 CI 或本地自检）。

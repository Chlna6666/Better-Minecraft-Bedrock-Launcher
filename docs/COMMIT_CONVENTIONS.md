# 提交信息规范

本项目统一使用 Conventional Commits 格式，描述内容使用中文：

```text
类型(范围): 中文描述
```

允许的类型：

- `feat`：新增功能
- `fix`：修复问题
- `docs`：文档变更
- `style`：格式或样式调整
- `refactor`：重构，不改变外部行为
- `perf`：性能优化
- `test`：测试变更
- `build`：构建或依赖变更
- `ci`：持续集成变更
- `chore`：其他维护工作
- `revert`：回滚提交

示例：

```text
fix(渲染): 修复关于页面渲染后端名称显示
docs(规范): 补充提交信息与本地校验说明
```

仓库提供轻量级 Git hook（无需 Husky）：执行一次即可启用：

```powershell
git config core.hooksPath .githooks
```

提交时 hook 会调用 `scripts/validate-commit.js` 校验首行格式。Node.js 仅用于开发期提交校验，不参与 Rust 应用构建和运行。

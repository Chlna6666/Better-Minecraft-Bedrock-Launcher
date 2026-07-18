# 提交信息规范

本项目采用 AngularJS 使用的 Conventional Commits 格式，并由 Rust 编写的
Cocogitto (`cog`) 负责提交校验、规范提交和 CHANGELOG 管理。提交主题可以使用中文或英文。

```text
类型(范围) : 中文描述
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

提交主题建议不超过 50 个字符。范围是可选的，不限制固定范围，便于 Rust crate、GPUI
模块和 CI 工作流使用各自清晰的范围名称。

安装 Cocogitto 并启用 Git hook：

```powershell
cargo install cocogitto --locked
cog install-hook commit-msg
```

创建提交时可以直接使用：

```powershell
cog commit fix "修复关于页面渲染后端显示" "渲染"
```

也可以使用 `git commit`；已安装的 `commit-msg` hook 会执行 `cog verify` 和
`cog check`。Cocogitto 是开发工具，不参与 Rust 应用构建和运行。

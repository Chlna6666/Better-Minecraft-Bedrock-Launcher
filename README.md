# Better Minecraft Bedrock Launcher

一个采用 [Tauri](https://tauri.app/) 框架，使用 Vite + React + Rust 编写的低性能启动器，专为 Minecraft 基岩版设计。

---

## 功能介绍

1. **支持多分支基岩版**  
   启动器支持基岩版、预览版、教育版、教育预览版等所有主流分支。

2. **DLL 注入**  
   启动时自动进行 DLL 注入，并支持延迟注入功能。

3. **依赖检测**  
   自动检测并提示所有启动及运行所需的依赖环境。

4. **多启动支持（单版本）**  
   同一游戏版本可多开运行，满足多窗口需求。

5. **多线程下载（支持代理）**  
   下载资源时采用多线程技术，支持代理加速，提高下载效率。

6. **锁定鼠标功能**  
   防止鼠标移出游戏窗口，提升沉浸感与操作体验。

7. **启动器插件（JS）支持**  
   启动器界面及部分功能可以通过 JavaScript 插件进行个性化定制和拓展。

8. **多语言支持**  
   已集成多语言功能（部分内容尚未完全覆盖）。

---

## 技术栈

- 前端：Vite + React
- 后端/核心逻辑：Rust
- 跨平台桌面框架：Tauri

---

## 环境依赖

- [Node.js](https://nodejs.org/)（推荐 v18 及以上版本）
- [Rust](https://www.rust-lang.org/tools/install)（建议最新版）
- [Tauri CLI](https://tauri.app/)（`cargo install tauri-cli`）
- [pnpm](https://pnpm.io/) 或 [npm](https://www.npmjs.com/)

---

## 快速开始

1. 克隆项目：

   ```bash
   git clone https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher.git
   cd Better-Minecraft-Bedrock-Launcher
   ```

2. 安装依赖：

   ```bash
   pnpm install
   # 或
   npm install
   ```

3. 启动开发环境：

   ```bash
   pnpm tauri dev
   # 或
   npm run tauri dev
   ```

4. 打包发布版本：

   ```bash
   pnpm tauri build
   # 或
   npm run tauri build
   ```

---

## 插件开发说明

插件需通过 JavaScript 编写，并放置于 `plugins` 目录。可用于自定义启动器外观与功能，具体开发接口详见项目文档。

---

## 贡献指南

欢迎任何形式的 PR 与 Issue！如果你发现 Bug 或有新功能建议，欢迎在 Issues 中反馈。

提交信息请遵循 Conventional Commits（见 `docs/commit-messages.md`），本仓库使用 `commitlint` + `husky` 在本地 `commit-msg` 阶段进行校验。

---

## 致谢与引用

本项目中的部分代码参考和致谢如下：

- `src-tauri/src/core/downloads/WuClient/protocol.rs` 文件，  
  Windows Update 协议客户端实现，参考了 [mc-w10-version-launcher](https://github.com/MCMrARM/mc-w10-version-launcher) 项目（C#，GPLv3），  
  本项目以 Rust 语言重写，遵循 GPLv3 许可。  
  原始项目作者：[MCMrARM](https://github.com/MCMrARM)

- UWP 脱离沙盒运行及多开支持相关实现，  
  参考了以下项目与文档：
    - [mc-w10-version-launcher/ManifestHelper.cs](https://github.com/QYCottage/mc-w10-version-launcher/blob/master/MCLauncher/ManifestHelper.cs)（C#，GPLv3）
    - [【UWP】修改清单脱离沙盒运行](https://www.cnblogs.com/wherewhere/p/18171253)
    - 微软官方文档：[UWP 多实例支持](https://github.com/MicrosoftDocs/windows-dev-docs/blob/docs/uwp/launch-resume/multi-instance-uwp.md)  
      相关代码采用 Rust 语言实现，遵循 GPLv3 许可。

- 参考实现修复最小化 UWP 导致停滞的相关代码，  
  致谢 [Aetopia/AppLifecycleOptOut](https://github.com/Aetopia/AppLifecycleOptOut)

- **部分解包 GDK 等实现**  
  吸收和致谢 [BedrockLauncher.Core](https://github.com/Round-Studio/BedrockLauncher.Core) 部分代码实现解包 GDK 等相关功能。

---

## 特别说明

本项目**大量使用了 AI 辅助生成代码与结构，所实现的功能与代码仅供功能演示使用**。  
**不建议将本项目用作代码规范、学习、风格或工程结构的参考**。如有编程实践或代码质量需求，请参考更专业的项目实现与官方文档。

---

## 版权声明

本项目仅供学习与技术交流，**禁止用于商业用途**。  
Minecraft 及其相关内容归 Mojang 及微软所有。

---

感谢你的使用与支持！

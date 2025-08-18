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

---

## 致谢与引用

本项目中的 `src-tauri/src/core/downloads/WuClient/protocol.rs` 文件，  
Windows Update 协议客户端实现，参考了 [mc-w10-version-launcher](https://github.com/MCMrARM/mc-w10-version-launcher) 项目（C#，GPLv3），  
本项目以 Rust 语言重写，遵循 GPLv3 许可。

原始项目作者：[MCMrARM](https://github.com/MCMrARM)

---

## 版权声明

本项目仅供学习与技术交流，**禁止用于商业用途**。  
Minecraft 及其相关内容归 Mojang 及微软所有。

---

感谢你的使用与支持！
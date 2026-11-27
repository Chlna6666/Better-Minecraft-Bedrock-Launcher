# GPUI 平台入口

应用通过 `gpui_platform::application()`、`application_with_renderer_options()` 或
`headless()` 启动。桌面平台实现通过 GPUI 平台接口提供原生窗口与输入，GPUI
负责共享场景、呈现数据和 Nova 渲染。iOS 和 Android 预留平台目录。

[English](README.md)

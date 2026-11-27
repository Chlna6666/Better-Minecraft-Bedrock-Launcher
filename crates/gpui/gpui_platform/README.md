# GPUI Platform

Native composition entry point for GPUI. Applications use `gpui_platform::application()`,
`application_with_renderer_options()`, or `headless()`. The selected desktop provider supplies
native windowing and input through GPUI's platform contracts. GPUI owns the shared scene,
presentation packets and Nova rendering. iOS and Android are reserved for future providers.

[简体中文](README.zh-CN.md)

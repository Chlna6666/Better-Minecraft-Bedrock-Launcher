# gpui_macos

供 GPUI 使用的 macOS 显示与任务调度服务。CoreVideo 显示时钟将刷新事件合并到主线程的 dispatch 队列。此 crate 从系统 SDK 生成 libdispatch 绑定。

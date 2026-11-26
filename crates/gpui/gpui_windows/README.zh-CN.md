# gpui_windows

供 GPUI 使用的 Win32 服务。显示时钟管理合成器、显示器和原生窗口缩放的唤醒事件。对话框模块负责原生调用和 COM 单元的生命周期。GPUI 消费唤醒事件、管理窗口树，并通过 Nova-GFX 渲染。

```powershell
cargo test -p gpui_windows
```

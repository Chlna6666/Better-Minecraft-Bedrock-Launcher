# gpui_windows

Win32 services used by GPUI. The display clock schedules compositor, monitor and native resize wakes. The dialog module owns the native dialog calls and COM apartment lifetime. GPUI consumes the wake events, owns the window tree and uses Nova-GFX for rendering.

```powershell
cargo test -p gpui_windows
```

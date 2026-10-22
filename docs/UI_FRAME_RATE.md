# BMCBL UI frame-rate policy

This controls the **launcher UI**, not Minecraft Bedrock's rendering.

## Configuration (`settings.toml`)

```toml
[launcher]
frame_rate_mode = "auto"
frame_rate_limit = 120
```

- `auto` (default): use platform VSync, with a per-window **120 FPS maximum**. If display refresh is lower, the display remains the pacing authority.
- `vsync`: use **native display refresh without a 120 FPS software cap**, such as 144, 240 or 300 Hz. This is still subject to backend and compositor restrictions.
- `custom`: request an explicit **1–1000 FPS** upper bound, using Immediate presentation instead of VSync.
- `unlimited`: opt out of the 120 FPS cap and VSync. Rendering remains event-driven and may be constrained by platform/driver/compositor throughput; on Windows the native frame-request worker uses a 1 ms unpaced ceiling, not a busy loop.

Custom cap examples:

```toml
[launcher]
frame_rate_mode = "custom"
frame_rate_limit = 240
```

## Launch-only overrides

```powershell
BMCBL.exe --fps-limit 240
BMCBL.exe --fps-limit 0
BMCBL.exe --uncapped-fps
```

`--fps-limit 0` and `--uncapped-fps` both select unlimited mode. These flags affect **only the current process** and do not overwrite `settings.toml`. `--fps-limit` accepts integers from 0 through 1000. Start normally without a flag to return to the persisted setting.

In **Settings → Launcher → UI frame rate**, select Auto, VSync, Custom, or Unlimited. A custom cap can be entered as an integer from 1 to 1000. Input commits on Enter or focus loss.

Changing a numerical cap updates the clocks of existing windows. **Switching between VSync and Immediate requires restarting the launcher** because the backend's swapchain present mode is chosen during renderer initialization.

## Engine behavior and caveats

- `FrameClock` coalesces frame requests and limits renderer-owned animations, rather than inserting `sleep` on the GPUI UI thread.
- On Windows, Immediate mode uses independent native scheduling instead of DWM VBlank and retains the lower background-window cadence. Native resize reconciliation remains immediate.
- DX11/DX12/Vulkan/OpenGL may implement Immediate differently. Drivers, DXGI composition, Wayland surface frame callbacks, and compositor policies may cap actual **displayed** frames at the monitor refresh rate even when additional frames are **rendered/submitted**.
- Do not equate the configured maximum with delivered FPS. Confirm with GPU trace, presented frames, frame-start timing, and live-resize tests. Test secondary window lifetime on DX12 and X11/Wayland separately.

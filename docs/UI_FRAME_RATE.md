# BMCBL UI frame-rate policy

This controls the **launcher UI**, not Minecraft Bedrock's rendering.

## Configuration (`settings.toml`)

```toml
[launcher]
frame_rate_mode = "vsync"
frame_rate_limit = 120
```

- `vsync` (default): use native display-driven presentation without a software FPS cap. The actual cadence depends on display, backend and compositor.
- `auto`: legacy alias for `vsync`; existing settings no longer impose a 120 FPS cap.
- `custom`: request an explicit **1–1000 FPS** upper bound while keeping native display-driven presentation. On a fixed-refresh display the effective visible rate cannot exceed the native cadence; non-integer ratios select alternating refresh slots.
- `unlimited`: opt out of the software FPS cap and VSync. Rendering remains event-driven and may be constrained by platform/driver/compositor throughput; on Windows the native frame-request worker uses a 1 ms unpaced ceiling, not a busy loop.

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

In **Settings → Launcher → UI frame rate**, select Native VSync, Custom, or Unlimited. A custom cap can be entered as an integer from 1 to 1000. Input commits on Enter or focus loss.

Changing a numerical cap updates the clocks of existing windows. **Switching to or from Unlimited requires restarting the launcher** because the backend's swapchain present mode is chosen during renderer initialization.

## Engine behavior and caveats

- Windows, Wayland, X11 and macOS UI commits select eligible native display/compositor ticks before layout. The Windows GPU-owner readiness report releases latched dirty work; geometry and first-frame progress remain separate. Pending GPU scenes merge damage and supersede older queued scenes.
- X11 uses Present NotifyMSC when supported and an on-demand XRandR timer fallback. macOS 14+ uses CAMetalDisplayLink with the default Metal renderer, with CVDisplayLink fallback on older systems and the optional Blade renderer. Windows DX11/DX12 use non-tearing mailbox presentation; other synchronized backends retain FIFO/VSync. Wayland GPU-owner resource/deadline retries remain separate from UI tick selection. Windows uses the native compositor clock on Windows 11, then DXGI output VBlank on older systems; DWM phase/timer waiting is a compatibility fallback. Callback wake timestamps, MSC clock samples and drawable availability are not exact hardware present timestamps.
- Vulkan optionally bounds the outstanding FIFO presentation queue with `VK_KHR_present_id` and zero-timeout `VK_KHR_present_wait` queries. The graphics fence remains a separate resource-lifetime gate. An unsupported or stalled present-wait path falls back to normal acquire/fence pacing until swapchain recreation. `VK_GOOGLE_display_timing`, when available, reports historical presentation timestamps for diagnostics.
- Wayland registers GPU-owned native frame callbacks directly through the dedicated event queue handle before WSI submission; only registration and teardown synchronize with the event thread. Callback generations prevent a stale callback from releasing a later frame.
- On Windows, Immediate mode uses independent native scheduling instead of DWM VBlank and retains the lower background-window cadence. Native resize reconciliation remains immediate.
- DX11/DX12/Vulkan/OpenGL may implement Immediate differently. Drivers, DXGI composition, Wayland surface frame callbacks, and compositor policies may cap actual **displayed** frames at the monitor refresh rate even when additional frames are **rendered/submitted**.
- Do not equate the configured maximum with delivered FPS. Confirm with GPU trace, presented frames, frame-start timing, and live-resize tests. Test secondary window lifetime on DX12 and X11/Wayland separately.

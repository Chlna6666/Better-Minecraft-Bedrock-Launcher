# Render Performance Worklist

## Implemented In This Batch

| Item | Boundary |
| --- | --- |
| GPU Owner diagnostics | Per-owner/window queue wait, merged pending age, service time and explicit submission waits; global wall occupancy and OS thread CPU time. See [measurement contracts](GPUI_GPU_OWNER_METRICS.md). |
| P0-C native buffer batches | Five backend overrides, ordered writes and fence-protected staging. DX12/Vulkan device-local copies share one submission/wait. See [backend details](GPUI_VENDOR_RENDERING.md#buffer-upload-batches-p0-c). |
| Path Mask pixel residency | Independent of DrawStep caching; committed only after successful presentation. Per-window rendered/skipped counters and CPU timing distributions are exported by `gpui_perf_lab`. |
| Dirty Entity segment index | Each Frame owns an Entity-to-stable-index map, populated with Segment insertion and carried through replay/swap/clear. No Segment objects are copied. |
| Layout Root membership | Small root sets retain linear lookup; when both sets exceed 16, the existing scratch vector is sorted after saving roots and searched by binary lookup. No new HashSet allocation. |
| RAF/refresh audit | Candidate classification below; no unverified animation migration or application refresh replacement. |

The dirty index removes the two per-damage map constructions, not every scene-wide operation.
The sibling bounds comparison is retained because layout can move or remove views that were
not directly notified. Duplicate/noncontiguous segments use multiple stable indices. The
index follows the sole Frame append method, is cleared with scratch/degraded frames, and
participates in capacity accounting and trimming. Debug Frame finalization checks its count.

The layout change improves membership complexity only. Retained bounds restoration and
subtree traversal remain linear in subtree size. The 16-root small-set threshold is a
fast-path heuristic, not a measured platform optimum. No Taffy node relationship redesign
or claimed constant-time layout cache was introduced.

## RAF And Refresh Evidence

These are code-level candidates, not measured causes of scrolling stalls:

| Path | Finding / Required Follow-Up |
| --- | --- |
| [Main window](../src/ui/main_window.rs), [chrome views](../src/ui/main_window/chrome_view.rs), [map view](../src/ui/window/map_viewer/view.rs) | Theme color interpolation still uses render-time styles and RAF. A compositor migration needs scene color bindings; removing RAF alone would freeze the transition. |
| [CurseForge detail wheel](../src/ui/views/download/curseforge.rs) | Scroll offset changes call whole-window `refresh()`. The viewport is a Div, not an Entity; first validate a targeted root notification and visible-content updates. |
| [Import overlay](../src/ui/state/import.rs) | State already knows its owner but show/clear refresh all windows. Targeting must also repaint the previous owner when replacing an overlay across windows. |
| [Map lifecycle](../src/ui/window/map_viewer/lifecycle.rs) | Tile result events call AsyncApp refresh, which reaches all windows. Resource content requires a UI update, but that update should be scoped and batch frequency measured. |
| [Modal](../src/ui/components/modal.rs) | Visual closing work and completion cleanup are coupled. A migration must preserve the single completion/dismiss event and mixed-subtree behavior. |
| [Scrollbar](../src/ui/components/scroll.rs) | Viewport changes also drive agreement completion state; not a pure GPU opacity/translation animation. |
| [Skin preview](../src/ui/window/skin_pack/preview.rs) | Per-frame walk pose is 3D scene work. GPU time-driven skinning must exist before removing UI sampling. |

`request_animation_frame()` still notifies the current/root Entity on the next platform frame.
Request coalescing does not make this API an independent compositor lane. Layout geometry,
text/list content and resource completion updates retain UI semantics.

## Evidence Still Required

- Multiwindow owner distributions under a busy main/map/auxiliary workload. The initial
  single-window Debug sample verifies collection only; it is not a contention or FPS result.
- GPU pass timestamps for main sprites/quads, path masks, backdrop capture, Gaussian axes,
  composite and transfer. CPU pass-call durations are not shader execution time.
- Cosmic Text lock acquisition and locked method wall-time, separately for shaping and
  rasterization; drop the write guard before recording. Existing text cache counters should
  be reused. Windows DirectWrite measurements do not validate this Linux-first backend.
- Per-backend dirty/scissor area, partial-present attempts and fallback reasons. Keep the
  transparent Windows composition/backbuffer correctness restriction.
- Blur/overdraw changes require timestamp and pixel-quality evidence; do not downsample
  merely to improve a CPU submission counter.
- Buffer budgets, cross-frame residency and adapter-based GPU Owners need distinct
  ownership/lifetime validation. Existing dirty-tree memory/residency work is preserved;
  this batch does not claim to finish those separate high-risk items.
- Apple-specific Metal batch compilation/runtime and Linux owner clock/runtime validation.

Validation should distinguish deterministic tests, native GPU readback and real-window
reports. Native Windows upload readback tests verify ordering and one-fence batching;
neither those tests nor asymptotic improvements establish an application-wide frame-time gain.

## Local Validation (2026-10-10)

- Core batch planner: 8 passed; upload ring: 19 passed.
- Windows native DX12/Vulkan batch tests passed without an unavailable-device skip.
  Overlapping writes/readback, empty/invalid ranges and one submitted fence were checked.
  DX11/OpenGL native uniform update pixel tests also passed.
- GPU Owner, Path Mask residency, Frame index and retained layout tests passed in their
  focused groups. Apple Metal and Linux runtime validation remains outstanding.
- A native DX12 mask test passed through real `NovaRenderer` presentation: the first red
  path rasterization and five cache-hit presentations produced identical sampled pixels;
  a green replacement caused a second rasterization with changed pixels. Readback uses a
  test-only sampled scratch target, without expanding production mask texture usage.
  This is GPU readback evidence, not physical display/scanout or other-backend coverage.
- DX12 `gpui_perf_lab` schema 4, separate presentation host, `buffer-growth`, 150 samples:
  one window with matching WindowId in window metrics and 532 raw owner jobs.
  Path Mask recorded 262 rendered and 265 skipped frames. This verifies branch activity,
  not cached-mask pixel equivalence, multiwindow contention or an application FPS gain.
- Three existing retained/damage assertions still fail: child notified bounds, generic
  dirty descendant root traversal count and degraded-draw full damage. They also failed
  when this batch's five originally-clean draw/Frame/element-context/layout files were
  temporarily restored to HEAD. The remaining dirty tree was preserved throughout;
  this is not a clean-repository baseline or a green full-suite result.

GPU render-pass queries need a separate optional backend contract and fence-retired,
nonblocking result collection. Existing DX12/Vulkan transfer queries are reusable
infrastructure, not already-implemented main/mask/blur/composite pass timing. A stable
packet frame identity is also required before claiming WindowId + FrameId attribution.

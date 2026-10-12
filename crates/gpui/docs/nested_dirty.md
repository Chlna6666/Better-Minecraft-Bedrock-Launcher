# Nested updates and retained rendering

`nested_dirty` is a finite native-window reproduction. It has 384 static tiles,
a parent Entity, nested divs, and one leaf Entity whose color changes. Only the
leaf receives `notify()`. `plain` uses ordinary Entity rendering; `cached` gives
the parent and leaf fixed layout boundaries with `CachedView`.

```powershell
cargo run -p gpui --example nested_dirty --no-default-features --features nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl -- --backend=nova-dx12 --cache=cached --blur=none --frames=120
```

Backend fallback is disabled. Repeat with `nova-dx11`, `nova-vulkan`, or
`nova-opengl`; inspect `resolved_backend` and `adapter` in the JSON output.
Adapters can differ between backends, so do not compare their timings as if
they were running on the same GPU.

Blur cases:

- `none`: no explicit filter.
- `behind`: full-window backdrop blur before the changing leaf.
- `ahead`: full-window backdrop blur after the changing leaf.
- `element-small`: sigma 16 on the 80 x 40 leaf.
- `element-full`: sigma 16 on the full page.

Add `--trace` to write per-frame dirty scopes, selective-splice results and
rendered view types to stderr. Leave it off for timing measurements. There are
120 warmup callbacks before counters reset. Samples count platform callbacks,
not successfully presented GPU frames; queued notifications may coalesce.

## Interpreting the evidence

`counts` measures actual `Render` calls for each Entity. The window root owns a
viewport-sized retained boundary. Ordinary nested entities preserve parent-driven
layout and record their weak handle, traversal context, frame-local ranges and
computed subtree geometry. A unique dirty entity can replace its retained range
without calling the root or clean ancestors' `Render`.

Ordinary-view reconciliation requires the same recursive layout fingerprint and
node count. It rebinds the exact previous computed geometry to current layout IDs,
including measurement inputs. It does not turn an ordinary entity into a fixed
layout proxy or guess parent constraints from its old size. Changed or unprovable
layout semantics require structural layout; text content changes and impure
measurement without a fingerprint currently take that conservative fallback.
The current fallback can rebuild the root even when an explicit cached ancestor
would be sufficient, and the failed leaf-layout probe can cause another leaf
Render during that rebuild. Promoting only the failed target to that ancestor
while transferring its prepared element remains unresolved.
Forced refresh, degraded recovery and unsafe capture boundaries also retain
their structural fallback. The optimization targets same-layout local updates;
it does not eliminate scene replay, GPU presentation or necessary layout work.
An arbitrary div's `ElementOnly` hover/active update still rebuilds its owning
View when no independently reconcilable View boundary exists. This patch does
not establish leaf-only reconciliation for every kind of element invalidation.
Each View also records Entity reads performed by its own Render, separately
from descendant layout/prepaint reads. A notification marks actual readers
directly dirty and applies a conservative window cache barrier, including when
a sibling reads another visible View's state. Merely composing a child Entity
does not create this read dependency, preserving independent leaf updates.
Non-view data notifications also keep the conservative window refresh until
all dependency ownership can be proven, so the root cannot conceal changed
data. Global state continues to require the explicit observer/refresh contract.

Consuming queued entity invalidations now records direct/ancestor dirty scopes
without requesting another UI frame for the work already being drawn. The
invalidation producer owns scheduling. This prevents a redundant clean UI commit.

An empty metadata prefix before the only cached child was incorrectly rejected
as invalid, despite fragment replay supporting an empty no-op. That degraded
every selective draw and forced a parent rebuild on the following update.
Fragment validation now accepts an empty range while retaining all bounds and
nonempty subtree checks.

The presentation phase activates an immutable pending scene before UI work.
After a UI draw publishes a new scene during an early presentation tick, its
continuation requests presentation, avoiding a second clean `ui_commit` that
would force cache refresh. A fresh notify still requests and merges UI work.

`renderer_memory[*].blur_target_bytes` records allocated logical texel bytes
for sources and filter/composite targets. Check `submitted_frames`,
`render_error`, and `sample_age_ms` before using a memory observation.
These bytes exclude native alignment, swapchain images, driver overhead and
GPU residency accounting. Work-pixel counts in `samples` are latest global
asynchronous observations; they are not allocation bytes or GPU durations.
Source work pixels estimate the union of source-step scissors. `partial_redraws`
counts both retained partial work and native swapchain damage, so it alone does
not prove that a backend performed a partial swapchain redraw.

Pure element filters and retained compositor layers now omit the unused
shared full-window backdrop source and its frame-slot bindings. Root-level
backdrop filters still allocate that source; nested backdrops inside an element
capture use the element’s own source. Target compatibility includes this
topology change, with the existing submission-fence wait before replacement.

Gaussian passes use a color-only render pass without a depth attachment. Source
capture keeps its ordinary scene depth. A downsampled filter target must not be
paired with the full-window depth texture; DX11 and OpenGL reject that mismatch.

Small and full element filters still allocate full-window isolated source and
Gaussian targets. Scissors reduce filtering work but do not shrink those
allocations. Compact regional targets need corresponding source viewport,
filter UV, Gaussian halo, scissor and depth-attachment handling across backends;
this reproduction must not be interpreted as completing that change.

Native partial swapchain redraw is currently enabled only on DX12. DX11,
Vulkan and OpenGL use the full presentation path. CPU subtree reuse, regional
filter work and swapchain partial redraw are separate measurements.
For eligible unfiltered scenes (at least 384 primitives with a static majority),
the existing retained-root color target can still update local source pixels
on all four backends before the final window composite. Full presentation does
not by itself widen that offscreen damage. This eligibility rule excludes
filtered scenes and does not eliminate batch scanning or full-window composition.

## Windows validation (2026-10-11)

Before ordinary-view reconciliation, 120 leaf updates produced
`root/parent/leaf = 120/0/120` with explicit cached boundaries on all four
backends, versus `120/120/120` in plain mode. Before the empty-fragment
correction, DX12 cached mode produced `120/60/120`: each selective splice
degraded and the following update refreshed the parent.

After ordinary-view reconciliation, the same 120-update native-window run
produced `0/0/120` in both plain and cached modes on DX11, DX12, Vulkan and
OpenGL. This measures CPU view construction; it does not imply that the native
presentation path skips all static pixel drawing.
Plain-mode samples also changed from 390 requested layout nodes per frame to 3,
and from 416,000 recorded dirty pixels to 3,200. The latter is CPU damage area,
not a GPU-time measurement. Main presentation still uses one pass. The demo's
asynchronous upload/command observations do not establish a before/after GPU
cost comparison; a zero command observation must not be read as no GPU work.

The ordinary-view regression checks four color updates without root or parent
Render calls, unchanged static sibling geometry, and a width change from 80 to
100 that triggers structural layout and moves the following sibling by 20.
The independent-entities regression alternates four updates to each ordinary
entity and checks that the root and untouched sibling do not Render, while the
untouched sibling's scene color remains correct. The strengthened native readback
gates compare the entire image after a plain leaf splice with a forced full
rebuild of the same state, after frame-slot and quality warmup. Each backend
checks backdrop barriers before and after the leaf, and a separate unfiltered
scene with 384 static sibling quads. The latter submits local leaf damage,
verifies retained source preservation and local source-step scissors, then checks
every pixel against the full rebuild. The fixture now uses matching 1x logical
and physical dimensions and takes damage from the committed leaf quad's scaled
bounds. The strengthened gates now pass on DX11, DX12, Vulkan and OpenGL.
They establish pixel correctness and regional source preservation, not a
GPU-duration gain. The warm gate exposed stale edge texels in downsampled
backdrop filtering: the visible composite linearly samples outside its nominal
bounds. Damage now includes that adjacent target-texel footprint and propagates
its horizontal/source convolution dependencies. The four-backend image comparison
and two focused ROI tests pass after this correction.

The 800 x 520 reproduction measured 1,664,000 logical target bytes for the
retained zero-filter root, versus 3,328,000 before removing the unused source.
Both positive-radius element cases still measured 4,992,000 bytes after that
source removal. This confirms the remaining full-window allocation issue.

The backdrop case now submits without a render error on DX11, DX12, Vulkan
and OpenGL. The native Gaussian readback gate uses a 16 x 16 source,
8 x 16 horizontal target and 8 x 8 final target to exercise downsampling.
These checks do not establish frame-time, RSS or driver-residency improvement.

Focused regressions and hardware readback can be repeated locally:

```powershell
cargo test -p gpui --no-default-features --lib empty_metadata_prefix_keeps_cached_parent_reusable
cargo test -p gpui --no-default-features --lib committed_scene_continuation_presents_without_rebuilding_ui
cargo test -p gpui --no-default-features --lib plain_nested_leaf_update_preserves_ancestors_and_sibling_geometry
cargo test -p gpui --no-default-features --lib unrelated_plain_entities_update_independently
cargo test -p gpui --no-default-features --lib non_view_entity_notify_cannot_replay_stale_root
cargo test -p gpui --no-default-features --lib rendered_leaf_notify_updates_parent_that_reads_its_state
cargo test -p gpui --no-default-features --features nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl --lib plain_leaf_splice_matches_full_rebuild -- --ignored --test-threads=1
cargo test -p gpui --no-default-features --features nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl --lib production_glyphs_and_images_reach_native_pixels -- --ignored --test-threads=1
```

The hardware gates require the respective Windows GPU APIs. They validate
native pixels, independently of the finite demo's asynchronous metric samples.

The final targeted checks pass ordinary/independent/data-entity reconciliation,
root selective splice, direct parent-state dependencies, simultaneous-target
fallback, composite-capture fallback,
expired-budget selective work and progressive dirty-view deferral. The child
pixel-damage and degraded-recovery fixtures now acknowledge the initial frame
through the real presentation path before checking subsequent local damage;
otherwise the unpresented initial full damage is correctly accumulated. Their
existing partial-damage and cache-range assertions remain unchanged and pass.

Two broader assertions remain unresolved: nearest-cached-parent promotion after
changing leaf text, and the clean next-frame callback's pending request class.
The former still rebuilds the root as described above. Both assertions also
failed in the initial baseline and at HEAD
`9d00b1b90b6e978998d916813bbbfc01bff7931b`; that comparison establishes history,
not completion or a waiver. It used identical no-default-feature invocations
and only three missing test-fixture fields in the detached HEAD checkout;
production code in that baseline was unchanged. Full workspace test and Clippy
coverage have not been run for this patch.

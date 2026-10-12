# Application animation audit (2026-10-11)

Scope: common dropdowns, their custom triggers, theme propagation, and the next
application components to inspect. Framework reconciliation and native pixel
gates are recorded separately in [nested_dirty.md](../crates/gpui/docs/nested_dirty.md).

## Evidence from the supplied Vulkan log

The supplied log has 15 frame-generation budget warnings. Of the 14 settings
records, 10 request `LayoutAnimation`, three request `Input`, and one requests
`StateNotify`. Each records RootView, MainWindowView, AppChromeView and
SettingsPageView rendering once, plus eight ToggleSwitchViews. Layout records
367 nodes, 79 measured nodes, and two cache misses. The selective splice misses
with `no_unique_target`. The last warning is a separate AuthChromeView input.
Actual generation elapsed time is 4.0059–8.7292 ms. Nested element timing totals
are inclusive observations and must not be added into frame duration.

Dropdown appears in the timing records, but the log does not identify an open
popup, theme toggle or hover. It establishes repeated application reconstruction,
not a causal attribution of every warning to the dropdown or the backend.

## Common dropdown migration

`src/ui/components/dropdown.rs` previously attached a layout-animation target to
the trigger and supplied popup opacity/reveal samples through stable sampled
wrappers. Stable identity alone did not transfer the timeline to the compositor.
The new `DropdownMotion` declares a discrete target and transition duration.
The arrow, opacity and fixed-edge reveal use engine-owned visual tracks over
final menu geometry. The three custom triggers in CurseForge, Manage and Import
use the same arrow binding; the Settings language menu uses the default trigger.

Opening remains 220 ms and closing 160 ms with cubic ease-out. Reversal retains
the remaining duration of the old cubic curve; active GPUI tracks retarget from
their presentation value. The duration is stable across unrelated owner renders.
The animation spec uses built-in `Easing::OutCubic`. Passing the equivalent Rust
function to `with_easing` creates `Easing::Custom` and forces the CPU Paint driver,
so a callback-free visual wrapper alone would not establish compositor ownership.
Closing still retains the popup and outside-click catcher until state cleanup.
GPUI's existing completion invalidates the retained owner path once at the actual
endpoint. Consuming that invalidation renders the owner, allowing removal and phase
settlement; intermediate samples stay on the presentation lane. Native completion
generation checks reject old events after a retarget. The UI-owned fallback uses
the same endpoint invalidation. No new completion API, timer or per-frame application
Notify drives these tracks.

The cached-owner regression exposed a separate framework bug: a same-layout
animation target could be hidden by a safe Div's retained semantic proof, which
does not include animation declarations. `AnimationElement` now owns a frame-local
cache boundary, so fresh declarations reach prepaint while inner content can still
reuse its cache. The retarget state also updates its property descriptor, rather
than retaining the previous endpoint descriptor after a successful retarget.
Completion retains `InvalidateSubtree`: the single-target splice now admits that
scope without weakening it. CachedView's whole-subtree replay checks retained-path
invalidation on the same owner route, so cached descendants below the animated
target refresh while static ancestors and unrelated routes remain eligible for reuse.
The splice resolves the element path to its owner's closest registered CachedView
boundary; the original dirty element path and scope remain unchanged. An animation
element id is not itself a CachedView state id. Multiple instances of an owner and
unrelated owner routes are checked by a focused resolver regression.

The overlay snapshot now compares its palette when refreshed. Previously an
open popup could keep its old colors when its owner changed theme without
changing geometry or selection.

## Remaining application paths

| Path | Current evidence | Next boundary |
| --- | --- | --- |
| Theme (`state/theme.rs`, `main_window.rs`, `main_window/chrome_view.rs`) | Theme Global changes notify observers once; root/chrome continue frame work while palette interpolation is active. Helpers use the frame clock. | A whole palette changes primitive colors. Current GPU animation properties do not support Color; a color-capable retained path or validated crossfade needs separate design. |
| Map viewer (`window/map_viewer/view.rs`, `lifecycle.rs`) | Theme frames synchronize the canvas snapshot; its cache key contains interpolated colors. | Avoid rebuilding unaffected world data when only palette changes; measure snapshot invalidation independently. |
| Home version menu (`views/home/page.rs`) | Separate SpringValue state, animated height/position and row stagger; not the common Dropdown. | Preserve spring/stagger semantics and virtualize/bound genuine layout work before migrating compatible visual parts. |
| Modal (`components/modal.rs`) | Sampled backdrop opacity and content scale/opacity. | A retained composite is plausible, but mixed text, clipping, hit targets and exit lifecycle need pixel checks. |
| Import spinner (`window/import/view.rs`) | SVG rotation is sampled with the frame clock and wrapped in a layout target. | Replace rotation with a repeating visual timeline; keep progress/content updates separate. |

No theme tick using `refresh_windows()` was found. The modal's window refresh is
a discrete dismiss event. Input cursor blink uses frame time in prepaint; fresh
monotonic time there belongs to edit events and scheduling, not visual sampling.

## Theme and label reads (2026-10-12)

Theme Global reads are in-memory type lookups, without a read-path lock or I/O.
Light/dark base palettes have OnceLock caches; the main window also caches its
settled palette. During the 260 ms transition, the main window and five chrome
views still compute palette interpolation when rendered and request animation
frames. ThemeColors is Copy and interpolation uses value arithmetic. This is
repeated computation, but its duration has not been profiled. Avoid equating a
palette cache hit with avoidance of the surrounding View render.

The embedded translation catalog initializes once. Plain labels use a binary
key search and an atomic locale read. Navigation items are separately cached
until language or plugin-navigation revisions change. Embedded text has static
lifetime, so I18n now uses SharedString::new_static instead of copying it into
SmolStr; this removes long-label allocation in unformatted lookup paths. Messages
with substitution arguments still construct their formatted output.
The static-text change passes seven UI-state I18n tests and two embedded-catalog
tests with `--no-default-features --locked`; targeted rustfmt and diff checks pass.

Global reads do not enter GPUI's EntityId dependency set. Theme and language
changes notify explicit observers, whose callbacks mark their Views dirty.
Cache validity therefore needs explicit theme/language revisions or equivalent
invalidation; a parent render alone does not guarantee a cached descendant rereads
its globals. The observer fan-out and continuous theme renders need a separate
reconciliation/presentation gate before claiming the theme issue solved.

## Static texture cache and independent dynamic drawing

The intended boundary is an ordered retained layer, not a texture for every
element. Static content is rasterized when its output changes. Dynamic content
has its own retained target and presentation tracks. Final composition preserves
the original paint order, including static content above a dynamic element.
A single background texture plus all dynamic elements on top would break that order.

For a dynamic content update, damage includes the old and new visual bounds.
The retained color target preserves pixels outside that region, restores the
affected static pixels, and draws intersecting content in paint order. Presentation
may still composite the complete window; that does not require rebuilding its
Views or rasterizing every static primitive. For a visual-only layer transform,
the compositor samples its existing content without rebuilding layout or text.

Cache validity must include content and resource revisions, physical scale,
size, clipping and palette. Theme changes invalidate affected colors; they cannot
be ignored as though the pixels were static. Backdrop blur is a dependency on
underlying pixels: changing a dynamic element below it invalidates the intersecting
sample footprint, including Gaussian support and linear-sampling texel padding.
Layer replacement and eviction must retain resources until submission fences retire.

The current fork has CPU subtree reuse, explicit composite layers, and regional
retained-root source updates. Color retention now also considers local motion in
scenes containing composite/filter layers, as described below. Small element
filters still allocate full-window targets. Compact filter targets and automatic
ordered static-layer partitioning are remaining work, not completed by the
dropdown migration. Avoid allocating a full-window texture for every child.
There is also an opt-in static-prefix capture in `renderer/retained_upload.rs`,
restricted to a flat, unfiltered stream with at least 96 static quad/shadow
primitives before its dynamic suffix. Its existing source-equality tests do not
establish native pixel correctness for general overlapping layers; it is not
the default general solution.

The acceptance gate for extending this design is a two-independent-elements demo:
updating one preserves the other's Render/layout/content generation, full-image
readback matches a forced rebuild (including overlap, movement and blur), and GPU
source draws/filter dispatch regions are measured separately from final composition.
Allocation bytes and target dimensions must distinguish small filters from full
background filters before claiming the resource issue solved.

## Local popup color retention (2026-10-12)

The newer supplied Vulkan log records zero retained-root presents during all
active intervals, while direct presents remain active. Three idle intervals
record no submissions. This establishes a missing final-color reuse path during
activity, rather than an idle rendering loop. It does not identify individual
dropdown events or measure GPU execution time. Blur source/target counters are
latest-frame values, not five-second totals; zero work there can coexist with a
full direct main pass. Retained packed buffers and retained filter textures do
not by themselves preserve the final static page pixels.

The previous default retained-root policy rejected every scene containing an
element filter, including the zero-radius composite used by a visual-only popup.
It also required at least 384 ordinary primitives. Consequently a small menu
could keep the static page on the direct main-pass path even without Gaussian
blur. The new shared Nova policy admits local motion when estimated static
raster work is at least twice the viewport area and animated output is at most
one quarter of that area. Existing complex unfiltered-scene eligibility remains
as a fallback. These are conservative selection heuristics, not measured GPU
time or opaque coverage. A root color target has an extra full-window allocation
and final composite cost, so cheap or broadly animated scenes remain direct.

The estimate counts captured/composited output once; it does not count already
retained capture children as repeated page raster work. Both capture-owner and
captured-child animation are recognized, including normal StartBlur/EndBlur
captures. Existing old/new presentation damage, painter-order barriers and
submission-fence ownership govern the actual preserved-source updates. No
backend-specific swapchain preservation is assumed.

Completed forwards-filled tracks retain their animation ID. Settings uses such
a track on its large content panel, so ID presence alone would misclassify the
settled panel as large active motion and still reject the small popup. The
policy now resolves completed samples (progress exactly 1) as static. Missing
samples and spring overshoot remain conservative motion candidates. The
regression covers both a completed capture owner and a completed captured child.

The native popup gate uses the actual HWND drawable size and normal capture
interfaces, including a forwards-filled settled panel covering 75 percent of
the viewport. It tests six alternating opacity samples with and without a static
backdrop filter, compares the complete image against forced full repaint, and
requires an observable animation, unchanged static foreground occlusion,
preserved root source, nonempty scissored source draws and one final main draw.
The packet supplies the sole sample for its animation ID; adding a duplicate
caller-sampled scene value would mask the packet under the existing first-wins
contract. Scissor pixels quantify the source region, not GPU utilization.
DX11, DX12, Vulkan and OpenGL all pass this gate, including both backdrop
cases. At the native 512 x 512 viewport, the largest root source draw scissor
is 400 pixels without backdrop blur and 2,304 pixels with it, compared with
262,144 viewport pixels. All six samples keep one final main draw and match
full repaint. The four existing plain-leaf splice pixel gates also pass, as do
six local-color policy tests and two existing retained-upload policy tests.
No Vulkan-only swapchain preservation or engine workaround is involved.
The required GPUI check (`windows-manifest,mimalloc-collect`, no default
features, locked) and default-feature BMCBL binary build pass after the
completion-state repair. The latter includes all four Windows backends and
produces `target/debug/BMCBL.exe` (2026-10-12 11:10:49 +08:00). Targeted rustfmt
and `git diff --check` also pass; full-workspace test/Clippy coverage remains
as bounded in the validation section below.

Repeat the native gate locally with:

```powershell
rtk cargo test -p gpui --lib --no-default-features --features test-support,nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl popup_reversal_preserves_static_color -- --ignored --test-threads=1 --nocapture
```

Real Settings-window rapid reversal, input lifecycle and GPU-busy A/B remain
separate acceptance evidence; native fixture results do not establish that the
reported 20 percent has fallen.

## Completion-time admission and source depth damage (2026-10-12)

The next supplied runtime log (`cc5047df-d4b0-441a-8b80-9d83f979e87f`)
still records no retained-root presents during Settings activity. Several latest
scene samples have 583,200 dirty pixels, equal to the 972 x 600 main window.
These observations confirm the runtime symptom remains. The log contains no
color-admission estimates, so it cannot identify the specific rejection reason.
Direct-present, blur-frame and platform-present counters have different recording
sites; adding them does not establish duplicate GPU submissions.

Color admission used to run only when the scene encode key changed. A large
panel can be rejected while moving, then retain the same scene revision after
its forwards-filled animation completes. Refreshing only the presentation values
freezes that first rejection. The native gate now first encodes a moving panel,
then encodes its completed sample without changing the scene revision. Before
the repair it fails at `same-revision panel completion must reconsider color
retention`. A rejected root now reconsiders admission once when a presented
track reaches its completed sample. Retained roots continue to reuse their encode;
this does not add a UI Render/layout/prepaint driver or a per-frame scene walk.

The gate also exercises actual direct warmup followed by retained-root promotion.
Changing the capture topology can replace the filter-target chain and retire its
resource sets. Fence draining protects submitted work, but cached CPU draw steps
also hold those handles. Target replacement now invalidates that draw-step cache.
The new root must initialize the complete image even when the packet has only
local popup damage, and the gate compares that first result against full repaint.

Preserving root color previously still cleared the entire depth attachment on
the first source group. Source submission now carries the same local clear region
for depth while keeping color Load. The Vulkan clear-area planner also recognizes
color Load plus depth Clear, with the same conservative rejection of unknown or
out-of-region draw scissors. Native gates inspect the actual pass descriptors
used by submission and require all later source groups to draw within the first
group's initialized depth region, as well as complete-image pixel equivalence.

Debug logs now report `GPUI color retention policy` with scene revision, static
raster pixels, animated output pixels, viewport pixels and eligibility. The
matching `GPUI color retention result` reports actual root selection and unsupported
content guards. These estimates explain admission; they are not GPU timings.
No frame-rate limit, blur-quality change or runtime configuration switch was added.

After these repairs, all four native popup gates pass both backdrop variants,
including direct-to-retained transition and first-target initialization. At
512 x 512 the largest source draw and actual source-pass depth-clear region
are both 400 pixels without blur and 2,304 pixels with blur. The four existing
leaf-splice pixel gates, six local-color policy tests and two root-source pass
tests also pass. Six Vulkan clear/render-area unit tests and the required GPUI
check pass. These validate submitted regions and pixels, not the reported
Task Manager utilization. The existing app CLI and lab examples do not provide
an automatic real Settings/dropdown scenario; that runtime A/B remains open.
The default-feature BMCBL build also passes (490 warnings, no errors). The
new `target/debug/BMCBL.exe` is 124,658,176 bytes, timestamped
2026-10-12 11:43:03 +08:00, with SHA-256
`665478D00D896D4AFF0392785E205BF39DBE6750CFDBC8FE68ED50E9520000B1`.
This supersedes the earlier 11:10 binary for further runtime measurements.

## Nested local damage and packet bounds (2026-10-12)

The current `target/debug/BMCBL/logs/latest.log` was copied read-only for analysis
(640,549 bytes; SHA-256
`58FC6BE1DA2EA231036B08EC4DD0650E9CBEF5CEB215343469111B7657B67FE0`).
All 1,524 main-window color-retention decisions rejected the retained root.
The three selected roots belong to a secondary window. The log's slow Render
warning names DownloadPageView and its loading placeholder, so these samples
must not be described as a confirmed Settings-only capture.

Two common framework paths enlarged local animation damage. A nested child
changed the entire enclosing capture's damage even at zero blur radius. The
capture group then widened that damage again. Zero-radius captures whose old
and new coordinate mappings are identity now propagate clipped child damage;
filters and nonidentity mappings retain conservative capture-wide damage.
The presentation packet also unconditionally added registered timeline bounds,
which can cover the viewport for translation. Scene-bound animation IDs now use
old/new sampled Scene damage for supported geometry/opacity properties. Unbound
tracks and properties without a matching bounds calculation keep the
registered-bounds fallback, including horizontal edge reshaping and filter blur.

Damage narrowing exposed a correctness prerequisite: translation must move raw
geometry before intersecting its fixed mask. Scene insertion and retained replay
also must preserve animated primitives initially outside the mask, because they
can enter it without another UI paint. Ordering reserves their mask conservatively
so static siblings keep the correct draw order as presentation samples change.

Color admission previously required two viewports of static overdraw. A logged
sample has 1,144,849 static raster pixels, 36,744 animated output pixels and a
583,200-pixel viewport: it narrowly fails that arbitrary threshold despite local
motion. Admission now compares estimated saved static raster work with the final
composition and local redraw costs. Nested identity captures account for local
child motion instead of treating the full 471,424-pixel page shell as animated.
The estimate is a policy heuristic, not a GPU timing measurement. Packed visual
samples reaching their final identity pose also reconsider a previously rejected
root once, matching ordinary transition completion. A packed sample's progress
is always one; nonidentity packed poses are conservatively treated as motion,
rather than misclassified as completed merely by that progress field.

The native fixture uses one full-window background plus a settled 75-percent
panel, rather than three artificial full-window backgrounds. It covers sibling
popups, nested popups and translation entering a fixed clip, each with and without
backdrop blur. Local rendering is compared pixel-for-pixel with full repaint,
including retained-root promotion, reversals and static foreground ordering.
The first expanded DX12 native run exposed a backend correctness defect:
offscreen texture passes set the full texture scissor once and ignored each draw
step's scissor. A local depth clear therefore accompanied draws outside the
initialized region. One failing run changed 300 static pixels during the first
translation sample, including the corner at (0, 0). The DX12 texture recorder
now applies and clamps each step's scissor, including DrawIndexed, and skips
empty intersections. Steps without a scissor use the full texture as before.
The other three backends already apply source draw scissors. After the repair,
all four native popup gates pass all six cases, including exact complete-image
readback equivalence. At a 512 x 512 viewport, maximum root source draw scissor and
root source depth-clear footprints match: 400 pixels for sibling/nested opacity and
720 for clipped translation without blur; 2,304 and 3,072 respectively with blur.
The final main pass has one retained-color composite draw. These are fixture
source footprints, not total GPU frame costs or Task Manager utilization.
The four reactive native gates, nine color-policy tests, 44 Scene CPU tests and
two packet damage tests also pass on the freshly compiled four-backend target.
The packet gate includes registered-bounds fallback for horizontal edge samples.

An additional check of actual submitted work found a remaining intermediate-layer
cost that root-only scissor inspection missed. On DX11, sibling opacity submits
400 source pixels, while nested opacity submits 197,520, with no Gaussian work.
The ordinary zero-radius parent source failed its animation-value cache comparison
and was unconditionally recaptured with a full-layer scissor. A local final-root
update therefore did not imply local work throughout the nested capture chain.
The native gate now reads the production aggregate source-pixel metric immediately
after each successful local present and rejects large parent recaptures, in
addition to checking full-image readback and attachment descriptors.

A resident zero-radius parent now preserves its source texture when its immutable
Scene Arc, layer identity, bounds, mask and radius remain unchanged and both old
and new coordinate mappings are identity. It derives source damage from consumed
GPU child samples, clears that source region to transparent with the existing
retained-clear draw, and reconstructs its painter list only within that region.
Target initialization, changed source Scenes, filters, nonidentity transforms and
forced refreshes keep the full-source path. The focused cache test verifies the
local case and rejection of new source Scenes and nonzero filters. This repair
targets the intermediate parent work rather than reducing only root metrics.
The aggregate gate now passes on all four backends. For the 512 x 512 fixture,
maximum actually submitted source footprints are:

| Placement | No backdrop blur | With backdrop blur |
| --- | ---: | ---: |
| Sibling opacity | 400 | 4,608 |
| Nested opacity | 800 | 5,008 |
| Nested translation into clip | 1,440 | 6,864 |

The nested no-blur case falls from 197,520 to 800 source pixels while preserving
exact complete-image readback equivalence. This is aggregate source work across
executed offscreen layers, including the intermediate parent; it still excludes
the final full-window composite and does not establish GPU time or utilization.
The five filter-cache tests pass, including source preservation during pending
refresh and conservative source replacement/filter handling. All four reactive
native gates and the focused color, Scene and packet tests remain passing.
The required GPUI check passes (29 existing warnings), and the default-feature
BMCBL build passes (490 existing warnings), including all four Windows backends.
The final `target/debug/BMCBL.exe` is 124,692,992 bytes, timestamped
2026-10-12 14:45:06 +08:00, with SHA-256
`E8580B08C71BCC2AC770028EE3DC29959C71CBBE478D7A679596EB8F841D18F8`.
This supersedes the earlier binaries for real-window measurements. Task Manager
GPU-busy reduction on the actual Settings dropdown remains unmeasured.

## Stable-page popup cache lifetime follow-up (2026-10-12)

The new 14:51 runtime capture still reports repeated direct presentation, not
just an old executable: the new color-policy fields are present. In its final
five-second sample, the 972 x 600 window has 107 presentations; the accompanying
process-wide counters include 668 direct presentations and 883,800 blur-source
pixels. These scopes must not be conflated, and none measures GPU busy time.
The large settled panel is sometimes classified as moving, while frames without
an active animation drop the synthetic retained window-color target altogether.

Missing animation samples execute at identity in both CPU fallback and the
indexed GPU table. The color-admission and capture-coordinate predicates now
follow that contract. Retiring an identity sample does not dirty an entire
capture; retiring a nonidentity pose still damages the old and new positions.
The source-cache comparison also uses the complete flattened GPU sample set,
including low-ID values registered in ancestor Scenes. It compares all tracks
for an owner, so a changing clip cannot hide behind unchanged opacity. Missing
samples on both sides can reuse static source pixels. Changed unsupported tracks
continue to refresh conservatively.

A previously admitted window-color target now survives stable frames and popup
removal. Compatible warmed layouts reuse their allocated slots with a separate
window-color role, rather than treating its changing flattened filter index as a
new texture. Lookups use the active prefix; spare slots remain included in byte
accounting, destruction and buffer rebinding. Growth, shared Gaussian scratch,
changed source layouts and incompatible filters retain the existing fenced
replacement path. Reused layouts rebuild draw-step mappings without globally
invalidating resident filter pixels. This saves compatible capture work; it
does not shrink every small capture texture to the element's dimensions.

The native popup fixture now includes an inactive child animation binding with
no sample. Its lifecycle gate also omits the settled parent sample and performs
three complete close/reopen cycles, checking the retained window-color texture
identity, aggregate source-pixel bounds and exact full-redraw readback. The CPU
source tests cover missing samples, nonidentity retirement, ancestor-owned low-ID
samples and opacity/clip tracks sharing an ID. Final validation and executable
identity are recorded below after the local gates complete. Actual Settings
dropdown GPU-busy utilization remains a separate runtime measurement.

## Validation status

The dropdown batch's root application check passes with `--no-default-features --locked`
and `CARGO_INCREMENTAL=0`. The dropdown reversal-duration test passes. Targeted
format checks for the dropdown and three custom-trigger files, and
`git diff --check`, pass. The touched MainWindow file has a pre-existing format
difference outside this change; it has not been reformatted wholesale.
Focused lifecycle checks cover owner-only endpoint Render, mandatory nested-cache
refresh, native retarget generation changes, rejected stale/duplicate completions,
UI-owned intermediate-frame reuse, and no restart when the same forwards target is
rendered again: three completion tests, three presentation-ownership tests, one
cache-boundary resolver test, 14 dirty-scope tests and four reactive tests pass.
The required GPUI check with `windows-manifest,mimalloc-collect` also passes.
The native pending-completion fixture now uses an actual animated
primitive in the committed scene. An engine-only track on an empty scene is
correctly pruned by a UI commit and cannot prove native ACK lifetime.
Two broader regressions remain unresolved, as recorded in `nested_dirty.md`:
generic descendant invalidation still renders the root in one cached-parent case,
and a clean next-frame callback requests the wrong frame class. This is not a
claim that the complete framework test suite passes.
The supplied real-window log predates this change. Headless Render-count tests
and native filter readback do not substitute for an updated Settings dropdown
capture with rapid reversal, outside-click dismissal, selection, scrolling,
theme changes, fractional scale, and stable static siblings. Whole-page theme
interpolation and the other rows above are not claimed as fixed.

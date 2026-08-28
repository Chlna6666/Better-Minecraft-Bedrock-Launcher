# Blur

[Chinese](backdrop_blur.zh-CN.md)

Two distinct `Styled` properties share a GPU Gaussian kernel:

| GPUI | Web CSS | Input |
| --- | --- | --- |
| `.blur(px(3.))` | `filter: blur(3px)` | Element background, shadow, border and descendants |
| `.backdrop_blur(px(3.))` | `backdrop-filter: blur(3px)` | Previously painted content behind the element |
| `.bg(rgba(0xffffffcc))` | `background: rgb(255 255 255 / 80%)` | The element's own fill |
| `.opacity(0.5)` | `opacity: .5` | Element opacity, including the filter output |

This is a typed Rust API, not a CSS parser or all CSS filter functions. The two
blur methods are not aliases. They also work after `.id(...)`; builder call order
does not change paint order.
CSS filter-list parsing, arbitrary filter composition and the full browser
Backdrop Root/stacking-context model are not implied by these two properties.

```rust
use gpui::{div, px, rgba, prelude::*};

let content = div().blur(px(3.)).child("Blurred text");
let panel = div()
    .id("account-panel")
    .backdrop_blur(px(6.))
    .bg(rgba(0xffffffeb))
    .rounded(px(16.))
    .opacity(0.8)
    .child("Sharp text");
```

Backdrop blur adds no theme color. A translucent material uses the **same
element's** background, without an extra overlay element. An opaque fill hides
the filtered backdrop. `BackdropBlurStyle` also provides quality hints,
saturation, optional tint and overlap policy. Tint alpha is independent of
element opacity: fading only tint does not fade the filter.

The length is Gaussian standard deviation (sigma) in logical pixels, following
[CSS Filter Effects](https://www.w3.org/TR/filter-effects-1/#funcdef-filter-blur).
Element/window scale converts it to device pixels. The finite GPU kernel
approximates a Gaussian through three standard deviations and preserves
fractional values; output is not guaranteed pixel-identical to browsers.
The old API measured kernel support: migrate old values `r` to `r / 3` to
preserve their visual strength.

## Scene Data

Backdrop sampling precedes the element's own shadow and fill. Its background,
descendants and border are painted afterward and remain sharp. Rounded corners
and ancestor clipping constrain the result. Element blur isolates its subtree
and composites the filtered result at the original scene position; its bounds
must include overflowing content and Gaussian support. Filtering does not
change layout or hit testing.

## GPU Pipeline

Nova's shared shader is [`blur.wgsl`](../src/platform/nova/shaders/blur.wgsl).
It uses separable X/Y passes with weights precomputed on the CPU, avoiding
per-fragment exponentials. Small kernels combine adjacent taps with hardware
bilinear filtering (at most nine samples per axis); wider kernels use 17 samples
without incorrectly merging nonadjacent texels. RGBA is filtered in premultiplied
form, then color is converted for tint/compositing. This avoids dark fringes on
transparent edges.

Intermediate GPU textures are render attachments and sampled textures, not CPU
readbacks or screenshot uploads. Offscreen passes are necessary for subtree
filtering; avoid redundant source copies. Backdrop source segments preserve
draw-order barriers, compatible regions may share filtered targets, and scissors
include kernel support. Keep radii, areas and overlapping filter counts bounded.
Prefer one filter around a group to one per row. Keep popover radius fixed,
animate opacity/transform, then unmount after exit. Hardware acceleration does
not imply zero bandwidth cost or a guaranteed frame rate.

## Comparison and verification

[Flutter BackdropFilter](https://api.flutter.dev/flutter/widgets/BackdropFilter-class.html)
and [ImageFiltered](https://api.flutter.dev/flutter/widgets/ImageFiltered-class.html)
similarly separate backdrop and child-subtree inputs.
[Qt Quick MultiEffect](https://doc.qt.io/qt-6/qml-qtquick-effects-multieffect.html)
uses an explicit source and recommends limiting effect size and blur limits.

Verify parent/child opacity, scale, fill sampling order, transparent-edge color,
subtree isolation, nesting, cached replay and the frame after unmount. Run scene,
window and Nova tests and validate WGSL on enabled backends. Descriptor tests do
not replace rendered pixel checks or interactive visual verification.

## Guidelines

- Keep blur behavior deterministic and renderer-owned.
- Do not add application theme defaults to the renderer.
- Use metrics when diagnosing blur-heavy windows.
- Prefer local `#[expect(...)]` only for intentionally retained diagnostic or
  platform compatibility code.

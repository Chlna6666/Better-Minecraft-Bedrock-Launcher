# Validation

[Chinese](validation.zh-CN.md)

Run focused validation when changing GPUI framework code, examples, docs, or the
GPUI skill.

## Rust Checks

```powershell
rtk cargo fmt --manifest-path Cargo.toml --all
rtk cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
rtk cargo clippy --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --lib -- -D warnings
rtk cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx12,nova-gfx-vulkan --examples
rtk cargo bench --manifest-path Cargo.toml --features bench-support --no-run
```

Fix warnings in GPUI scope. Use local `#[expect(..., reason = "...")]` only for
intentional compatibility or diagnostic code.

## Support Evidence

Platform support is evidence-based and recorded per release; it is not inferred
from `cfg` declarations.

| Platform | Intended backend | Required evidence |
| --- | --- | --- |
| Windows (primary) | Nova DX12; Nova Vulkan when explicitly built | feature check, library tests, benchmark compilation, example compilation, and a real-window smoke test |
| Linux / FreeBSD | Nova Vulkan | native feature check and tests on the stated display protocol |
| macOS | Nova Metal | native feature check, tests, and a real-window smoke test |

The `compile-check` workflow uses the `macos-15` arm64 runner to test the
graphics contracts and Metal backend, run the Metal atlas resource smoke, check
GPUI with only `nova-gfx-metal`, and present one automatically closed GPUI Metal
window. Cross-compilation from Windows is an additional compile gate, not a
replacement for this macOS run. Bare-metal qualification still requires a
self-hosted physical Mac and must be recorded separately.

A release record must name the target triple, OS version, enabled features,
Rust version, GPU/driver, renderer selected at runtime, commands and exit codes.
If a row was not executed for that release, label it unverified; do not report
it as passing based on another platform.

## Documentation Checks

Check that official GPUI docs use official library wording and split-language
files. Search the repository for local vendored-path wording and for same-file
bilingual section headings before finishing.

Each canonical English document should have a matching `.zh-CN.md` file in the
same directory. The GPUI skill is English-only and should not contain Chinese
text.

## Example Checks

Examples must compile with current GPUI APIs and avoid references to missing
dependencies. The Windows no-default-features example check must enable the Nova
backends so `RendererBackend::Auto` has DX12 and a Vulkan fallback. Platform-specific
examples should compile on unsupported platforms through a guarded fallback entry
point.

When updating GPU examples, verify they use current GPUI scene primitives and
available nova-gfx renderer extension points rather than removed surface APIs.

For the Windows presentation owner, run `presentation_lane_block` with both
`--backend=nova-dx12` and `--backend=nova-vulkan`. Each run must report at least
two distinct presented animation samples during its 200 ms blocked UI `Render`
interval while the UI Render count stays unchanged. A compile check or a single
backend does not satisfy this gate. Linux Wayland/X11 use the dedicated GPU
owner but share the native/UI event loop, so blocked UI work also delays native ticks.
Validate their window creation, resize, teardown and retained presentation on the native
platform. Independent progress through a blocked UI Render requires separating that event
loop first; Windows builds do not validate Linux.

# GPUI Naming Conventions

GPUI follows the project-wide
[`Rust naming conventions`](RUST_NAMING_CONVENTIONS.md). This document only adds
GPUI-specific examples and migration constraints. Development-stage migrations
replace the old API directly; they do not add aliases, deprecated wrappers, or
parallel re-exports.

## Modules

- Let the full path carry context: use `app::events`, `app::windows`,
  `img::loader`, and `list::state`; do not repeat those parents in leaf names.
- Name format modules directly when the format is the stable distinction:
  `assets::webp`, `assets::png`, and `assets::jpeg`.
- Do not mechanically ban `state`, `context`, `options`, `types`, or `util`.
  Keep an established cohesive use; split only an actual catch-all.
- Split a file when unrelated types would require a category name to coexist.
- A short leaf name is acceptable only when its full module path is unambiguous
  and the module has one reason to change.
- Private implementation paths use their context directly: prefer
  `img::loader::AssetLoader`, `animation_stream::Stream`, and
  `frame_upload::Summary`. Do not produce `img::loader::ImageAssetLoader` or
  `frame_upload::FrameUploadSummary`.
- Public root re-exports keep the object noun when their implementation module is
  private. `gpui::ImageSource`, `gpui::ImageStyle`, `gpui::Window`, and
  `gpui::RenderImage` remain meaningful root APIs; shortening them to `Source`,
  `Style`, or `State` would lose information rather than remove repetition.

## Types and fields

- Name a public type for the value it represents, not the operation that created
  it or the layer that currently owns it.
- Use conventional role suffixes when they are accurate. Qualify a type only at
  a visibility boundary where the parent path no longer supplies enough context.
- Fields remain short only inside a small, strongly typed private context.
  Cross-module fields must retain their object meaning.
- Boolean values and methods use `is_`, `has_`, `can_`, `should_`, or another
  predicate phrase.

## Functions and methods

- Put construction and conversion on the most specific result or source type.
  Prefer `EncodedImage::render(...)` over `decode_image_bytes(...)`.
- Use `new`, `from_*`, `to_*`, `as_*`, and `into_*` according to Rust API
  conventions. Do not encode a parameter list in a function name.
- Inside a format module, rely on the module path: prefer `webp::frame` over
  `decode_static_webp_frame`.
- Names describe externally observable semantics. Avoid implementation wording
  such as `decode`, `cache`, `fallback`, `strict`, or `if_needed` unless that
  behavior is itself the API contract.

## Migration gate

Before completing a rename:

1. Search implementation, tests, examples, benches, docs, features, and BMCBL
   callers with `rg`.
2. Delete unused or duplicate APIs instead of renaming them.
3. Migrate every caller to one authoritative name without compatibility paths.
4. Run formatting, focused tests, and the GPUI Windows feature check.

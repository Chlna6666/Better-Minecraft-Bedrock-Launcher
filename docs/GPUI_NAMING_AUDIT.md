# GPUI Naming Audit

This audit is the ordered migration queue for GPUI naming. It complements
[`GPUI_NAMING_CONVENTIONS.md`](GPUI_NAMING_CONVENTIONS.md): the conventions define the rule;
this file records concrete violations and the order in which they are removed.

## Phase 1: image pipeline and public API

Status: implemented in the current development migration.

- Replace operation-shaped image API with object-shaped values: `EncodedImage`,
  `ImageRenderSize`, `ImageRenderInfo`, `ImagePixelFormat`, `ImageRenderRequest`,
  `ImageStream`, and `AnimatedImageFrames`.
- Replace broad clipboard and asset names with `ClipboardImage` and `AssetLocation`.
- Replace `ImageSource` variants with `Asset`, `RenderImage`, `Clipboard`, `Encoded`,
  and `Loader`.
- Replace target-size/decode wording with sized rendering, resident memory, image
  processing, render paths, and frame prefetch terminology.
- Split the former image `source`, `loader`, `state`, `style`, `error`, and
  `target_size` files into object-specific modules.
- Delete unused or duplicate public APIs instead of preserving compatibility
  shims.

## Phase 2: generic module buckets

Status: implemented, then corrected against upstream naming practice. The
result keeps short contextual modules and only splits genuine mixed-responsibility
buckets.

1. Performance diagnostics: split snapshot models by image, frame, window,
   allocator, layout, and scene; split shared and per-window metric stores.
2. Application ownership: keep the canonical `app/state.rs` home for `App`, use
   short `events` and `windows` children, and split only unrelated platform or
   interaction responsibilities.
3. Element protocols: retain conventional `context`, `traits`, and `state`
   modules where they form one cohesive Rust object family; avoid repeating
   `image_` below `element::img`.
4. Text and input: split font, glyph, text-run, shaped-line, key-dispatch, parser,
   predicate, and key-context objects out of `types`/`context` buckets.
5. Platform and renderer: split Windows utilities, nova upload/resource types,
   and window runtime/diagnostic/cache/input state.

## Phase 3: remaining role and policy names

Status: audited; migrate only with all repository callers in the same change.

- Replace generic controller/manager/service names with the object they own.
- Replace `new_with_*` constructors with one associated construction path.
- Replace parameter-list names such as `*_at_*` and implementation-policy
  suffixes with the returned object or observable action.
- Keep `decoder` only for a concrete third-party codec object such as
  `png::Decoder` or `jpeg_decoder::Decoder`; it is not a GPUI-owned public noun.

## Completion gate

Each phase migrates implementation, tests, examples, benches, documentation,
feature-gated code, and BMCBL callers together. Old names, aliases, forwarding
wrappers, deprecated exports, and parallel module paths are prohibited.

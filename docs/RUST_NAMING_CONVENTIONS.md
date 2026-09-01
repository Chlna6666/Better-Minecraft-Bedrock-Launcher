# Rust Naming Conventions for Refactoring

This document is the project-wide naming rule for AI-assisted Rust refactors.
It applies to Rust source, modules, files, public APIs, tests, and examples. It
does not define naming for JavaScript, configuration formats, protocols, FFI,
generated code, or asset files.

## Decision order

Choose a name in this order:

1. Follow Rust standard-library and established ecosystem terminology.
2. Follow the upstream project when this repository carries or extends upstream
   code, unless the local object has materially different semantics.
3. Read the complete path, receiver, parameter types, and return type as part of
   the name.
4. Use the shortest name that remains unambiguous at its visibility boundary.
5. Add domain qualification only where a value is re-exported broadly or would
   otherwise collide with another concept.

Specificity is not measured by word count. A longer name is worse when it only
repeats context that Rust already exposes.

## Path-context rule

Judge an item by the path that its callers actually write, not by its declaration
in isolation. Remove a word when the immediately visible module, receiver, or
associated type already supplies the same distinction:

- `img::loader::AssetLoader`, not `img::loader::ImageAssetLoader`;
- `animation_stream::Stream`, not `animation_stream::AnimationStream`;
- `frame_upload::Summary`, not `frame_upload::FrameUploadSummary`;
- `caption::State`, not `caption::CaptionState`.

This rule is based on the item's visibility boundary. A type re-exported from a
private module must still make sense at its public path. For example,
`assets::source::AssetSource` may remain `gpui::AssetSource`, and
`img::source::ImageSource` may remain `gpui::ImageSource`, because callers cannot
name the private `source` module. Do not shorten a useful public API to an
ambiguous root-level `Source`, `State`, `Config`, or `Error`.

The following are not automatically redundant:

- canonical primary types such as `window::Window`, `image::Image`, and
  `io::Error`;
- a public root re-export whose private implementation path is invisible;
- two adjacent words that denote different objects rather than repeating one
  qualifier.

An AI refactor must therefore compute the effective caller path before proposing
a rename. Token similarity alone is insufficient.

## Modules and files

- Let the parent path provide context: prefer `app::events`, `app::windows`,
  `img::loader`, and `list::state` over `event_observers`, `window_registry`,
  `image_asset_loader`, and `list_runtime`.
- Prefer familiar domain nouns and cohesive responsibility names. Singular and
  plural forms are both valid when their meaning matches the contents.
- `state`, `context`, `options`, `types`, `util`, and similar names are not
  categorically forbidden. Keep them when they are the conventional name for a
  cohesive object family; reject them when they merely hide unrelated contents.
- Do not encode the entire hierarchy in a leaf filename. A file under `img/`
  normally does not need an `image_` prefix.
- A leaf file or directory must not repeat an ancestor without introducing a
  distinct object: prefer `img/{loader,source,style}.rs` over
  `img/{image_asset_loader,image_source,image_style}.rs`.
- Collapse a directory that only forwards to one same-named child. Keep a
  directory when it owns several cohesive siblings or forms a real visibility,
  platform, generated-code, or test boundary.
- Do not name modules after temporary implementation stages such as `decode`,
  `processing`, `strict`, `fallback`, or `legacy` unless that distinction is a
  stable part of the public contract.
- Split by a shared reason to change, not one type or one function per file.
  Prefer a small cohesive file over a directory of thin fragments.

## Types and traits

- Name a type for the value or capability it represents. Publicly re-exported
  types may carry domain context when callers genuinely need it, such as
  `WindowOptions` or `ImageRenderSize`. Do not repeat a crate/module domain that
  is already visible at the effective public path; in `bedrock_world::world`,
  prefer `World` and `OpenOptions` over `World` and
  `OpenOptions` unless a broader re-export would make the shorter
  name ambiguous.
- Conventional suffixes such as `State`, `Context`, `Options`, `Config`,
  `Builder`, `Error`, and `Iterator` are valid when the type actually has that
  role. Do not replace them merely to avoid a suffix.
- Avoid role words such as `Manager`, `Service`, `Engine`, `Registry`, or
  `Controller` when they do not identify concrete ownership or behavior. If the
  role is real, qualify it with the owned object and do not duplicate the parent
  module.
- Traits name a capability or contract. Do not add `Trait` or `Interface`.
- Use Rust acronym casing: `HttpClient`, `Uuid`, `Gpu`, and `parse_url`.

## Functions and methods

- Include the receiver and module path when judging a method name. Prefer
  `EncodedImage::render(size)` over a free function that repeats the input,
  operation, policy, and output in its name.
- Use standard ownership prefixes consistently: `as_` borrows, `to_` may
  allocate or compute, `into_` consumes, and `from_` constructs from a source.
- Do not serialize the signature into the name. Names shaped like
  `from_*_at_*_with_*`, `*_using_*`, or `*_if_needed` require a design review.
- Use `is_`, `has_`, `can_`, and `should_` only for boolean predicates. Ordinary
  getters omit `get_`; keyed lookup may use `get` and `get_mut`.
- Name observable behavior, not the current codec, cache, thread, fallback, or
  algorithm used internally.

## Sync and async APIs

Synchronous domain APIs are the canonical default. Do not mark ordinary
synchronous functions with `_blocking` or `_sync` merely because an async adapter
also exists:

```rust
world.chunk(pos)?;
world.player(id)?;
world.read_level_dat()?;
```

When one semantic operation intentionally has both synchronous and asynchronous
entry points, the asynchronous version may use `_async` so call sites clearly
show that it returns a `Future`:

```rust
world.chunk(pos)?;
world.chunk_async(pos).await?;
```

This convention applies across the BMCBL Rust workspace. Do not hide async
behind an otherwise meaningless `io()` or `async_world()` facade solely to avoid
an `_async` suffix.

`blocking` remains appropriate for executor/runtime operations whose stable
meaning is to schedule blocking work, such as `run_io_blocking` or
`spawn_download_blocking`. That is an execution primitive, not a synchronous
version of a domain operation.

If an async API is only a `tokio::task::spawn_blocking` adapter over a synchronous
implementation, document that fact. Do not imply that the underlying LevelDB,
filesystem, FFI, or codec became native async I/O.

## Domain terminology

Use the real domain vocabulary before generic software-layer words. Minecraft
code should prefer established Bedrock names such as `World`, `Dimension`,
`Chunk`, `SubChunk`, `BlockState`, `BlockEntity`, `Biome`, `Actor`, `Player`,
`Item`, `Structure`, `LevelDat`, and actual persisted record/key names.

Do not replace those with architecture filler such as `Repository`, `Service`,
`Manager`, `Controller`, `Helper`, or `Operations`. In `bedrock-world`, raw
Bedrock records, storage contracts, and backend adapters belong to `storage`;
the Mojang LevelDB engine itself belongs to `bedrock-leveldb`.

Player parsing, record-source classification, saved-item compatibility, and
player persistence semantics belong to the public `player` domain. `world`
should contain world lifecycle and cross-domain coordination rather than a set of
`world/player_*` helper modules.

## Options and control flow

Do not create families of functions whose names encode argument combinations,
such as `with_options`, `with_control`, `if_*`, or `using_*`, when typed options
or conditions can express the difference through one canonical operation.

Reject sentence-shaped APIs such as:

```rust
prepare_block_edits_if_primary_states_match_blocking(...)
audit_world_integrity_blocking(...)
```

Prefer an operation plus typed arguments:

```rust
prepare_block_edits(..., conditions, options)
audit(..., options)
```

## Public API documentation

Public API documentation must describe contracts, not merely restate names. For
storage, parsing, mutation, or async APIs, document the applicable source format,
read/write behavior, unknown-data preservation, transaction/atomicity boundary,
conflict protection, version-conversion behavior, async adapter behavior, and
meaningful error cases.

Reject comments such as `Get X`, `Put X`, or `Foo blocking` when they add no
contract information.

## Locals, fields, and tests

- Local names may be short when scope and type make them obvious. Increase
  precision with scope, not mechanically everywhere.
- Fields describe durable meaning, not a temporary step in an algorithm.
- Collection names are plural; counts use `_count`; booleans use affirmative
  predicates.
- Test names may be long enough to state the scenario and expected outcome.

## Refactoring gate for AI agents

Before renaming:

1. Inspect the definition, all callers, the complete module path, and upstream
   precedent. Do not rename from a token search alone.
2. Classify the problem: ambiguity, incorrect domain, duplicated context,
   implementation leakage, misleading ownership, or inconsistent Rust idiom.
3. Prefer removing words before adding words. If the proposed name is longer,
   explain what new stable distinction it communicates.
4. Check whether moving or regrouping code solves the naming problem better than
   inventing a compound name.
5. Migrate implementation, re-exports, tests, examples, benches, documentation,
   and feature-gated callers together. Development migrations keep one canonical
   API and no compatibility aliases or forwarding wrappers.
6. Run `rustfmt`, focused tests, affected feature checks, and `rg` for the old
   names. A compiling rename is not complete if stale terminology remains.
7. Record the old effective path and proposed effective path. Reject the change
   if it merely moves a repeated word between the module, file, and type.

## Review failures

Reject a proposed rename when it:

- repeats the parent module or receiver;
- replaces one vague role word with another;
- adds words that describe parameters already visible in the signature;
- turns an established Rust term into project-specific vocabulary;
- creates one-type-per-file fragmentation without a separate change reason;
- differs from upstream without a local semantic reason;
- preserves the old API beside the new one during a development migration.

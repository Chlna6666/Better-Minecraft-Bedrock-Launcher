# Examples, Lint, And Documentation

## Examples

Examples must compile with the current public API. Prefer:

```rust
Application::new().run(|cx: &mut App| {
    cx.open_window(WindowOptions::default(), |window, cx| {
        cx.new(|cx| MyView::new(window, cx))
    })?;
    cx.activate(true);
});
```

Use GPUI-exported dependencies where available, such as `gpui::http_client` for
image examples. Do not reference nonexistent crates.

Keep platform-specific examples guarded and include a small fallback `main`.

## Lint Policy

Fix clippy and rustc warnings directly. Common issues to remove:

- unnecessary unwraps;
- redundant clones;
- clone-on-copy;
- identity operations;
- manual clamp;
- hand-written defaults that can be derived.

Use local `#[expect(..., reason = "...")]` only when the code is intentionally
reserved for platform compatibility or diagnostics.

## Documentation Policy

Official GPUI docs use standalone library voice. English canonical files use
`*.md`; Chinese translations use matching `.zh-CN.md` files. GPUI skill files
are English-only.

## Validation Commands

```powershell
cargo fmt --manifest-path Cargo.toml --all
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
cargo clippy --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --lib -- -D warnings
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --examples
```

Docs and skill searches:

- Search official GPUI docs and skill files for local vendored-path wording.
- Search GPUI docs for same-file bilingual section headings.
- Search the GPUI skill directory for Han script characters.

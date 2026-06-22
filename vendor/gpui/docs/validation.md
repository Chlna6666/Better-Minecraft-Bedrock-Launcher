# Validation

[Chinese](validation.zh-CN.md)

Run focused validation when changing GPUI framework code, examples, docs, or the
GPUI skill.

## Rust Checks

```powershell
cargo fmt --manifest-path Cargo.toml --all
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect
cargo clippy --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --lib -- -D warnings
cargo check --manifest-path Cargo.toml --no-default-features --features windows-manifest,mimalloc-collect --examples
```

Fix warnings in GPUI scope. Use local `#[expect(..., reason = "...")]` only for
intentional compatibility or diagnostic code.

## Documentation Checks

Check that official GPUI docs use official library wording and split-language
files. Search the repository for local vendored-path wording and for same-file
bilingual section headings before finishing.

Each canonical English document should have a matching `.zh-CN.md` file in the
same directory. The GPUI skill is English-only and should not contain Chinese
text.

## Example Checks

Examples must compile with current GPUI APIs and avoid references to missing
dependencies. Platform-specific examples should compile on unsupported
platforms through a guarded fallback entry point.

When updating Nova GPU examples, verify the flow uses `paint_gpu_mesh_3d`,
`back_buffer_view`, `present` or `swap_buffers`, and `paint_gpu_mesh_3d`
according to where rendering occurs.

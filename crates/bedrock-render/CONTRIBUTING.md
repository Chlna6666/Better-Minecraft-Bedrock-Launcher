# Contributing

Thanks for working on `bedrock-render`. This crate is a library for rendering
Minecraft Bedrock world data, so changes should preserve deterministic output,
bounded memory use, and clear diagnostics for incomplete or mixed-version worlds.

## Local Checks

Run these before opening a PR or asking for a release review:

```powershell
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
cargo test --no-default-features
cargo doc --no-deps --all-features
cargo rustdoc --lib --all-features -- -D missing_docs
cargo bench --bench render --all-features
```

`cargo bench` is expected to run the quick Criterion suite. Full web-map export
benchmarks are opt-in so normal review does not spend several minutes repeating
large region exports.

## Fixture Policy

Do not commit real Bedrock worlds, `.mcworld` exports, generated tile folders,
or Criterion output. Real worlds are large and can contain player data.

When a regression needs coverage, prefer one of these approaches:

- Add a small in-memory storage test.
- Reuse the optional `bedrock-world` sample fixture when it exists locally.
- Document a manual large-world reproduction step in `docs/TESTING.md`.

## API Policy

- Public fallible APIs return `bedrock_render::Result<T>`.
- Match `BedrockRenderError::kind()` for stable error categories.
- Absence in normal world data should be represented as transparent output,
  diagnostics, or `Option`, not as a panic.
- Keep long-running operations cancellable and keep callback payloads cheap to
  clone or aggregate.

## Benchmark Policy

Keep default benchmarks focused on repeatable microbenchmarks. Any benchmark that
exports many web-map tiles, depends heavily on disk throughput, or regularly runs
for more than a few seconds per sample should be gated behind an explicit opt-in
environment variable and documented in `docs/BENCHMARKS.md`.

## Dependency Policy

`bedrock-render` depends on `bedrock-world` by pinned Git revision until the
crate family is published. Before publishing to crates.io, publish
`bedrock-world`, replace the Git dependency with a versioned crates.io
dependency, and remove `publish = false` from `Cargo.toml`.

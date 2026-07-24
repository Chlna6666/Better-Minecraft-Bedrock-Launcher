# GPUI Upstream Tracking

This crate is an independently maintained fork of GPUI from the Zed
repository.

## Baseline

- Upstream repository: <https://github.com/zed-industries/zed>
- Upstream path: `crates/gpui`
- Baseline revision: `69e2130295c2649963eb639fc70b4f2ee8ea1624`
- Baseline package version: `0.2.2`
- Upstream license: Apache-2.0

The baseline revision is the Zed commit that published GPUI 0.2.2. Local
renderer, platform, API, example, documentation, and workspace-integration
changes are maintained by the BMCBL/egpui project.

## Synchronization

1. Fetch the Zed repository without changing this repository's default remote.
2. Review upstream GPUI changes from the recorded baseline to the candidate
   revision.
3. Classify conflicts as upstream compatibility, Nova renderer integration,
   platform behavior, local API extension, or documentation.
4. Preserve upstream copyright, license headers, and attribution when applying
   changes.
5. Update this file and `[package.metadata.upstream]` only after the candidate
   revision passes the GPUI package and example checks.
6. Record intentionally skipped upstream changes and the reason in the
   integration commit or pull request.

Required validation:

```text
cargo metadata --manifest-path crates/gpui/Cargo.toml --no-deps
cargo check -p gpui
cargo check -p gpui --examples
cargo package --manifest-path crates/gpui/Cargo.toml --allow-dirty --list
```

Do not replace this crate wholesale with a generated registry package. The
hand-maintained `Cargo.toml`, `NOTICE`, this file, and local changes must remain
reviewable.

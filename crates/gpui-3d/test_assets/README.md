# gpui-3d test assets

Verified, licensed glTF 2.0 test models used by the offscreen renderer examples, GPU pixel
tests, and import regression tests.

- `manifest.toml` lists every asset with its source URL, SHA-256, byte size, license, and
  feature tags.
- `download.ps1` downloads missing assets into `cache/`, verifies every SHA-256, and fails
  on any error. Re-running it without network access succeeds once the cache is complete.
- `licenses/README.txt` records the required attribution and license links.
- `cache/` is not committed; it is the only directory that needs to be present for offline runs.

First-time setup:

```powershell
pwsh -NoProfile -File crates/gpui-3d/test_assets/download.ps1
```

Offline regression runs only read the verified cache. Tests and examples treat a missing cache
file as an error, never as a skipped pass.

# BMCBL Documentation

This directory is the canonical entry point for BMCBL and workspace crate documentation.
Chinese is the primary language for project documentation; developer-facing core guides have
English mirrors. Crate `README.md` files are English with a Chinese `README.zh-CN.md`.

## Document map

| Document | Contents |
| --- | --- |
| [ARCHITECTURE](ARCHITECTURE.en.md) | Layering, crate boundaries, rendering stack, runtime and plugins |
| [BUILD_AND_TEST](BUILD_AND_TEST.en.md) | Toolchain, commands, feature/backend matrix, test tiers, troubleshooting |
| [CONTRIBUTING](CONTRIBUTING.en.md) | Change workflow, boundaries, naming, tests, docs, commits |
| [CRATES](CRATES.en.md) | Workspace crate catalog: role, status, boundaries, maintenance |
| [CLI](CLI.en.md) | Terminal subcommands, output format and exit codes |
| [STARTUP_ARGUMENTS](STARTUP_ARGUMENTS.en.md) | Startup arguments for shortcuts, file associations and the updater, plus legacy aliases |
| [NAMING](NAMING.md) | Naming rules (Chinese; wgpu-aligned vocabulary) |
| [ADR 0001](adr/0001-nova-gfx-gpui-boundary.md) | gpui / nova-gfx boundary decision (Chinese) |

Chinese originals: [README.md](README.md) · [ARCHITECTURE.md](ARCHITECTURE.md) ·
[BUILD_AND_TEST.md](BUILD_AND_TEST.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [CRATES.md](CRATES.md) ·
[CLI.md](CLI.md) ·
[STARTUP_ARGUMENTS.md](STARTUP_ARGUMENTS.md)

## Reading paths

- **New to the project**: root [README](../README.md) → this page → [ARCHITECTURE](ARCHITECTURE.en.md).
- **GPU/rendering changes**: [ADR 0001](adr/0001-nova-gfx-gpui-boundary.md), [NAMING](NAMING.md),
  then the backend matrix in [BUILD_AND_TEST](BUILD_AND_TEST.en.md).
- **Application features**: feature tables in the root README → the matching `src/` module →
  neighbouring crates in [CRATES](CRATES.en.md).
- **AI / automation agents**: start with `AGENTS.md` at the repository root.

## Documentation rules

1. **Code is the source of truth.** Commands, features, versions and paths must match the
   current code and manifests; unverifiable performance numbers must not be added.
2. **No transient artefacts.** Test output, raw benchmark data, screenshots and logs do not
   belong in `docs/`; evidence belongs in tests or CI artefacts.
3. **No rename history.** Migration maps and old-name inventories are not long-term docs;
   describe them in the commit or pull request instead.
4. **No dated plans or drafts.** Roadmaps live in issues/PRs; `docs/` keeps stable knowledge.
5. **Boundaries first.** A cross-crate responsibility change must update
   [ARCHITECTURE](ARCHITECTURE.en.md) or add an ADR before code changes.
6. **Keep languages in sync.** When editing a bilingual pair, update both sides.

## Documentation checks

```powershell
pwsh -File scripts/check-docs.ps1
pwsh -File scripts/check-nova-gfx-boundary.ps1
pwsh -File scripts/check-naming.ps1
```

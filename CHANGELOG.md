# Changelog

All notable changes to Better Minecraft Bedrock Launcher are documented here.
The `Unreleased` section describes the current development line. Commit summaries
inside that section are maintained automatically by
`scripts/generate_changelog.ps1`.

## [Unreleased]

### Highlights

- Added a version-management flow for custom icons, thumbnails, version settings,
  and generated Minecraft entity icon assets.
- Reworked the map viewer around visible-tile demand loading, render sessions,
  viewport generations, query budgets, metadata caching, tile planning, 2D/3D
  previews, editing history, structure import/export, and selection workflows.
- Improved launcher workflows for local version discovery, AppX/GDK prerequisites,
  task scheduling, archive handling, music playback ordering, and settings state.
- Moved management-page business reads and parsing into hidden Tokio services,
  while invalidating route-scoped requests when views are released or recreated,
  so completed version/resource scans cannot leave the UI stuck loading or flood
  the task page. Tokio async and blocking capacity now scale to twice the
  available logical CPU count, and capacity is released only when the underlying
  blocking operation actually exits.
- Expanded GPUI/Nova rendering support for frame lifecycle, image painting,
  upload/resource handling, DX12/Vulkan paths, native text backends, font catalogs,
  and small-text rasterization.
- Added localization and documentation updates covering architecture boundaries,
  rendering internals, project structure, UI conventions, and version icon behavior.

### Build And Release

- Bumped the application package version to `0.2.0` and enabled WebP decoding for
  image assets.
- Updated GitHub Actions to checkout the public `BE-Community-Dev` crates in the
  sibling layout required by the existing Cargo path dependencies, without tokens.
- Removed bundled font files from the repository and kept local agent/planning
  artifacts ignored.

### Maintenance

- Added a repository-local commit message hook and Chinese Conventional Commits
  documentation for consistent contribution history.

<!-- changelog:generated:start -->
### Commit Summary

Automatically generated from `v0.1.3` through `HEAD`.

### Added
- enable p2p by default (`09d0469`, 2026-07-18)
- ship launcher rendering and release automation (`51aaeb3`, 2026-07-18)
- optimize rendering and launcher workflows (`da8442c`, 2026-07-13)
- add custom skin previews and cover cache (`45489f8`, 2026-07-08)
- update GPUI nova renderer stack (`fd1ffc5`, 2026-07-05)

### Fixed
- unify async tasks and loading state (`37277ff`, 2026-07-18)
- 修复圆角裁切字体显示与后端标识 (`c9c030c`, 2026-07-18)
- keep GPUI inspector query available in release (`8313be9`, 2026-07-18)
- improve nova resize and skin previews (`b765e6d`, 2026-07-06)

### Changed
- 统一联机与管理页面布局 (`3ec012e`, 2026-07-19)

### Documentation
- specify version custom icon behavior (`264c06d`, 2026-07-13)

### Maintenance
- 使用 Cocogitto 统一提交校验 (`a78facd`, 2026-07-18)
- harden nightly and release publishing (`a9a4b21`, 2026-07-18)
- use git sources for Bedrock dependencies (`9da2dea`, 2026-07-18)
- clone public dependencies without checkout tokens (`81b09aa`, 2026-07-18)
- use anonymous Node 24 dependency checkouts (`0015513`, 2026-07-18)
- remove bundled skills directory (`fb0e566`, 2026-06-22)
- ignore playwright mcp artifacts (`7a18c26`, 2026-06-22)
- remove gpui-ce reference submodule (`9e26a81`, 2026-06-22)
- establish GPUI workspace baseline (`e5c4d23`, 2026-06-22)

<!-- changelog:generated:end -->

## Release Notes

- Stable tags matching `v<major>.<minor>.<patch>` trigger the Windows release
  workflow.
- Nightly tags are created weekly when the default branch has new commits and
  are published as prereleases.
- GitHub Actions generates release notes from commits since the previous stable
  tag and uploads the Windows x86_64 executable to GitHub Releases.

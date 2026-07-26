# Changelog

All notable changes to Better Minecraft Bedrock Launcher are documented here.
The `Unreleased` section is a short, manually curated preview of the next stable
release. Stable release sections are archived automatically when a stable tag is
published. Nightly release notes are generated only on GitHub Releases and are
not copied into this file.

## [Unreleased]

### Highlights

- Rewrote the bundled NetherNet transport (`crates/bedrock-nethernet`) as a
  layered, zero-copy implementation with its wire format verified byte-for-byte
  against go-nethernet, including its own test vectors. Fixed two hard 64 KiB
  ceilings in webrtc-rs that silently broke every game packet above that size:
  the SCTP send limit is now raised to 256 KiB, and the data channels are
  detached so the read loop uses a buffer sized for NetherNet's 256 KiB
  segments rather than the built-in 65535-byte one. Also added the full
  `CONNECTERROR` code table, a `Signaling` abstraction, discovery entry
  expiry, per-source rate limiting, and session statistics. The public API is
  unchanged.

- Replaced the vendored bedrock-crustaceans RakNet crates with an in-house
  implementation (`crates/raknet`): go-raknet-style connection model on a
  zero-copy (`bytes::Bytes`) reliability engine with proper reliable-frame
  deduplication, RFC 6298 retransmission timing, AIMD congestion control,
  u24 sequence-wrap safety, and hardening limits for split reassembly and
  ordering buffers. The tokio driver keeps the previous `raknet_tokio::prelude`
  API (no caller changes) while removing the per-send actor round-trips;
  loopback throughput improved from 5.6 MiB/s to 78 MiB/s in the bundled
  benchmark. go-raknet host interop (cookie echo, negative client GUID) is
  preserved.

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

## Release Notes

- Stable tags matching `v<major>.<minor>.<patch>` trigger the Windows release
  workflow.
- Nightly tags are created weekly when the default branch has new commits and
  are published as prereleases.
- GitHub Actions generates release notes from commits since the previous stable
  tag and uploads the Windows x86_64 executable to GitHub Releases.

# Upstream provenance

- Repository: https://github.com/BE-Community-Dev/bedrock-block-model
- Imported revision: `9f8c4436bbc26617551a5003cc2762b997e76fff`
- License: MIT OR Apache-2.0

This directory is a normal BMCBL workspace crate, not a nested Git checkout.
BMCBL intentionally uses it through a local path dependency so the block-model
resolver and the current `bedrock-world::BlockState` API change together.

When updating from upstream, import a reviewed revision, remove nested VCS
metadata, preserve local workspace integration, update the revision above, and
run the crate tests plus the `bedrock-render` and BMCBL compile checks. Do not
introduce a runtime network fallback or an old/new API compatibility layer.

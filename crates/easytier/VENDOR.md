# EasyTier Core Vendor

This directory vendors the `easytier` Rust crate used by BMCBL.

- Upstream base: `EasyTier/EasyTier` commit `346f32d3d0d38b6cd9a877a15b379ce466bc6c0d`
- Downstream source: `Chlna6666/EasyTier`
- Retained changes: network-interface enumeration, ACL application and payload
  fingerprinting, and the embedded game profile
- Omitted: EasyTier GUI, Web, contrib, Tauri, CLI binaries, service installation,
  FakeTCP, WinDivert, CI, release tooling, and Windows TUN runtime assets

BMCBL enables only the library features needed by its no-TUN PaperConnect transport.

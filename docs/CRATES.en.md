# Workspace crate catalog

This page lists every workspace crate with its role, status and documentation entry. When a crate
is added or removed, update this table and `[workspace].members` in the root `Cargo.toml`.

## 1. Application and framework

| Crate | Path | Responsibility | Status | Docs |
| --- | --- | --- | --- | --- |
| `bmcbl` | `/` | Launcher binary/library: UI, core domain, downloads, plugin host | Main application | root [README](../README.md) |
| `egpui` | `crates/egpui` | Desktop host, runtime, task scopes, UI bridges, i18n, durable tasks | Active | [README](../crates/egpui/README.md) |
| `egpui-build` | `crates/egpui-build` | `App.toml`, resource index and platform bundle plans | Active | [README](../crates/egpui-build/README.md) |
| `egpui-manifest` | `crates/egpui-manifest` | Application manifest schema and validation | Active | [README](../crates/egpui-manifest/README.md) |

## 2. UI core

| Crate | Path | Responsibility | Status | Docs |
| --- | --- | --- | --- | --- |
| `gpui` | `crates/gpui` | Windows, entities, elements, layout, input, text, scene, presentation | Active, pre-1.0 | [README](../crates/gpui/README.md) |
| `gpui-3d` | `crates/gpui-3d` | 3D scenes, cameras, materials, animation, queries, glTF, offscreen and scene views | Active, pre-1.0 | [README](../crates/gpui-3d/README.md) |
| `gpui_tokio` | `crates/gpui_tokio` | GPUI/Tokio task integration | Active | [README](../crates/gpui_tokio/README.md) |
| `gpui-hooks` | `crates/gpui-hooks` | GPUI hooks and state reuse | Active | [README](../crates/gpui-hooks/README.md) |
| `gpui-hooks-macros` | `crates/gpui-hooks-macros` | Hook macros | Internal | [README](../crates/gpui-hooks-macros/README.md) |
| `gpui-macros` | `crates/gpui-macros` | GPUI derive-macro mirror | Internal | [README](../crates/gpui-macros/README.md) |
| `lucide-gpui` | `crates/lucide-gpui` | Lucide icon assets for GPUI | Active | [README](../crates/lucide-gpui/README.md) |

## 3. Graphics (nova-gfx)

| Crate | Path | Responsibility | Status | Docs |
| --- | --- | --- | --- | --- |
| `nova-gfx` | `crates/nova-gfx` | Graphics umbrella and documentation | Active | [README](../crates/nova-gfx/README.md) |
| `gfx-core` | `crates/nova-gfx/gfx-core` | Backend-neutral descriptors, handles and capability traits | Active, breaking evolution | [README](../crates/nova-gfx/gfx-core/README.md) |
| `gfx-memory` | `crates/nova-gfx/gfx-memory` | GPU allocation wrappers, upload ring, deferred free, statistics | Active | [README](../crates/nova-gfx/gfx-memory/README.md) |
| `gfx-shader` | `crates/nova-gfx/gfx-shader` | WGSL validation and SPIR-V/HLSL/MSL/GLSL generation | Active | [README](../crates/nova-gfx/gfx-shader/README.md) |
| `gfx-shader-build` | `crates/nova-gfx/gfx-shader-build` | Build-time shader compilation and packaging | Active | [README](../crates/nova-gfx/gfx-shader-build/README.md) |
| `gfx-dx11` | `crates/nova-gfx/gfx-dx11` | Windows D3D11 backend | Active | [README](../crates/nova-gfx/gfx-dx11/README.md) |
| `gfx-dx12` | `crates/nova-gfx/gfx-dx12` | Windows D3D12 backend | Active | [README](../crates/nova-gfx/gfx-dx12/README.md) |
| `gfx-vulkan` | `crates/nova-gfx/gfx-vulkan` | Vulkan backend | Active | [README](../crates/nova-gfx/gfx-vulkan/README.md) |
| `gfx-opengl` | `crates/nova-gfx/gfx-opengl` | OpenGL 4.5 backend (WGL/EGL) | Active | [README](../crates/nova-gfx/gfx-opengl/README.md) |
| `gfx-metal` | `crates/nova-gfx/gfx-metal` | Apple Metal backend | Partial (offscreen unimplemented) | [README](../crates/nova-gfx/gfx-metal/README.md) |
| `gfx-webgl` | `crates/nova-gfx/gfx-webgl` | WebGL backend | Stub | [README](../crates/nova-gfx/gfx-webgl/README.md) |

`examples/triangle*`, `examples/atlas-smoke*` and `examples/public-api-smoke` are build smoke
examples, not library APIs.

## 4. Bedrock domain and storage

| Crate | Path | Responsibility | Status | Docs |
| --- | --- | --- | --- | --- |
| `bedrock-world` | `crates/bedrock-world` | World/level.dat/NBT/player/chunk I/O and version semantics | Active | [README](../crates/bedrock-world/README.md) |
| `bedrock-leveldb` | `crates/bedrock-leveldb` | Modified LevelDB storage engine | Active | [README](../crates/bedrock-leveldb/README.md) |
| `bedrock-render` | `crates/bedrock-render` | CPU world tile renderer | Active | [README](../crates/bedrock-render/README.md) |
| `bedrock-block-model` | `crates/bedrock-block-model` | Resource-pack block model resolution | Active | [README](../crates/bedrock-block-model/README.md) |
| `bedrock-brarchive` | `crates/bedrock-brarchive` | Read-only BRArchive v1 access | Active | [README](../crates/bedrock-brarchive/README.md) |
| `bedrock-voxel` | `crates/bedrock-voxel` | Image/OBJ to block placement plans | Active | [README](../crates/bedrock-voxel/README.md) |

## 5. Plugins and networking

| Crate | Path | Responsibility | Status | Docs |
| --- | --- | --- | --- | --- |
| `bmcbl_plugin_api` | `crates/bmcbl-plugin-api` | Plugin ABI, macros, `.bmcblx` packing | Active (API `0.6`) | [README](../crates/bmcbl-plugin-api/README.md) |
| `bmcbl_plugin_macros` | `crates/bmcbl-plugin-macros` | Plugin attribute macros | Internal | [README](../crates/bmcbl-plugin-macros/README.md) |
| `nethernet` | `crates/nethernet` | Bedrock NetherNet (WebRTC) transport | Active | [README](../crates/nethernet/README.md) |
| `easytier-bmcbl` | `crates/easytier-bmcbl` | EasyTier wrapper (threads/instance management) | Active | [README](../crates/easytier-bmcbl/README.md) |

## 6. External and vendored code

The following directories are **not** workspace members and follow upstream practice; the
repository documentation rules do not apply to them:

| Path | Notes |
| --- | --- |
| `crates/easytier` | EasyTier upstream sources (excluded) |
| `crates/raknet` | RakNet upstream/port sources (excluded) |
| `vendor/*` | Patched dependencies such as `dtls`, `webrtc`, `webrtc-ice`, `gpu-allocator`, `gpui-router` |

When modifying vendored code, document the upstream version, the patch reason and the upgrade
strategy in the pull request.

## 7. Boundary quick reference

- `nova-gfx`/`gfx-*` never depend on `gpui`/`egpui`/application code; `gfx-core` has no window
  handle dependency.
- `gpui` does not depend on the application; `gpui-3d` depends on `gpui` + `gfx-core` and owns no
  window or backend.
- `egpui` owns no product semantics such as downloads, maps or routes.
- `bedrock-*` does not depend on UI/GPU presentation; `bedrock-voxel` only produces plans and does
  not write worlds.
- A plugin ABI change must update the API version, manifest schema and example plugins together.

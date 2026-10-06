# gfx-shader-build

Build-time WGSL compilation and embedding for
[`nova-gfx`](../README.md) backends. Add it as a **build dependency** and call it
from `build.rs`; the crate translates every declared entry point for the backends
the build enables, compiles DX12 shaders to Direct3D bytecode when the build host
can run the Direct3D compiler, and writes a generated table the crate embeds. WGSL is
parsed/validated once per declared bundle and reused across all entry points and
backend translations. The build helper calls FXC directly instead of depending on
the full `gfx-dx12` runtime backend.

The point is that no shader compiler runs while an application starts: the
renderer receives ready-to-use `gfx_core::EmbeddedShader` values.

## Usage

```rust
// build.rs
use gfx_shader_build::{Dx12ArtifactPolicy, Shader, ShaderSet, ShaderStage};

fn main() {
    let shaders = ShaderSet::new("viewer")
    .dx12_artifact_policy(Dx12ArtifactPolicy::RequireBytecode)
    .shader(
        Shader::new("scene_view")
            .wgsl_file("src/scene_view.wgsl")
            .entry("vs_main", ShaderStage::Vertex)
            .entry("fs_main", ShaderStage::Fragment),
    );

    if let Err(error) = shaders.emit() {
        println!("cargo::error={error}");
        std::process::exit(1);
    }
}
```

```rust
// src/shader_table.rs
include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
```

The calling crate must depend on `gfx-core`, because the generated table names
`gfx_core::EmbeddedShader`. Entry point names must be unique inside one

`ShaderSet`; the build fails when they are not.

## Backend selection

Artifacts are emitted only for backends that are both requested through the
calling crate's Cargo features and valid for the target platform:

| Feature (uppercased by Cargo) | Target platforms | Artifact |
| --- | --- | --- |
| `NOVA_GFX_DX12` | windows | DXBC, or HLSL when the build host is not Windows |
| `NOVA_GFX_VULKAN` | windows, linux, freebsd | SPIR-V words |
| `NOVA_GFX_METAL` | macos | MSL source |

Direct3D bytecode is generated only when the build host runs Windows, because
`D3DCompile` ships with Windows. `Dx12ArtifactPolicy::RequireBytecode` is the default. A shader tool that deliberately
accepts runtime compilation must explicitly select `AllowRuntimeCompilation`, and its
DX12 runtime must enable `gfx-dx12/shader-compiler`. Under the strict default policy a Windows target built on a host that cannot run `D3DCompile` fails at
build time instead of silently moving shader compilation back into application
startup. BMCBL GPUI and gpui-3d shader sets use this strict policy.

## License

GPL-3.0.

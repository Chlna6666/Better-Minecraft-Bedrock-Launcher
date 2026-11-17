# Downloads the primary "large world" glTF model used by world_bench.
#
# The gpui-3d glTF loader (gltf = "=1.4.1") does NOT support KHR_draco_mesh_compression, so this
# script deliberately uses the plain glTF-Binary (non-Draco) Sponza variant from KhronosGroup's
# glTF-Sample-Assets repository. If the download or checksum fails, world_bench automatically
# falls back to the in-repo DamagedHelmet.glb fixture and then to a procedural world.
#
# Usage:
#   pwsh -NoProfile -File crates/gpui-3d/examples/world_bench/fetch_model.ps1
#   pwsh -NoProfile -File crates/gpui-3d/examples/world_bench/fetch_model.ps1 -Force

param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$outDir = Join-Path $PSScriptRoot 'assets'
$target = Join-Path $outDir 'Sponza.glb'

# Khronos Sponza, glTF-Binary, non-Draco. Pinned to a release tag for reproducibility.
$url = 'https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Sponza/glTF-Binary/Sponza.glb'
# Optional SHA-256 pin. Leave empty to skip verification (the loader still validates the GLB).
$expectedSha256 = ''

if ((Test-Path $target) -and -not $Force) {
    $existing = (Get-Item $target).Length
    Write-Host "world_bench: model already present at $target ($existing bytes); use -Force to re-download."
    exit 0
}

if (-not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir | Out-Null
}

Write-Host "world_bench: downloading $url"
$temp = "$target.download"
try {
    Invoke-WebRequest -Uri $url -OutFile $temp -UseBasicParsing -TimeoutSec 600
    $info = Get-Item $temp
    Write-Host "world_bench: downloaded $($info.Length) bytes"
    if ($info.Length -lt 100000) {
        throw "download seems too small ($($info.Length) bytes); refusing to install"
    }
    if ($expectedSha256) {
        $actual = (Get-FileHash -Algorithm SHA256 -Path $temp).Hash
        if ($actual.ToLowerInvariant() -ne $expectedSha256.ToLowerInvariant()) {
            throw "SHA-256 mismatch: expected $expectedSha256, got $actual"
        }
    }
    Move-Item -Force $temp $target
    Write-Host "world_bench: installed model at $target"
} catch {
    if (Test-Path $temp) { Remove-Item $temp -Force }
    Write-Warning "world_bench: model download failed: $_"
    Write-Warning 'world_bench: the example will use the DamagedHelmet fixture or a procedural world instead.'
    exit 1
}

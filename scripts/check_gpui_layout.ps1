$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$gpuiRoot = Join-Path $workspaceRoot "crates\gpui"
$gpuiManifest = Join-Path $gpuiRoot "Cargo.toml"
$legacyGpuiRoot = Join-Path $workspaceRoot "vendor\gpui"
$legacyManifest = Join-Path $gpuiRoot "Cargo.toml.orig"

if (-not (Test-Path -LiteralPath $gpuiManifest -PathType Leaf)) {
    throw "GPUI manifest is missing: $gpuiManifest"
}

if (Test-Path -LiteralPath $legacyGpuiRoot) {
    throw "The legacy vendor/gpui directory must not be restored: $legacyGpuiRoot"
}

if (Test-Path -LiteralPath $legacyManifest) {
    throw "The source tree must not contain Cargo.toml.orig: $legacyManifest"
}

Write-Host "GPUI workspace layout is valid."

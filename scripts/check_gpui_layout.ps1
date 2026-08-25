$ErrorActionPreference = "Stop"

$workspaceRoot = Split-Path -Parent $PSScriptRoot
$gpuiRoot = Join-Path $workspaceRoot "crates\gpui"
$gpuiSourceRoot = Join-Path $gpuiRoot "src"
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

$repeatedPaths = [System.Collections.Generic.List[string]]::new()

Get-ChildItem -LiteralPath $gpuiSourceRoot -Recurse -Directory | ForEach-Object {
    $relativePath = $_.FullName.Substring($gpuiSourceRoot.Length + 1)
    $segments = $relativePath -split '[\\/]'
    if ($segments.Length -lt 2) {
        return
    }

    $parent = $segments[-2]
    $child = $segments[-1]
    if ($child -eq $parent -or $child.StartsWith("${parent}_", [StringComparison]::Ordinal)) {
        $repeatedPaths.Add($relativePath)
    }
}

Get-ChildItem -LiteralPath $gpuiSourceRoot -Recurse -File -Filter "*.rs" | ForEach-Object {
    $relativePath = $_.FullName.Substring($gpuiSourceRoot.Length + 1)
    $segments = $relativePath -split '[\\/]'
    if ($segments.Length -lt 2) {
        return
    }

    $parent = $segments[-2]
    $stem = [IO.Path]::GetFileNameWithoutExtension($segments[-1])
    if ($stem -eq $parent -or $stem.StartsWith("${parent}_", [StringComparison]::Ordinal)) {
        $repeatedPaths.Add($relativePath)
    }
}

if ($repeatedPaths.Count -gt 0) {
    $details = ($repeatedPaths | Sort-Object -Unique) -join [Environment]::NewLine
    throw "GPUI paths must not repeat their parent segment:$([Environment]::NewLine)$details"
}

Write-Host "GPUI workspace layout is valid."

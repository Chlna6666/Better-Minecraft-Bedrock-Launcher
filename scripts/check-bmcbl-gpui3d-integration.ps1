# BMCBL owns a GPUI window and requires the native gpui-3d SceneView sidecar.
# The gpui-3d library may be compiled without GPUI by standalone games, but
# never implicitly disable SceneView in the BMCBL map/skin editors.
# Run from repository root: pwsh -File scripts/check-bmcbl-gpui3d-integration.ps1
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$failures = [System.Collections.Generic.List[string]]::new()

$manifest = Get-Content -LiteralPath (Join-Path $repo "Cargo.toml") -Raw
$crateManifest = Get-Content -LiteralPath (Join-Path $repo "crates/gpui-3d/Cargo.toml") -Raw
$crateLibrary = Get-Content -LiteralPath (Join-Path $repo "crates/gpui-3d/src/lib.rs") -Raw

# The application must request the integration explicitly, independent of the
# standalone library's default feature policy.
if ($manifest -notmatch '(?m)^gpui-3d\s*=\s*\{[^\r\n]*features\s*=\s*\[[^\r\n]*"gpui-sidecar"') {
    $failures.Add("BMCBL must explicitly enable the gpui-3d/gpui-sidecar feature")
}
if ($crateManifest -notmatch '(?m)^gpui-sidecar\s*=\s*\["dep:gpui"\]') {
    $failures.Add("gpui-3d must expose gpui-sidecar and its optional GPUI dependency")
}
if ($crateLibrary -notmatch '(?s)#\[cfg\(feature\s*=\s*"gpui-sidecar"\)\]\s*pub use scene_view::') {
    $failures.Add("gpui-3d must export SceneView and scene_view() with gpui-sidecar")
}

# Smoke-check all production BMCBL callers. The map editor, embedded preview,
# detached preview and skin preview must still be connected to real SceneView.
$viewCallers = @(
    "src/ui/window/map_viewer/world3d/editor.rs",
    "src/ui/window/map_viewer/preview_panel_render.rs",
    "src/ui/window/map_viewer/preview_detached.rs",
    "src/ui/window/skin_pack/preview.rs"
)
foreach ($relative in $viewCallers) {
    $path = Join-Path $repo $relative
    if (-not (Test-Path $path -PathType Leaf)) {
        $failures.Add("BMCBL 3D view caller missing: $relative")
        continue
    }
    if (-not (Select-String -LiteralPath $path -SimpleMatch -Pattern 'gpui_3d::scene_view(' -Quiet)) {
        $failures.Add("BMCBL 3D view no longer consumes GPUI Sidecar: $relative")
    }
}

# The 3D draw path must remain native and offscreen. UI composite consumes the
# finished TextureViewId and must not reconstruct mesh draw commands.
$renderer = Join-Path $repo "crates/gpui-3d/src/scene_view/renderer.rs"
if (-not (Select-String -LiteralPath $renderer -SimpleMatch -Pattern 'submit_native_offscreen_passes(' -Quiet)) {
    $failures.Add("gpui-3d must still submit native 3D passes outside GPUI's 2D command encoder")
}
$sidecar = Join-Path $repo "crates/gpui-3d/src/scene_view/sidecar_surface.rs"
if (-not (Select-String -LiteralPath $sidecar -SimpleMatch -Pattern 'TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED' -Quiet)) {
    $failures.Add("SceneView sidecar must retain the native GPU render-target / sampled view bridge")
}

if ($failures.Count -gt 0) {
    Write-Host "BMCBL gpui-3d integration check failed:" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" }
    exit 1
}
Write-Host "BMCBL gpui-3d editor/sidecar integration source checks passed." -ForegroundColor Green
Write-Host "Source-only validation: compile, GPU barriers and hardware rendering require separate tests."

# Guards the engine / optional UI adapter split in gpui-3d.
# Run from repository root: pwsh -File scripts/check-gpui-3d-boundary.ps1
$ErrorActionPreference = "Stop"
$failures = [System.Collections.Generic.List[string]]::new()
$crateRoot = Join-Path $PSScriptRoot "..\crates\gpui-3d"
$sourceRoot = Join-Path $crateRoot "src"

# Pure engine-owned modules. Adding a GPUI/egpui/application import to any
# entry in this list is a hard error, even while the crate still contains the
# optional scene_view adapter as an unconditional module.
$pureEngineFiles = @(
    "native_offscreen.rs",
    "scene.rs",
    "mesh.rs",
    "material.rs",
    "camera.rs",
    "light.rs",
    "prepare.rs",
    "render/mod.rs"
)
$forbidden = '(?i)\b(?:use|pub\s+use)\s+(?:gpui|egpui|bmcbl)\b|\b(?:gpui|egpui|bmcbl)\s*::'
foreach ($relative in $pureEngineFiles) {
    $path = Join-Path $sourceRoot $relative
    if (-not (Test-Path $path -PathType Leaf)) {
        $failures.Add("missing pure 3D engine module: $path")
        continue
    }
    $lineNumber = 0
    foreach ($line in Get-Content -LiteralPath $path) {
        $lineNumber++
        $trimmed = $line.TrimStart()
        if ($trimmed.StartsWith("//")) { continue }
        if ([regex]::IsMatch($line, $forbidden)) {
            $failures.Add("$($path):$($lineNumber): pure 3D engine must not import UI/application: $($line.Trim())")
        }
    }
}

# The GUI adapter must be an opt-in Cargo feature for standalone renderers.
$manifest = Get-Content -LiteralPath (Join-Path $crateRoot "Cargo.toml") -Raw
$librarySource = Get-Content -LiteralPath (Join-Path $sourceRoot "lib.rs") -Raw
if ($manifest -notmatch '(?m)^gpui-sidecar\s*=\s*\["dep:gpui"\]') {
    $failures.Add("gpui-3d must define the optional gpui-sidecar dependency feature")
}
if ($manifest -notmatch '(?m)^gpui\s*=\s*\{[^\r\n]*optional\s*=\s*true') {
    $failures.Add("gpui-3d must mark the gpui dependency optional")
}
if ($librarySource -notmatch '(?s)#\[cfg\(feature\s*=\s*"gpui-sidecar"\)\]\s*(?:pub\s+)?mod scene_view;') {
    $failures.Add("gpui-3d must gate the scene_view adapter behind gpui-sidecar")
}
if ($librarySource -notmatch '(?s)#\[cfg\(feature\s*=\s*"gpui-sidecar"\)\]\s*pub use scene_view::') {
    $failures.Add("gpui-3d must gate the public SceneView symbols behind gpui-sidecar")
}

# A native 3D producer must be supplied from the public engine module, not
# privately recreated under GPUI's paint/sidecar implementation.
$library = Join-Path $sourceRoot "lib.rs"
$sidecar = Join-Path $sourceRoot "scene_view\sidecar_surface.rs"
if (-not (Select-String -LiteralPath $library -SimpleMatch -Pattern 'pub use native_offscreen::' -Quiet)) {
    $failures.Add("gpui-3d must export its framework-independent native offscreen API")
}
if (Select-String -LiteralPath $sidecar -SimpleMatch -Pattern 'struct NativeOffscreenPass {' -Quiet) {
    $failures.Add("GPUI sidecar must not re-own the native 3D graphics pass type")
}

# The boundary guard already in this repo checks that nova-gfx never imports
# GPUI. Keep both checks in CI; neither depends on a GPU or a display server.
if ($failures.Count -gt 0) {
    Write-Host "gpui-3d engine/sidecar boundary violations:" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" }
    exit 1
}
Write-Host "gpui-3d engine/sidecar boundary check passed." -ForegroundColor Green

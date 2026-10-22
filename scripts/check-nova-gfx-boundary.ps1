# Fails if nova-gfx stops being a UI-agnostic graphics library.
# Run from the repository root: pwsh -File scripts/check-nova-gfx-boundary.ps1
$ErrorActionPreference = "Stop"
$failures = [System.Collections.Generic.List[string]]::new()

$novaRoot = Join-Path $PSScriptRoot "..\crates\nova-gfx"
$novaFiles = Get-ChildItem -Recurse $novaRoot -File |
    Where-Object { $_.Extension -in ".rs", ".toml", ".md" }

# 1. No UI-framework names in the graphics library or its docs.
foreach ($file in $novaFiles) {
    $hits = Select-String -Path $file.FullName -Pattern "gpui" -SimpleMatch -CaseSensitive:$false
    foreach ($hit in $hits) {
        $failures.Add("$($hit.Path):$($hit.LineNumber): nova-gfx must not reference gpui: $($hit.Line.Trim())")
    }
}

# 2. gfx-core must not depend on UI, windowing, or a backend crate.
$coreManifest = Join-Path $novaRoot "gfx-core\Cargo.toml"
$forbiddenDeps = @("gpui", "winit", "raw-window-handle", "gfx-vulkan", "gfx-dx11", "gfx-dx12", "gfx-metal", "gfx-opengl")
foreach ($dep in $forbiddenDeps) {
    $hit = Select-String -Path $coreManifest -Pattern "^\s*$([regex]::Escape($dep))\s*=" -CaseSensitive:$false
    if ($hit) {
        $failures.Add("$($coreManifest):$($hit.LineNumber): gfx-core must not depend on $dep")
    }
}

# 3. Removed compatibility/legacy surface must not come back.
$removedSymbols = @(
    "BackendPresentationCompat", "BackendResources", "BackendPipelines",
    "BackendSurface", "BackendQueue", "BackendDiagnostics",
    "RenderStepList", "RenderStepRef", "ResourceSetId",
    "PresentationCapabilities", "TextureRenderStepList",
    "arm_swapchain_frame_ready", "presentation_capabilities",
    "draw_steps_and_present", "draw_steps_to_texture"
)
foreach ($symbol in $removedSymbols) {
    foreach ($file in $novaFiles) {
        $hits = Select-String -Path $file.FullName -Pattern $symbol -SimpleMatch
        foreach ($hit in $hits) {
            $failures.Add("$($hit.Path):$($hit.LineNumber): removed compatibility symbol '$symbol': $($hit.Line.Trim())")
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Host "nova-gfx boundary violations:" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  $_" }
    exit 1
}
Write-Host "nova-gfx boundary check passed." -ForegroundColor Green

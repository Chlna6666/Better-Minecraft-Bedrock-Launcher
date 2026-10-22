# Naming guard for the nova-gfx/gpui boundary.
# Hard failures: public nova-gfx names (the graphics library is the phase-1 gate).
# Warnings: historical gpui-internal names tracked for the phase-2 naming pass.
# Run from the repository root: pwsh -File scripts/check-naming.ps1
$ErrorActionPreference = "Stop"
$hard = [System.Collections.Generic.List[string]]::new()
$soft = [System.Collections.Generic.List[string]]::new()

$novaRoot = Join-Path $PSScriptRoot "..\crates\nova-gfx"
$gpuiNova = Join-Path $PSScriptRoot "..\crates\gpui\src\platform\nova"
$files = @()
$files += Get-ChildItem -Recurse $novaRoot -File -Filter *.rs
$files += Get-ChildItem -Recurse $gpuiNova -File -Filter *.rs
$files = $files | Where-Object {
    $_.Name -notmatch "(^|_)tests?\.rs$" -and
    $_.FullName -notmatch "\\tests?\\"
}

# Reviewed exceptions. Keep this list short and justify new entries in docs/NAMING.md.
$allowedNames = @(
    "render_and_present",
    "disable_allocator_for_test",
    "pending_upload_bytes_for_test",
    "pending_upload_count_for_test",
    "clear_pending_uploads_for_test"
)

$pattern = "^\s*(?<vis>pub(\([^)]*\))?\s+)?(async\s+)?fn\s+(?<name>[A-Za-z0-9_]+)"
foreach ($file in $files) {
    $isNova = $file.FullName.StartsWith($novaRoot)
    $lineNumber = 0
    foreach ($line in Get-Content $file.FullName) {
        $lineNumber++
        $match = [regex]::Match($line, $pattern)
        if (-not $match.Success) { continue }
        $name = $match.Groups["name"].Value
        if (-not $match.Groups["vis"].Success) { continue }
        if ($allowedNames -contains $name) { continue }

        $isChained = $name -match "_(with|for|if_needed|this_frame)_"
        $hasAnd = $name -match "_(and)_"
        $tooLong = $name.Length -gt 40
        if ($isChained -or $hasAnd -or $tooLong) {
            if ($isNova) {
                $hard.Add("$($file.FullName):$($lineNumber): long/chained public name '$name'")
            } else {
                $soft.Add("$($file.FullName):$($lineNumber): phase-2 name '$name'")
            }
        }
        if ($isNova -and $name -match "compat|legacy|resource_set|render_step|draw_step") {
            $hard.Add("$($file.FullName):$($lineNumber): legacy vocabulary in public name '$name'")
        }
    }
}

if ($soft.Count -gt 0) {
    Write-Host "Phase-2 naming findings (reported, not blocking):" -ForegroundColor Yellow
    $soft | Select-Object -First 20 | ForEach-Object { Write-Host "  $_" }
    if ($soft.Count -gt 20) { Write-Host "  ... $($soft.Count - 20) more" }
}
if ($hard.Count -gt 0) {
    Write-Host "Nova-gfx naming violations:" -ForegroundColor Red
    $hard | ForEach-Object { Write-Host "  $_" }
    Write-Host "See docs/NAMING.md for the rules and migration table." -ForegroundColor Yellow
    exit 1
}
Write-Host "Nova-gfx naming check passed." -ForegroundColor Green

# Phase-3 advisory: count long/chained public names elsewhere in gpui. These are
# tracked in docs/naming-audit.md and do not fail this phase-1/2 gate yet.
$gpuiSrc = Join-Path $PSScriptRoot "..\crates\gpui\src"
$advisory = 0
Get-ChildItem -Recurse $gpuiSrc -File -Filter *.rs | Where-Object {
    $_.FullName -notmatch "platform\\nova" -and
    $_.Name -notmatch "(^|_)tests?\.rs$" -and
    $_.FullName -notmatch "\\tests?\\"
} | ForEach-Object {
    $lineNumber = 0
    foreach ($line in Get-Content $_.FullName) {
        $lineNumber++
        $match = [regex]::Match($line, $pattern)
        if (-not $match.Success) { continue }
        if (-not $match.Groups["vis"].Success) { continue }
        $name = $match.Groups["name"].Value
        if ($name.Length -gt 40 -or $name -match "_(and|with|for|if_needed|this_frame)_") {
            $advisory++
        }
    }
}
if ($advisory -gt 0) {
    Write-Host "Phase-3 advisory: $advisory reviewed/kept public names; see docs/NAMING.md for the rules." -ForegroundColor Yellow
}

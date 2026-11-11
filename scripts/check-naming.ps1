# Naming guard for the nova-gfx/gpui boundary.
# Hard failures are only for explicit generic graphics vocabulary contract violations.
# Long names and words such as 'with' or 'and' are semantic review hints, not errors.
# Do not treat this output as an automatic rename list.
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
            $soft.Add("$($file.FullName):$($lineNumber): advisory only; review full semantics before renaming '$name'")
        }
        # 'compat' and 'legacy' may express real protocol/format semantics.
        if ($isNova -and $name -match "(^|_)(resource_set|render_step|draw_step)(_|$)") {
            $hard.Add("$($file.FullName):$($lineNumber): forbidden generic graphics vocabulary '$name'")
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
    Write-Host "See docs/NAMING.md for domain contracts; do not rename unrelated symbols mechanically." -ForegroundColor Yellow
    exit 1
}
Write-Host "Nova-gfx naming check passed." -ForegroundColor Green

# GPUI advisory: count long/chained names. These are not errors; review meanings and callers.
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

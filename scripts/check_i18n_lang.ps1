# Checks that every locale file has the same key set as en-US.lang.
# Usage: pwsh -File scripts/check_i18n_lang.ps1 [-Strict]
param([switch]$Strict)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$localeDir = Join-Path $root "assets/locales"
$baselinePath = Join-Path $localeDir "en-US.lang"
if (-not (Test-Path $baselinePath)) { Write-Error "missing baseline $baselinePath"; exit 2 }

function Read-Keys([string]$path) {
    $keys = [System.Collections.Generic.HashSet[string]]::new()
    foreach ($line in Get-Content -Path $path -Encoding UTF8) {
        $trimmed = $line.Trim()
        if ($trimmed.Length -eq 0 -or $trimmed.StartsWith("#")) { continue }
        $index = $trimmed.IndexOf("=")
        if ($index -le 0) { continue }
        [void]$keys.Add($trimmed.Substring(0, $index))
    }
    return $keys
}

$baseline = Read-Keys $baselinePath
$issues = 0
Get-ChildItem $localeDir -Filter *.lang | Where-Object { $_.Name -ne "en-US.lang" } | ForEach-Object {
    $current = Read-Keys $_.FullName
    $missing = @($baseline | Where-Object { -not $current.Contains($_) })
    $extra = @($current | Where-Object { -not $baseline.Contains($_) })
    if ($missing.Count -gt 0 -or $extra.Count -gt 0) {
        $issues++
        Write-Host "$($_.Name): missing=$($missing.Count) extra=$($extra.Count)" -ForegroundColor Yellow
        $missing | Select-Object -First 10 | ForEach-Object { Write-Host "  missing $_" }
        $extra | Select-Object -First 10 | ForEach-Object { Write-Host "  extra   $_" }
    }
}
if ($issues -gt 0) {
    if ($Strict) { exit 1 }
    Write-Host "i18n key check found $issues locale file(s) with drift (non-strict)." -ForegroundColor Yellow
} else {
    Write-Host "i18n keys are consistent with en-US.lang." -ForegroundColor Green
}

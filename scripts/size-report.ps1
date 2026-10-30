param(
    [switch]$NoBuild,
    [string]$Baseline
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

if (-not $NoBuild) {
    Push-Location $root
    try {
        cargo build --release --bin BMCBL
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

$reportFile = Get-ChildItem -Path (Join-Path $root "target\release\build") -Recurse -Filter "resource_size_report.json" -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $reportFile) {
    throw "resource_size_report.json not found; run a release build first"
}
$report = Get-Content -Raw -LiteralPath $reportFile.FullName | ConvertFrom-Json

$exe = Get-Item -LiteralPath (Join-Path $root "target\release\BMCBL.exe") -ErrorAction SilentlyContinue
if (-not $exe) {
    $exe = Get-ChildItem -Path (Join-Path $root "target\release\deps") -Filter "BMCBL.exe" -File |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
}
if (-not $exe) {
    throw "BMCBL.exe not found; run a release build first"
}

$embedded = [int64]$report.totals.embedded_bytes
$exeBytes = [int64]$exe.Length
$summary = [pscustomobject]@{
    Exe = $exe.FullName
    ExeBytes = $exeBytes
    EmbeddedBytes = $embedded
    NonResourceBytes = $exeBytes - $embedded
    ImagesRaw = [int64]$report.assets.images.source_bytes
    ImagesEmbedded = [int64]$report.assets.images.embedded_bytes
    ImagesDedupSaved = [int64]$report.assets.images.deduplicated_saved_bytes
    ImagesOptimizedSaved = [int64]$report.assets.images.optimized_saved_bytes
    IconsRaw = [int64]$report.assets.icons.source_bytes
    IconsEmbedded = [int64]$report.assets.icons.embedded_bytes
    LocalesSource = [int64]$report.locales.source_bytes
    LocalesEmbedded = [int64]$report.locales.embedded_bytes
    BLoaderSource = [int64]$report.bloader.source_bytes
    BLoaderEmbedded = [int64]$report.bloader.embedded_bytes
    Profile = [string]$report.profile
}

Write-Host "BMCBL release size report"
Write-Host "  EXE:        $($summary.ExeBytes) bytes ($([math]::Round($summary.ExeBytes / 1MB, 2)) MiB)"
Write-Host "  embedded:   $($summary.EmbeddedBytes) bytes"
Write-Host "  other:      $($summary.NonResourceBytes) bytes"
Write-Host "  images:     $($summary.ImagesRaw) -> $($summary.ImagesEmbedded) bytes (dedupe $($summary.ImagesDedupSaved), optimize $($summary.ImagesOptimizedSaved))"
Write-Host "  icons:      $($summary.IconsRaw) -> $($summary.IconsEmbedded) bytes"
Write-Host "  locales:    $($summary.LocalesSource) -> $($summary.LocalesEmbedded) bytes"
Write-Host "  BLoader:    $($summary.BLoaderSource) -> $($summary.BLoaderEmbedded) bytes"
Write-Host "  report:     $($reportFile.FullName)"

if ($Baseline) {
    $base = Get-Content -Raw -LiteralPath $Baseline | ConvertFrom-Json
    $baseExe = $null
    if ($null -ne $base.PSObject.Properties["exe_bytes"]) {
        $baseExe = [int64]$base.exe_bytes
    } elseif ($null -ne $base.PSObject.Properties["ExeBytes"]) {
        $baseExe = [int64]$base.ExeBytes
    } elseif ($null -ne $base.PSObject.Properties["final_exe_bytes"] -and $null -ne $base.final_exe_bytes) {
        $baseExe = [int64]$base.final_exe_bytes
    }
    if ($null -eq $baseExe) {
        throw "Baseline JSON has no exe_bytes / ExeBytes / final_exe_bytes field"
    }
    $delta = $exeBytes - $baseExe
    $percent = if ($baseExe -ne 0) { 100.0 * $delta / $baseExe } else { 0 }
    Write-Host ("  baseline:   {0} bytes; delta {1:+#;-#;0} bytes ({2:N2}%)" -f $baseExe, $delta, $percent)
}

$summary | ConvertTo-Json -Depth 3

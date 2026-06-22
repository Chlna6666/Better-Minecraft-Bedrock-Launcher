param(
    [string]$ExePath = "",
    [int]$DurationSeconds = 12,
    [int]$IntervalMilliseconds = 200,
    [int]$GpuIntervalMilliseconds = 1000,
    [switch]$StopExisting,
    [switch]$SkipWpr,
    [switch]$ResolveSymbols,
    [switch]$BuildRelease,
    [string]$OutDir = ".tmp\startup-profile"
)

$ErrorActionPreference = "Stop"

function Resolve-AbsolutePath([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Path))
}

function Format-Bytes([double]$Bytes) {
    if ($Bytes -ge 1GB) { return "{0:N2} GB" -f ($Bytes / 1GB) }
    if ($Bytes -ge 1MB) { return "{0:N2} MB" -f ($Bytes / 1MB) }
    if ($Bytes -ge 1KB) { return "{0:N2} KB" -f ($Bytes / 1KB) }
    return "{0:N0} B" -f $Bytes
}

function Add-ProcessRegionSummaryLines([object]$Report, [System.Collections.Generic.List[string]]$Summary) {
    if (-not $Report.process_regions) {
        return
    }

    $largestRegions = @($Report.process_regions.largest_regions | Select-Object -First 8)
    if ($largestRegions.Count -eq 0) {
        return
    }

    $Summary.Add("")
    $Summary.Add("## Process Regions")
    $Summary.Add("")
    foreach ($region in $largestRegions) {
        $line = "- {0} @ 0x{1:X} [{2} / {3} / {4}]" -f `
            (Format-Bytes $region.region_size), `
            ([uint64]$region.base_address), `
            $region.state, `
            $region.kind, `
            $region.protection
        $Summary.Add($line)
    }

    $privateSummary = @($Report.process_regions.summary | Where-Object {
        $_.state -eq "MEM_COMMIT" -and $_.kind -eq "MEM_PRIVATE"
    } | Sort-Object reserved_bytes -Descending | Select-Object -First 5)
    if ($privateSummary.Count -gt 0) {
        $Summary.Add("")
        $Summary.Add("### Committed Private Buckets")
        $Summary.Add("")
        foreach ($bucket in $privateSummary) {
            $Summary.Add(
                "- $(Format-Bytes $bucket.reserved_bytes) across $($bucket.region_count) regions [$($bucket.protection)]"
            )
        }
    }
}

function Try-GetCounterSamples([string[]]$Counters) {
    try {
        return (Get-Counter -Counter $Counters -ErrorAction Stop).CounterSamples
    } catch {
        return @()
    }
}

function Sum-GpuUtilizationForPid([int]$ProcessId) {
    $samples = Try-GetCounterSamples @("\GPU Engine(*)\Utilization Percentage")
    $total = 0.0
    foreach ($sample in $samples) {
        if ($sample.InstanceName -like "pid_$ProcessId*") {
            $total += [double]$sample.CookedValue
        }
    }
    return $total
}

function Sum-GpuMemoryForPid([int]$ProcessId) {
    $samples = Try-GetCounterSamples @(
        "\GPU Process Memory(*)\Dedicated Usage",
        "\GPU Process Memory(*)\Shared Usage",
        "\GPU Process Memory(*)\Total Committed"
    )
    $dedicated = 0.0
    $shared = 0.0
    $committed = 0.0
    foreach ($sample in $samples) {
        if ($sample.InstanceName -notlike "pid_$ProcessId*") {
            continue
        }

        if ($sample.Path -like "*\Dedicated Usage") {
            $dedicated += [double]$sample.CookedValue
        } elseif ($sample.Path -like "*\Shared Usage") {
            $shared += [double]$sample.CookedValue
        } elseif ($sample.Path -like "*\Total Committed") {
            $committed += [double]$sample.CookedValue
        }
    }

    [pscustomobject]@{
        DedicatedBytes = $dedicated
        SharedBytes = $shared
        CommittedBytes = $committed
    }
}

function Get-TargetProcesses([string]$Path) {
    $fileName = [System.IO.Path]::GetFileNameWithoutExtension($Path)
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    Get-Process -Name $fileName -ErrorAction SilentlyContinue | Where-Object {
        try {
            $_.Path -and ([System.IO.Path]::GetFullPath($_.Path) -eq $fullPath)
        } catch {
            $false
        }
    }
}

function Stop-WprIfRunning {
    try {
        & wpr -cancel | Out-Null
    } catch {
    }
}

$workspace = Resolve-AbsolutePath "."
$OutDir = Resolve-AbsolutePath $OutDir
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if ($BuildRelease) {
    $oldDebug = $env:CARGO_PROFILE_RELEASE_DEBUG
    $oldStrip = $env:CARGO_PROFILE_RELEASE_STRIP
    try {
        $env:CARGO_PROFILE_RELEASE_DEBUG = "2"
        $env:CARGO_PROFILE_RELEASE_STRIP = "false"
        cargo build --release
    } finally {
        $env:CARGO_PROFILE_RELEASE_DEBUG = $oldDebug
        $env:CARGO_PROFILE_RELEASE_STRIP = $oldStrip
    }
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $releaseExe = Join-Path $workspace "target\release\BMCBL.exe"
    $debugExe = Join-Path $workspace "target\debug\BMCBL.exe"
    if (Test-Path $releaseExe) {
        $ExePath = $releaseExe
    } elseif (Test-Path $debugExe) {
        $ExePath = $debugExe
    } else {
        throw "No BMCBL executable found. Run cargo build first or pass -ExePath."
    }
}

$ExePath = Resolve-AbsolutePath $ExePath
if (-not (Test-Path $ExePath)) {
    throw "Executable not found: $ExePath"
}

$targetName = [System.IO.Path]::GetFileNameWithoutExtension($ExePath)
$existing = @(Get-TargetProcesses $ExePath)
if ($existing) {
    if (-not $StopExisting) {
        $ids = ($existing | Select-Object -ExpandProperty Id) -join ", "
        throw "$targetName is already running from $ExePath (PID $ids). Close it first or rerun with -StopExisting."
    }

    foreach ($process in $existing) {
        $null = $process.CloseMainWindow()
    }
    Start-Sleep -Seconds 2
    foreach ($process in $existing) {
        $stillRunning = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
        if ($stillRunning) {
            Stop-Process -Id $process.Id -Force
        }
    }
}

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$artifactPrefix = $targetName.ToLowerInvariant()
$sampleCsv = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-samples.csv"
$summaryMd = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-summary.md"
$etlPath = Join-Path $OutDir "$artifactPrefix-startup-$timestamp.etl"
$profileTxt = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-xperf-profile.txt"
$symbolProfileTxt = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-xperf-profile-symbols.txt"
$processTxt = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-xperf-process.txt"
$gpuiJson = Join-Path $OutDir "$artifactPrefix-startup-$timestamp-gpui.json"

$wprStarted = $false
if (-not $SkipWpr) {
    Stop-WprIfRunning
    & wpr -start CPU -start GPU -start DiskIO -filemode | Out-Null
    $wprStarted = $true
}

$startInfo = @{
    FilePath = $ExePath
    WorkingDirectory = Split-Path -Parent $ExePath
    PassThru = $true
}
$oldStartupReportPath = $env:GPUI_STARTUP_REPORT_PATH
try {
    $env:GPUI_STARTUP_REPORT_PATH = $gpuiJson
    $process = Start-Process @startInfo
} finally {
    $env:GPUI_STARTUP_REPORT_PATH = $oldStartupReportPath
}
$ProcessId = $process.Id

$logicalProcessors = [Environment]::ProcessorCount
$process.Refresh()
$previousCpu = $process.TotalProcessorTime.TotalMilliseconds
$previousSampleAt = Get-Date
$startedAt = $previousSampleAt
$lastGpuSampleAt = $startedAt
$lastGpuUtilization = 0.0
$lastGpuMemory = [pscustomobject]@{
    DedicatedBytes = 0.0
    SharedBytes = 0.0
    CommittedBytes = 0.0
}
$rows = New-Object System.Collections.Generic.List[object]
$deadline = (Get-Date).AddSeconds($DurationSeconds)

while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds $IntervalMilliseconds
    $sampleAt = Get-Date
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if (-not $process) {
        break
    }

    $currentCpu = $process.TotalProcessorTime.TotalMilliseconds
    $elapsedMs = [Math]::Max(1.0, ($sampleAt - $previousSampleAt).TotalMilliseconds)
    $cpuPercent = (($currentCpu - $previousCpu) / $elapsedMs) * 100.0 / $logicalProcessors
    if (($sampleAt - $lastGpuSampleAt).TotalMilliseconds -ge $GpuIntervalMilliseconds) {
        $lastGpuMemory = Sum-GpuMemoryForPid $ProcessId
        $lastGpuUtilization = Sum-GpuUtilizationForPid $ProcessId
        $lastGpuSampleAt = Get-Date
    }

    $rows.Add([pscustomobject]@{
        Timestamp = $sampleAt.ToString("o")
        ElapsedMs = [Math]::Round(($sampleAt - $startedAt).TotalMilliseconds, 3)
        ProcessId = $ProcessId
        CpuPercent = [Math]::Round($cpuPercent, 3)
        TotalCpuMs = [Math]::Round($currentCpu, 3)
        WorkingSetBytes = [int64]$process.WorkingSet64
        PrivateBytes = [int64]$process.PrivateMemorySize64
        VirtualBytes = [int64]$process.VirtualMemorySize64
        ThreadCount = $process.Threads.Count
        HandleCount = $process.HandleCount
        GpuUtilizationPercent = [Math]::Round($lastGpuUtilization, 3)
        GpuDedicatedBytes = [int64]$lastGpuMemory.DedicatedBytes
        GpuSharedBytes = [int64]$lastGpuMemory.SharedBytes
        GpuCommittedBytes = [int64]$lastGpuMemory.CommittedBytes
    })

    $previousCpu = $currentCpu
    $previousSampleAt = $sampleAt
}

$running = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
if ($running) {
    $null = $running.CloseMainWindow()
    Start-Sleep -Seconds 1
    $running = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($running) {
        Stop-Process -Id $ProcessId -Force
    }
}

if ($wprStarted) {
    & wpr -stop $etlPath | Out-Null
    $env:_NT_SYMBOL_PATH = "srv*$OutDir\symbols*https://msdl.microsoft.com/download/symbols;$workspace\target\release;$workspace\target\debug"
    $env:_NT_SYMCACHE_PATH = "$OutDir\symcache"
    try {
        & xperf -i $etlPath -o $profileTxt -a profile -detail | Out-Null
        if ($ResolveSymbols) {
            & xperf -i $etlPath -symbols -o $symbolProfileTxt -a profile -detail | Out-Null
        }
        & xperf -i $etlPath -o $processTxt -a process | Out-Null
    } catch {
        Write-Warning "xperf post-processing failed: $_"
    }
}

$rows | Export-Csv -NoTypeInformation -Encoding UTF8 -Path $sampleCsv

$maxCpu = $rows | Sort-Object CpuPercent -Descending | Select-Object -First 1
$maxWorkingSet = $rows | Sort-Object WorkingSetBytes -Descending | Select-Object -First 1
$maxPrivate = $rows | Sort-Object PrivateBytes -Descending | Select-Object -First 1
$maxGpu = $rows | Sort-Object GpuUtilizationPercent -Descending | Select-Object -First 1
$maxGpuDedicated = $rows | Sort-Object GpuDedicatedBytes -Descending | Select-Object -First 1
if (-not $maxCpu) {
    $maxCpu = [pscustomobject]@{ CpuPercent = 0.0; Timestamp = "n/a" }
    $maxWorkingSet = [pscustomobject]@{ WorkingSetBytes = 0; Timestamp = "n/a" }
    $maxPrivate = [pscustomobject]@{ PrivateBytes = 0; Timestamp = "n/a" }
    $maxGpu = [pscustomobject]@{ GpuUtilizationPercent = 0.0; Timestamp = "n/a" }
    $maxGpuDedicated = [pscustomobject]@{ GpuDedicatedBytes = 0; Timestamp = "n/a" }
}
$tailCount = [Math]::Max(1, [Math]::Min(10, $rows.Count))
$tailRows = @($rows | Select-Object -Last $tailCount)
$tailCpuAverage = if ($tailRows.Count -gt 0) {
    [Math]::Round((($tailRows | Measure-Object CpuPercent -Average).Average), 3)
} else {
    0.0
}
$tailGpuAverage = if ($tailRows.Count -gt 0) {
    [Math]::Round((($tailRows | Measure-Object GpuUtilizationPercent -Average).Average), 3)
} else {
    0.0
}

$summary = @(
    "# $targetName Startup Profile",
    "",
    "- Executable: ``$ExePath``",
    "- PID: ``$ProcessId``",
    "- Duration: $DurationSeconds seconds",
    "- Samples: $($rows.Count)",
    "- Sample CSV: ``$sampleCsv``",
    "- ETL: ``$etlPath``",
    "- xperf CPU profile: ``$profileTxt``",
    "- xperf symbol profile: ``$symbolProfileTxt``",
    "- GPUI startup report: ``$gpuiJson``",
    "",
    "## Peaks",
    "",
    "- CPU: $($maxCpu.CpuPercent)% at $($maxCpu.Timestamp)",
    "- Working Set: $(Format-Bytes $maxWorkingSet.WorkingSetBytes) at $($maxWorkingSet.Timestamp)",
    "- Private Bytes: $(Format-Bytes $maxPrivate.PrivateBytes) at $($maxPrivate.Timestamp)",
    "- GPU Engine: $($maxGpu.GpuUtilizationPercent)% at $($maxGpu.Timestamp)",
    "- GPU Dedicated: $(Format-Bytes $maxGpuDedicated.GpuDedicatedBytes) at $($maxGpuDedicated.Timestamp)",
    "- Tail CPU average: $tailCpuAverage%",
    "- Tail GPU Engine average: $tailGpuAverage%",
    "",
    "## Notes",
    "",
    "- Minimal GPUI window RAM target: 30 MB working set, treated as a hard goal to measure against rather than a guaranteed current-machine result.",
    "- Open the `.etl` in Windows Performance Analyzer for CPU Usage (Sampled) flame graph/call tree.",
    "- Use the CSV for process CPU, memory, and GPU time-series peaks."
)

if (Test-Path $gpuiJson) {
    $gpuiReport = Get-Content -LiteralPath $gpuiJson -Raw | ConvertFrom-Json
    Add-ProcessRegionSummaryLines -Report $gpuiReport -Summary $summary
    $summary += ""
    $summary += "## GPUI Startup Report"
    $summary += ""
    $summary += '```json'
    $summary += $gpuiReport | ConvertTo-Json -Depth 8
    $summary += '```'
}
$summary | Set-Content -Encoding UTF8 -Path $summaryMd

[pscustomobject]@{
    Summary = $summaryMd
    Samples = $sampleCsv
    Etl = $etlPath
    GpuiStartupReport = $gpuiJson
    XperfProfile = $profileTxt
    XperfSymbolProfile = $symbolProfileTxt
    XperfProcess = $processTxt
}

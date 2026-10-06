#Requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Memory', 'PathMask', 'All')]
    [string]$Suite = 'All',
    [ValidatePattern('^[a-zA-Z0-9][a-zA-Z0-9_-]*$')]
    [string]$Baseline = 'local',
    [string]$OutputDirectory = '',
    [ValidateRange(1, 60)]
    [int]$WarmupSeconds = 1,
    [ValidateRange(1, 300)]
    [int]$MeasurementSeconds = 2
)

$ErrorActionPreference = 'Stop'
$taskWorkspace = Split-Path $PSScriptRoot -Parent
if (!$OutputDirectory) {
    $OutputDirectory = Join-Path $taskWorkspace "target/diagnostics/gpui-cpu-bench/$Baseline"
}
$taskOutput = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Path $taskOutput -Force | Out-Null
$taskFeatures = if ($IsWindows) { 'bench-support,windows-manifest' } else { 'bench-support' }
$taskCommands = [System.Collections.Generic.List[object]]::new()

function Invoke-BenchmarkCommand {
    param([string[]]$Arguments, [string]$LogName)
    $taskLog = Join-Path $taskOutput $LogName
    $taskStarted = [DateTimeOffset]::UtcNow
    & rtk proxy @Arguments 2>&1 | Tee-Object -FilePath $taskLog
    $taskExitCode = $LASTEXITCODE
    $taskCommands.Add([ordered]@{
        arguments = $Arguments
        log = $taskLog
        started_utc = $taskStarted.ToString('o')
        finished_utc = [DateTimeOffset]::UtcNow.ToString('o')
        exit_code = $taskExitCode
    })
    if ($taskExitCode -ne 0) { throw "Benchmark failed with exit code $taskExitCode; see $taskLog" }
}

Push-Location $taskWorkspace
try {
    $taskRevision = & rtk proxy git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Git revision' }
    $taskDirty = @(& rtk proxy git status --porcelain --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Git status' }
    $taskRust = @(& rtk proxy rustc -Vv)
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Rust toolchain' }
    $taskCpu = if ($IsWindows) {
        @(Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors)
    } else { @() }
    $taskInputs = foreach ($taskRelativePath in @(
        'Cargo.lock', 'crates/gpui/Cargo.toml', 'crates/gpui/src/benchmark.rs',
        'crates/gpui/src/benchmark/memory.rs', 'crates/gpui/benches/memory.rs',
        'crates/gpui/src/assets/bitmap_pool.rs', 'crates/gpui/src/assets/bitmap_pool/tests.rs',
        'crates/gpui/src/platform/nova/renderer/draw_step_scratch.rs',
        'scripts/benchmark_gpui_cpu.ps1'
    )) {
        if (Test-Path -LiteralPath $taskRelativePath) {
            [ordered]@{ path = $taskRelativePath; sha256 = (Get-FileHash -LiteralPath $taskRelativePath).Hash }
        }
    }
    $taskPower = if ($IsWindows) { @(& rtk proxy powercfg /getactivescheme) } else { @() }
    if ($IsWindows -and $LASTEXITCODE -ne 0) { throw 'Cannot read active power scheme' }
    $taskMetadata = [ordered]@{
        schema_version = 1
        started_utc = [DateTimeOffset]::UtcNow.ToString('o')
        revision = "$taskRevision".Trim()
        dirty_files = $taskDirty
        rustc = $taskRust
        os = [Runtime.InteropServices.RuntimeInformation]::OSDescription
        architecture = [Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
        logical_processors = [Environment]::ProcessorCount
        cpu = $taskCpu
        active_power_scheme = $taskPower
        input_hashes = @($taskInputs)
        cargo_profile = 'bench/release'
        features = $taskFeatures
        allocator = 'Rust System (standalone GPUI benchmark binary)'
        bitmap_pool_scope = 'process-global; run serially in its own benchmark process'
        suite = $Suite
        baseline = $Baseline
        memory_warmup_seconds = $WarmupSeconds
        memory_measurement_seconds = $MeasurementSeconds
        memory_sample_size = 30
        notes = 'CPU-only workloads. Capacity is not RSS or heap fragmentation; batch averages are not frame/scanout percentiles.'
    }
    $taskMetadata | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $taskOutput 'metadata.json')
    $taskCommon = @('--manifest-path', 'crates/gpui/Cargo.toml', '--locked', '--no-default-features', '--features', $taskFeatures)
    if ($Suite -in @('Memory', 'All')) {
        Invoke-BenchmarkCommand -LogName 'memory.log' -Arguments (@('cargo', 'bench') + $taskCommon + @(
            '--bench', 'memory', '--', '--warm-up-time', "$WarmupSeconds",
            '--measurement-time', "$MeasurementSeconds", '--save-baseline', $Baseline
        ))
    }
    if ($Suite -in @('PathMask', 'All')) {
        Invoke-BenchmarkCommand -LogName 'path-mask.log' -Arguments (@('cargo', 'test') + $taskCommon + @(
            '--release', '--lib', 'path_mask_cache_benchmark', '--', '--ignored', '--nocapture', '--test-threads=1'
        ))
    }
} finally {
    $taskCommands | ConvertTo-Json -Depth 8 -AsArray | Set-Content (Join-Path $taskOutput 'commands.json')
    Pop-Location
}

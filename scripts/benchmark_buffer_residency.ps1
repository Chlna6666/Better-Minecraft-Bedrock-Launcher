# Run fresh-process A/B/B/A trials; requires the release buffer_residency_lab example.
# Writes raw JSON reports and a CSV index under OutputDirectory. Does not test Metal.
param(
    [string]$Executable = 'target/release/examples/buffer_residency_lab.exe',
    [string]$OutputDirectory = 'target/p1a-memory/release',
    [string]$DiscreteAdapter = ''
)

$labExecutable = (Resolve-Path -LiteralPath $Executable -ErrorAction Stop).Path
New-Item -ItemType Directory -Path $OutputDirectory -Force -ErrorAction Stop | Out-Null
$labPairs = @('nova-dx11', 'nova-dx12', 'nova-vulkan', 'nova-opengl') | ForEach-Object {
    [pscustomobject]@{ Backend = $_; Adapter = ''; Tag = 'default' }
}
if ($DiscreteAdapter) {
    $labPairs += @('nova-dx12', 'nova-vulkan') | ForEach-Object {
        [pscustomobject]@{ Backend = $_; Adapter = $DiscreteAdapter; Tag = 'selected' }
    }
}
$labScenarios = @(
    @{ Name = 'static-64k-draw8'; Bytes = 65536; Draws = 8; UpdateEvery = 0 },
    @{ Name = 'static-1m-draw8'; Bytes = 1048576; Draws = 8; UpdateEvery = 0 },
    @{ Name = 'static-8m-draw8'; Bytes = 8388608; Draws = 8; UpdateEvery = 0 },
    @{ Name = 'static-8m-draw1'; Bytes = 8388608; Draws = 1; UpdateEvery = 0 },
    @{ Name = 'dirty-8m-draw8'; Bytes = 8388608; Draws = 8; UpdateEvery = 16 }
)
$labIndex = [System.Collections.Generic.List[object]]::new()
foreach ($labPair in $labPairs) {
    foreach ($labScenario in $labScenarios) {
        $labPolicies = @('cpu-visible', 'gpu-only', 'gpu-only', 'cpu-visible')
        for ($labTrial = 0; $labTrial -lt $labPolicies.Count; $labTrial++) {
            $labPolicy = $labPolicies[$labTrial]
            $labArguments = @(
                "--backend=$($labPair.Backend)", "--memory=$labPolicy",
                "--bytes=$($labScenario.Bytes)", "--draws=$($labScenario.Draws)",
                "--update-every=$($labScenario.UpdateEvery)", '--samples=80', '--warmup=16'
            )
            if ($labPair.Adapter) { $labArguments += "--adapter=$($labPair.Adapter)" }
            $labStem = "$($labPair.Backend)-$($labPair.Tag)-$($labScenario.Name)-$labTrial-$labPolicy"
            $labOutput = & $labExecutable @labArguments
            if ($LASTEXITCODE -ne 0) { throw "Native baseline failed: $labStem (exit $LASTEXITCODE)" }
            $labReport = ($labOutput -join [Environment]::NewLine) | ConvertFrom-Json -Depth 100 -ErrorAction Stop
            if ($labReport.status -ne 'passed' -or $labReport.build.debug_assertions -ne $false) {
                throw "Expected successful optimized report: $labStem"
            }
            if ($labPair.Adapter -and $labReport.adapter -ne $labPair.Adapter) {
                throw "Requested adapter was not selected: $labStem -> $($labReport.adapter)"
            }
            $labPath = Join-Path $OutputDirectory "$labStem.json"
            $labOutput | Set-Content -LiteralPath $labPath -Encoding utf8 -ErrorAction Stop
            $labIndex.Add([pscustomobject]@{
                Backend = $labReport.backend; Adapter = $labReport.adapter
                Architecture = $labReport.memory_architecture; Scenario = $labScenario.Name
                Memory = $labPolicy; Trial = $labTrial; PixelChecks = $labReport.pixel_checks
                CreateUs = $labReport.create_us; InitialUploadUs = $labReport.initial_upload_us
                CompletionP50Us = $labReport.summaries.completion_us.p50
                CompletionP95Us = $labReport.summaries.completion_us.p95
                CompletionP99Us = $labReport.summaries.completion_us.p99
                DirtyUploadP50Us = $labReport.summaries.dirty_upload_us.p50
                AllocatedBytes = $labReport.final_memory.allocated_bytes
                ReservedBytes = $labReport.final_memory.reserved_bytes
                Report = $labPath
            })
            $labIndex | Export-Csv -LiteralPath (Join-Path $OutputDirectory 'index.csv') -NoTypeInformation -Encoding utf8 -ErrorAction Stop
            Write-Host "PASS $labStem [$($labReport.adapter), $($labReport.memory_architecture)]"
        }
    }
}

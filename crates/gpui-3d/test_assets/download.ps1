# Downloads and verifies the gpui-3d public glTF test models.
#
# The models are fetched once and then reused offline. A failed download or hash mismatch is an
# error, never a skipped test. Empty hashes print the computed hash so maintainers can pin it.
#
# Usage:
#   pwsh -NoProfile -File crates/gpui-3d/test_assets/download.ps1
#   pwsh -NoProfile -File crates/gpui-3d/test_assets/download.ps1 -Force

param(
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$cache = Join-Path $root 'cache'
$base = 'https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models'

# Each entry: label, url, cache-relative path, sha256.
$files = @(
    @('DamagedHelmet', "$base/DamagedHelmet/glTF-Binary/DamagedHelmet.glb", 'DamagedHelmet.glb', 'a1e3b04de97b11de564ce6e53b95f02954a297f0008183ac63a4f5974f6b32d8'),
    @('AnimatedCube gltf', "$base/AnimatedCube/glTF/AnimatedCube.gltf", 'AnimatedCube/AnimatedCube.gltf', '59a3c451a5167b8ac4b59d61d4b94b2b7a215ee827efaed75629677a1217b552'),
    @('AnimatedCube bin', "$base/AnimatedCube/glTF/AnimatedCube.bin", 'AnimatedCube/AnimatedCube.bin', '82de770fc82b48a77a33bb26abd4d7e75620491b36ddf98e8dca69fa73dd798c'),
    @('AnimatedCube albedo', "$base/AnimatedCube/glTF/AnimatedCube_BaseColor.png", 'AnimatedCube/AnimatedCube_BaseColor.png', '0750d5a03c1bebc640571e309f66c6e88efbff2ef4c120619466a7014551bb9a'),
    @('BoxVertexColors', "$base/BoxVertexColors/glTF-Binary/BoxVertexColors.glb", 'BoxVertexColors.glb', '9c48227f33b0ba2fbcf23b98ebf60d1c8ae0c6e6c5281e0aa3cc58affee10382')
)

$failures = 0
foreach ($file in $files) {
    $label = $file[0]
    $url = $file[1]
    $relative = $file[2]
    $expected = $file[3]
    $target = Join-Path $cache $relative
    New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
    $needsDownload = $Force -or -not (Test-Path $target)
    if (-not $needsDownload) {
        $actual = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLowerInvariant()
        if ($expected -eq '' -or $actual -eq $expected) {
            Write-Output ("ok      $label ($relative)")
            continue
        }
        Write-Output ("stale   $label, re-downloading")
        $needsDownload = $true
    }
    if ($needsDownload) {
        try {
            Invoke-WebRequest -Uri $url -OutFile $target -UseBasicParsing -TimeoutSec 120
        } catch {
            Write-Output ("FAILED  $label download: $($_.Exception.Message)")
            $failures += 1
            continue
        }
    }
    $actual = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLowerInvariant()
    if ($expected -ne '' -and $actual -ne $expected) {
        Write-Output ("FAILED  $label hash mismatch: expected $expected, actual $actual")
        $failures += 1
        continue
    }
    $bytes = (Get-Item $target).Length
    Write-Output ("downloaded $label sha256=$actual bytes=$bytes")
}
if ($failures -gt 0) {
    Write-Output "FAILED $failures test asset downloads"
    exit 1
}
Write-Output "all test assets verified in $cache"

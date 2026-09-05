param(
    [string[]]$Roots = @("src", "crates/gpui/src"),
    [switch]$VerboseOutput
)

$ErrorActionPreference = "Stop"

# Local/static guard only. It is deliberately not wired to GitHub CI.
# Contract:
#   current-frame visual sample -> Window::animation_time()
#   event/scheduler/deadline/profiling -> Instant::now()
#
# The check is semantic-by-scope rather than a repository-wide ban on Instant::now().

$repoRoot = Split-Path -Parent $PSScriptRoot
$violations = [System.Collections.Generic.List[object]]::new()
$lifecycleNames = @("render", "request_layout", "prepaint", "paint")
$visualHelperPattern = '^(theme_colors|current_theme_colors|detached_theme_colors|render_.+|build_render_model|sync_.+_animation)$'
$freshClockPattern = '(?:std::time::)?Instant::now\s*\(\s*\)'
$explicitMonotonicMarker = 'animation-clock:\s*monotonic-ok'

function Add-Violation {
    param(
        [string]$Path,
        [int]$Line,
        [string]$Reason,
        [string]$Text
    )

    $violations.Add([pscustomobject]@{
        Path = $Path
        Line = $Line
        Reason = $Reason
        Text = $Text.Trim()
    })
}

function Get-RustFunctionRanges {
    param([string[]]$Lines)

    $ranges = [System.Collections.Generic.List[object]]::new()
    $functionPattern = '^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b'

    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -notmatch $functionPattern) {
            continue
        }

        $name = $Matches[1]
        $depth = 0
        $started = $false
        $end = $i

        for ($j = $i; $j -lt $Lines.Count; $j++) {
            $line = $Lines[$j]
            $opens = ([regex]::Matches($line, '\{')).Count
            $closes = ([regex]::Matches($line, '\}')).Count

            if ($opens -gt 0) {
                $started = $true
            }

            if ($started) {
                $depth += $opens
                $depth -= $closes
                if ($depth -le 0) {
                    $end = $j
                    break
                }
            }
        }

        $ranges.Add([pscustomobject]@{
            Name = $name
            Start = $i
            End = $end
        })
        $i = [Math]::Max($i, $end)
    }

    return $ranges
}

function Test-ExplicitMonotonicException {
    param(
        [string[]]$Lines,
        [int]$Index
    )

    if ($Lines[$Index] -match $explicitMonotonicMarker) {
        return $true
    }
    if ($Index -gt 0 -and $Lines[$Index - 1] -match $explicitMonotonicMarker) {
        return $true
    }
    return $false
}

foreach ($root in $Roots) {
    $fullRoot = Join-Path $repoRoot $root
    if (-not (Test-Path $fullRoot)) {
        Write-Warning "animation clock audit root not found: $root"
        continue
    }

    $files = Get-ChildItem -Path $fullRoot -Recurse -File -Filter *.rs
    foreach ($file in $files) {
        $relative = [IO.Path]::GetRelativePath($repoRoot, $file.FullName).Replace('\\', '/')
        $lines = Get-Content -LiteralPath $file.FullName

        foreach ($range in (Get-RustFunctionRanges -Lines $lines)) {
            $isLifecycle = $lifecycleNames -contains $range.Name
            $isVisualHelper = $range.Name -match $visualHelperPattern
            if (-not $isLifecycle -and -not $isVisualHelper) {
                continue
            }

            for ($i = $range.Start; $i -le $range.End; $i++) {
                if ($lines[$i] -notmatch $freshClockPattern) {
                    continue
                }
                if (Test-ExplicitMonotonicException -Lines $lines -Index $i) {
                    continue
                }

                $reason = if ($isLifecycle) {
                    "fresh clock inside $($range.Name) lifecycle"
                } else {
                    "fresh clock hidden inside visual helper $($range.Name)"
                }
                Add-Violation -Path $relative -Line ($i + 1) -Reason $reason -Text $lines[$i]
            }
        }
    }
}

if ($violations.Count -gt 0) {
    Write-Host "GPUI animation clock audit failed: $($violations.Count) violation(s)." -ForegroundColor Red
    foreach ($violation in $violations) {
        Write-Host ("{0}:{1}: {2}: {3}" -f $violation.Path, $violation.Line, $violation.Reason, $violation.Text)
    }
    Write-Host ""
    Write-Host "Current-frame visual sampling must use window.animation_time()." -ForegroundColor Yellow
    Write-Host "Real event/scheduler/deadline/profiling reads remain Instant::now()." -ForegroundColor Yellow
    Write-Host "If a lifecycle read is intentionally monotonic and cannot affect visual sampling, document it with: // animation-clock: monotonic-ok" -ForegroundColor Yellow
    exit 1
}

if ($VerboseOutput) {
    Write-Host "Audited roots: $($Roots -join ', ')"
}
Write-Host "GPUI animation clock audit passed."

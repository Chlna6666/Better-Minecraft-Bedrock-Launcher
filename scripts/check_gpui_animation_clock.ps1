param(
    [string[]]$Roots = @("src", "crates/gpui/src"),
    [switch]$VerboseOutput
)

$ErrorActionPreference = "Stop"

# This is intentionally a local/static guard, not a CI repair mechanism.
# It catches high-signal violations of the repository animation-clock contract:
# current-frame visual sampling must use Window::animation_time(), while real
# event/scheduler/profiling time continues to use Instant::now().

$repoRoot = Split-Path -Parent $PSScriptRoot
$violations = [System.Collections.Generic.List[object]]::new()

$directPatterns = @(
    @{ Name = "theme factor samples fresh clock"; Regex = 'factor\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)\s*\)' },
    @{ Name = "raw progress samples fresh clock"; Regex = 'raw_progress\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)' },
    @{ Name = "eased progress samples fresh clock"; Regex = 'eased_progress\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)' },
    @{ Name = "animation sample uses fresh clock"; Regex = '\.sample\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)\s*\)' },
    @{ Name = "animation value uses fresh clock"; Regex = '\.value\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)\s*\)' },
    @{ Name = "animation state check uses fresh clock"; Regex = '\.is_animating\s*\(\s*(?:std::time::)?Instant::now\s*\(\s*\)\s*\)' }
)

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
    $lifecyclePattern = '^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+(render|request_layout|prepaint|paint)\b'

    for ($i = 0; $i -lt $Lines.Count; $i++) {
        if ($Lines[$i] -notmatch $lifecyclePattern) {
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

        # Exact high-signal patterns are invalid anywhere because they hide a fresh
        # clock inside the visual sampling expression. Event/scheduler code should
        # record Instant::now() separately and pass that timestamp explicitly.
        for ($i = 0; $i -lt $lines.Count; $i++) {
            foreach ($pattern in $directPatterns) {
                if ($lines[$i] -match $pattern.Regex) {
                    Add-Violation -Path $relative -Line ($i + 1) -Reason $pattern.Name -Text $lines[$i]
                }
            }
        }

        # Lifecycle functions must never obtain a fresh animation sample. This
        # catches `let now = Instant::now()` followed by indirect helper use.
        foreach ($range in (Get-RustFunctionRanges -Lines $lines)) {
            for ($i = $range.Start; $i -le $range.End; $i++) {
                if ($lines[$i] -match '(?:std::time::)?Instant::now\s*\(\s*\)') {
                    Add-Violation `
                        -Path $relative `
                        -Line ($i + 1) `
                        -Reason "fresh clock inside $($range.Name) lifecycle" `
                        -Text $lines[$i]
                }
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
    Write-Host "Visual-frame sampling must use window.animation_time()." -ForegroundColor Yellow
    Write-Host "Keep Instant::now() only for event/scheduler/deadline/profiling semantics outside current-frame visual sampling." -ForegroundColor Yellow
    exit 1
}

if ($VerboseOutput) {
    Write-Host "Audited roots: $($Roots -join ', ')"
}
Write-Host "GPUI animation clock audit passed."

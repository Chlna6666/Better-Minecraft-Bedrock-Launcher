[CmdletBinding()]
param(
    [string]$OutputPath = "CHANGELOG.md",
    [string]$SinceTag,
    [string]$Until = "HEAD",
    [string]$Version,
    [string]$ReleaseDate,
    [switch]$ReplaceUnreleased,
    [switch]$ReleaseNotes,
    [switch]$ArchiveRelease
)

$ErrorActionPreference = "Stop"

function Resolve-LatestStableTag {
    param([string]$ExcludedTag)
    $tags = @(& git tag --list 'v*' --sort=-version:refname)
    if ($LASTEXITCODE -ne 0) { throw "Unable to list Git tags." }
    return $tags | Where-Object { $_ -match '^v\d+\.\d+\.\d+$' -and $_ -ne $ExcludedTag } |
        Select-Object -First 1
}

function Invoke-ReleaseNoteGenerator {
    param([string]$StartTag, [string]$EndReference)
    $python = Get-Command python -ErrorAction SilentlyContinue
    if (-not $python) { $python = Get-Command python3 -ErrorAction SilentlyContinue }
    if (-not $python) { throw "Python 3 is required to generate the changelog." }
    $generator = Join-Path $PSScriptRoot "release_notes.py"
    $arguments = @($generator, "--format", "changelog", "--until", $EndReference)
    if (-not [string]::IsNullOrWhiteSpace($StartTag)) {
        $arguments += @("--since", $StartTag)
    }
    $output = @(& $python.Source @arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "Release note generation failed with exit code $LASTEXITCODE."
    }
    return ($output -join [Environment]::NewLine).TrimEnd()
}

function Update-UnreleasedSection {
    param(
        [Parameter(Mandatory)]
        [string]$Content,
        [Parameter(Mandatory)]
        [string]$GeneratedText
    )

    $sectionPattern = '(?ms)^## \[Unreleased\].*?(?=^## \[|\z)'
    $sectionMatch = [regex]::Match($Content, $sectionPattern)
    if (-not $sectionMatch.Success) {
        return "# Changelog`r`n`r`n## [Unreleased]`r`n`r`n$GeneratedText`r`n`r`n$Content".Trim() + "`r`n"
    }

    $section = $sectionMatch.Value.TrimEnd()
    $generatedPattern = '(?ms)<!-- changelog:generated:start -->.*?<!-- changelog:generated:end -->'
    if ([regex]::IsMatch($section, $generatedPattern)) {
        $section = [regex]::Replace($section, $generatedPattern, $GeneratedText)
    } else {
        $section = "$section`r`n`r`n$GeneratedText"
    }

    $updated = $Content.Substring(0, $sectionMatch.Index) + $section + $Content.Substring($sectionMatch.Index + $sectionMatch.Length)
    return $updated.TrimEnd() + "`r`n"
}

function Archive-UnreleasedSection {
    param(
        [Parameter(Mandatory)] [string]$Content,
        [Parameter(Mandatory)] [string]$ReleaseVersion,
        [Parameter(Mandatory)] [string]$Date
    )

    if ($ReleaseVersion -notmatch '^\d+\.\d+\.\d+$') {
        throw "Release version must use MAJOR.MINOR.PATCH format: $ReleaseVersion"
    }

    $sectionPattern = '(?ms)^## \[Unreleased\].*?(?=^## \[|\z)'
    $sectionMatch = [regex]::Match($Content, $sectionPattern)
    if (-not $sectionMatch.Success) {
        throw "CHANGELOG.md does not contain an [Unreleased] section."
    }

    $section = $sectionMatch.Value.Trim()
    $section = [regex]::Replace($section, '(?m)^## \[Unreleased\]\s*', "## [$ReleaseVersion] - $Date`r`n")
    $replacement = "## [Unreleased]`r`n`r`n$($section.Trim())`r`n"
    $updated = $Content.Substring(0, $sectionMatch.Index) + $replacement +
        $Content.Substring($sectionMatch.Index + $sectionMatch.Length)
    return $updated.TrimEnd() + "`r`n"
}

if ($ArchiveRelease) {
    if ([string]::IsNullOrWhiteSpace($Version)) {
        throw "-ArchiveRelease requires -Version."
    }

    $date = if ([string]::IsNullOrWhiteSpace($ReleaseDate)) {
        (Get-Date).ToString("yyyy-MM-dd")
    } else { $ReleaseDate }
    if (-not (Test-Path $OutputPath)) {
        throw "CHANGELOG file does not exist: $OutputPath"
    }

    $existing = Get-Content -Raw -Path $OutputPath -Encoding utf8
    $updated = Archive-UnreleasedSection -Content $existing -ReleaseVersion $Version -Date $date
    $writePath = (Resolve-Path $OutputPath).Path
    [System.IO.File]::WriteAllText($writePath, $updated, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "Archived [Unreleased] as [$Version] - $date"
    exit 0
}

$resolvedSinceTag = $SinceTag
if ([string]::IsNullOrWhiteSpace($resolvedSinceTag)) {
    $resolvedSinceTag = Resolve-LatestStableTag -ExcludedTag $Until
}
$generated = Invoke-ReleaseNoteGenerator -StartTag $resolvedSinceTag -EndReference $Until

if ($ReleaseNotes) {
    $generated | Set-Content -Path $OutputPath -Encoding utf8
    Write-Host "Generated release notes at $OutputPath"
    exit 0
}
if (-not $ReplaceUnreleased) {
    throw "Specify -ReplaceUnreleased or -ReleaseNotes."
}

$existing = if (Test-Path $OutputPath) {
    Get-Content -Raw -Path $OutputPath -Encoding utf8
} else {
    "# Changelog`r`n`r`n"
}
$marked = "<!-- changelog:generated:start -->`r`n$generated`r`n<!-- changelog:generated:end -->"
$updated = Update-UnreleasedSection -Content $existing -GeneratedText $marked
$writePath = $OutputPath
if (Test-Path $OutputPath) {
    $writePath = (Resolve-Path $OutputPath).Path
}
[System.IO.File]::WriteAllText($writePath, $updated, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "Updated $OutputPath using range $resolvedSinceTag..$Until"

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

function Invoke-GitText {
    param(
        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    $output = @(& git @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }

    return $output
}

function Resolve-LatestStableTag {
    param(
        [string]$ExcludedTag
    )

    $stableTags = @(Invoke-GitText @("tag", "--list", "v*", "--sort=-version:refname") |
        Where-Object { $_ -match '^v\d+\.\d+\.\d+$' })

    return $stableTags |
        Where-Object { $_ -ne $ExcludedTag } |
        Select-Object -First 1
}

function Get-CommitRecords {
    param(
        [string]$StartTag,
        [Parameter(Mandatory)]
        [string]$EndReference
    )

    $range = if ([string]::IsNullOrWhiteSpace($StartTag)) {
        $EndReference
    } else {
        "$StartTag..$EndReference"
    }

    $records = @(Invoke-GitText @(
            "log",
            $range,
            "--no-merges",
            "--format=%h%x1f%s%x1f%ad",
            "--date=short"
        ))

    $commits = [System.Collections.Generic.List[object]]::new()
    foreach ($record in $records) {
        if ([string]::IsNullOrWhiteSpace($record)) {
            continue
        }

        $parts = $record -split [char]0x1f, 3
        if ($parts.Count -ne 3) {
            continue
        }

        if ($parts[1] -match '^docs\(changelog\):') {
            continue
        }

        $commits.Add([pscustomobject]@{
                Hash    = $parts[0]
                Subject = $parts[1]
                Date    = $parts[2]
            })
    }

    return $commits
}

function Get-CommitCategory {
    param(
        [Parameter(Mandatory)]
        [string]$Subject
    )

    if ($Subject -match '^feat(?:\([^)]*\))?:') { return "Added" }
    if ($Subject -match '^fix(?:\([^)]*\))?:') { return "Fixed" }
    if ($Subject -match '^perf(?:\([^)]*\))?:') { return "Performance" }
    if ($Subject -match '^docs(?:\([^)]*\))?:') { return "Documentation" }
    if ($Subject -match '^(refactor|style)(?:\([^)]*\))?:') { return "Changed" }
    if ($Subject -match '^(build|ci|chore|test|revert)(?:\([^)]*\))?:') { return "Maintenance" }
    return "Changed"
}

function Format-CommitSubject {
    param(
        [Parameter(Mandatory)]
        [string]$Subject
    )

    return $Subject -replace '^[a-z]+(?:\([^)]*\))?:\s*', ''
}

function Build-ChangeText {
    param(
        [Parameter(Mandatory)]
        [object[]]$Commits,
        [string]$StartTag,
        [Parameter(Mandatory)]
        [string]$EndReference,
        [switch]$IncludeMarkers
    )

    $categories = [ordered]@{
        Added         = [System.Collections.Generic.List[string]]::new()
        Fixed         = [System.Collections.Generic.List[string]]::new()
        Performance   = [System.Collections.Generic.List[string]]::new()
        Changed       = [System.Collections.Generic.List[string]]::new()
        Documentation = [System.Collections.Generic.List[string]]::new()
        Maintenance   = [System.Collections.Generic.List[string]]::new()
    }

    foreach ($commit in $Commits) {
        $category = Get-CommitCategory $commit.Subject
        $summary = Format-CommitSubject $commit.Subject
        $categories[$category].Add(('- {0} (`{1}`, {2})' -f $summary, $commit.Hash, $commit.Date))
    }

    $rangeLabel = if ([string]::IsNullOrWhiteSpace($StartTag)) {
        ('through `{0}`' -f $EndReference)
    } else {
        ('from `{0}` through `{1}`' -f $StartTag, $EndReference)
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    if ($IncludeMarkers) {
        $lines.Add("<!-- changelog:generated:start -->")
    }
    $lines.Add("### Commit Summary")
    $lines.Add("")
    $lines.Add("Automatically generated $rangeLabel.")
    $lines.Add("")

    foreach ($category in $categories.Keys) {
        if ($categories[$category].Count -eq 0) {
            continue
        }

        $lines.Add("### $category")
        $lines.AddRange($categories[$category])
        $lines.Add("")
    }

    if ($Commits.Count -eq 0) {
        $lines.Add("No commits found in this range.")
        $lines.Add("")
    }

    if ($IncludeMarkers) {
        $lines.Add("<!-- changelog:generated:end -->")
    }

    return ($lines -join [Environment]::NewLine).TrimEnd()
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

$resolvedSinceTag = $SinceTag
if ([string]::IsNullOrWhiteSpace($resolvedSinceTag)) {
    $resolvedSinceTag = Resolve-LatestStableTag -ExcludedTag $Until
}

$commits = @(Get-CommitRecords -StartTag $resolvedSinceTag -EndReference $Until)
if ($ReleaseNotes) {
    $notes = Build-ChangeText -Commits $commits -StartTag $resolvedSinceTag -EndReference $Until
    $notes | Set-Content -Path $OutputPath -Encoding utf8
    Write-Host "Generated release notes at $OutputPath"
    exit 0
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

if (-not $ReplaceUnreleased) {
    throw "Specify -ReplaceUnreleased or -ReleaseNotes."
}

$existing = if (Test-Path $OutputPath) {
    Get-Content -Raw -Path $OutputPath -Encoding utf8
} else {
    "# Changelog`r`n`r`n"
}

$generated = Build-ChangeText -Commits $commits -StartTag $resolvedSinceTag -EndReference $Until -IncludeMarkers
$updated = Update-UnreleasedSection -Content $existing -GeneratedText $generated
$writePath = $OutputPath
if (Test-Path $OutputPath) {
    $writePath = (Resolve-Path $OutputPath).Path
}
[System.IO.File]::WriteAllText($writePath, $updated, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "Updated $OutputPath using range $resolvedSinceTag..$Until"

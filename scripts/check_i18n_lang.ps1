param(
    [string]$LocalesDir = (Join-Path $PSScriptRoot '..\\assets\\locales'),
    [switch]$FixOrder
)

$ErrorActionPreference = 'Stop'

function Read-LangEntries {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $entries = New-Object System.Collections.Generic.List[object]
    $lineNumber = 0
    foreach ($line in Get-Content -Path $Path -Encoding UTF8) {
        $lineNumber++
        $trimmed = $line.Trim()
        if ($trimmed.Length -eq 0 -or $trimmed.StartsWith('#') -or $trimmed.StartsWith('//')) {
            continue
        }

        $parts = $line.Split('=', 2)
        if ($parts.Length -ne 2) {
            throw "Invalid lang line: ${Path}:$lineNumber"
        }

        $key = $parts[0].Trim()
        if ([string]::IsNullOrWhiteSpace($key)) {
            throw "Empty key: ${Path}:$lineNumber"
        }

        $entries.Add([pscustomobject]@{
            Key = $key
            Value = $parts[1]
            LineNumber = $lineNumber
        })
    }

    return $entries
}

function Test-SortedKeys {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.Generic.List[object]]$Entries,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $sortedKeys = [string[]]$Entries.Key
    [Array]::Sort($sortedKeys, [System.StringComparer]::Ordinal)
    for ($index = 0; $index -lt $Entries.Count; $index++) {
        if ($Entries[$index].Key -cne $sortedKeys[$index]) {
            throw "Key order error in $Name at line $($Entries[$index].LineNumber): '$($Entries[$index].Key)' should be '$($sortedKeys[$index])'"
        }
    }
}

function Write-SortedLangFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [System.Collections.Generic.List[object]]$Entries
    )

    $sortedEntries = [System.Collections.Generic.List[object]]::new()
    foreach ($entry in $Entries) {
        [void]$sortedEntries.Add($entry)
    }

    $sortedEntries.Sort([System.Collections.Generic.Comparer[object]]::Create({
        param($left, $right)
        return [System.StringComparer]::Ordinal.Compare($left.Key, $right.Key)
    }))

    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $content = ($sortedEntries | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join [Environment]::NewLine
    [System.IO.File]::WriteAllText($Path, $content + [Environment]::NewLine, $utf8NoBom)
}

function Test-DuplicateKeys {
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.Generic.List[object]]$Entries,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $seen = @{}
    foreach ($entry in $Entries) {
        if ($seen.ContainsKey($entry.Key)) {
            throw "Duplicate key in ${Name}: '$($entry.Key)' at lines $($seen[$entry.Key]) and $($entry.LineNumber)"
        }
        $seen[$entry.Key] = $entry.LineNumber
    }
}

$resolvedLocalesDir = (Resolve-Path $LocalesDir).Path
$langFiles = Get-ChildItem -Path $resolvedLocalesDir -Filter *.lang | Sort-Object Name
if ($langFiles.Count -eq 0) {
    throw "No .lang files found in $resolvedLocalesDir"
}

$tables = @{}
foreach ($file in $langFiles) {
    $entries = Read-LangEntries -Path $file.FullName
    Test-DuplicateKeys -Entries $entries -Name $file.Name
    if ($FixOrder) {
        Write-SortedLangFile -Path $file.FullName -Entries $entries
        $entries = Read-LangEntries -Path $file.FullName
    }
    Test-SortedKeys -Entries $entries -Name $file.Name
    $tables[$file.Name] = $entries
}

$baseFile = $langFiles[0].Name
$baseKeys = New-Object System.Collections.Generic.HashSet[string]
foreach ($entry in $tables[$baseFile]) {
    [void]$baseKeys.Add($entry.Key)
}

$missingFound = $false
foreach ($file in $langFiles | Select-Object -Skip 1) {
    $currentKeys = New-Object System.Collections.Generic.HashSet[string]
    foreach ($entry in $tables[$file.Name]) {
        [void]$currentKeys.Add($entry.Key)
    }

    $missing = New-Object System.Collections.Generic.List[string]
    foreach ($key in $baseKeys) {
        if (-not $currentKeys.Contains($key)) {
            $missing.Add($key)
        }
    }

    $extra = New-Object System.Collections.Generic.List[string]
    foreach ($key in $currentKeys) {
        if (-not $baseKeys.Contains($key)) {
            $extra.Add($key)
        }
    }

    if ($missing.Count -gt 0 -or $extra.Count -gt 0) {
        $missingFound = $true
        Write-Host "Locale mismatch: $($file.Name)" -ForegroundColor Red
        if ($missing.Count -gt 0) {
            Write-Host "  Missing ($($missing.Count)): $($missing -join ', ')" -ForegroundColor Yellow
        }
        if ($extra.Count -gt 0) {
            Write-Host "  Extra ($($extra.Count)): $($extra -join ', ')" -ForegroundColor Yellow
        }
    }
}

if ($missingFound) {
    throw "Locale key mismatch detected."
}

Write-Host "Checked $($langFiles.Count) locale files successfully." -ForegroundColor Green

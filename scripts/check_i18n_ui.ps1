param(
    [string]$UiDir = (Join-Path $PSScriptRoot '..\src\ui'),
    [string]$LocalesDir = (Join-Path $PSScriptRoot '..\assets\locales'),
    [string]$BaseLocale = 'en-US.lang',
    [switch]$Strict
)

$ErrorActionPreference = 'Stop'

function Read-LocaleKeys {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $keys = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        $trimmed = $line.Trim()
        if ($trimmed.Length -eq 0 -or $trimmed.StartsWith('#') -or $trimmed.StartsWith('//')) {
            continue
        }

        $parts = $trimmed.Split('=', 2)
        if ($parts.Length -ne 2) {
            continue
        }

        $key = $parts[0].Trim()
        if ($key.Length -gt 0) {
            [void]$keys.Add($key)
        }
    }

    return $keys
}

function Test-TechnicalLiteral {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Text
    )

    if ([string]::IsNullOrWhiteSpace($Text)) {
        return $true
    }

    if ($Text -match '^[\s\p{P}\p{S}\d]+$') {
        return $true
    }

    if ($Text -match '^(?i:https?://|tcp://|socks5://|images/|[A-Za-z]:[\\/])') {
        return $true
    }

    if ($Text -match '^(?i:release|preview|beta|version|folder|package|native|hot-inject|preload-native|survival|market)$') {
        return $true
    }

    return $false
}

function Add-Candidate {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[object]]$Candidates,
        [Parameter(Mandatory = $true)]
        [string]$Path,
        [Parameter(Mandatory = $true)]
        [int]$LineNumber,
        [Parameter(Mandatory = $true)]
        [string]$Kind,
        [Parameter(Mandatory = $true)]
        [string]$Text
    )

    $candidateId = "{0}`0{1}`0{2}" -f $Path, $LineNumber, $Text
    if (-not $script:i18nCandidateIds.Add($candidateId)) {
        return
    }

    if (-not (Test-TechnicalLiteral -Text $Text)) {
        $Candidates.Add([pscustomobject]@{
                Path = $Path
                LineNumber = $LineNumber
                Kind = $Kind
                Text = $Text
            })
    }
}

$resolvedUiDir = (Resolve-Path -LiteralPath $UiDir).Path
$resolvedLocalesDir = (Resolve-Path -LiteralPath $LocalesDir).Path
$baseLocalePath = Join-Path $resolvedLocalesDir $BaseLocale
if (-not (Test-Path -LiteralPath $baseLocalePath -PathType Leaf)) {
    throw "Base locale file not found: $baseLocalePath"
}

$localeKeys = Read-LocaleKeys -Path $baseLocalePath
$sourceFiles = Get-ChildItem -LiteralPath $resolvedUiDir -Recurse -Filter '*.rs' -File |
    Sort-Object FullName
$candidates = [System.Collections.Generic.List[object]]::new()
$script:i18nCandidateIds = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
$missingKeys = [System.Collections.Generic.List[object]]::new()

foreach ($file in $sourceFiles) {
    $lineNumber = 0
    foreach ($line in Get-Content -LiteralPath $file.FullName -Encoding UTF8) {
        $lineNumber++
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('//') -or $trimmed.StartsWith('*')) {
            continue
        }

        foreach ($match in [regex]::Matches($line, '\.(?<kind>child|label|placeholder|tooltip|title|description|header)\(\s*"(?<text>[^"]+)"')) {
            Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
                -Kind $match.Groups['kind'].Value -Text $match.Groups['text'].Value
        }

        foreach ($match in [regex]::Matches($line, '(?<kind>set_placeholder|set_text)\(\s*(?:SharedString::from\(\s*)?"(?<text>[^"]+)"')) {
            Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
                -Kind $match.Groups['kind'].Value -Text $match.Groups['text'].Value
        }

        # Catch Chinese text even when it is hidden behind a helper or a
        # multiline/indirect UI call. This is deliberately a finding rather
        # than an automatic rewrite: user content and protocol data need a
        # human decision, while built-in UI copy must use a locale key.
        foreach ($match in [regex]::Matches($line, '"(?<text>(?:\\.|[^"\\])*)"')) {
            $text = $match.Groups['text'].Value
            $isPresentationLine = $line -match '\.(child|label|placeholder|tooltip|title|description|header|set_title|prompt)\(' -or
                $line -match '(set_placeholder|set_text|toast::|open_confirm|push_async)'
            if ($isPresentationLine -and
                $text -match '\p{IsCJKUnifiedIdeographs}' -and
                $text -notmatch '^https?://' -and
                $line -notmatch '\.(t|t_args)\(') {
                Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
                    -Kind 'CJK literal' -Text $text
            }
        }

        $hasUiSink = $line -match '\.(child|label|placeholder|tooltip|title|description|header)\(' -or
            $line -match '(set_placeholder|set_text)\('
        if ($hasUiSink) {
            foreach ($match in [regex]::Matches($line, 'SharedString::from\(\s*"(?<text>[^"]+)"')) {
                Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
                    -Kind 'SharedString::from' -Text $match.Groups['text'].Value
            }

            foreach ($match in [regex]::Matches($line, 'format!\(\s*"(?<text>[^"]*\s[^"]*)"')) {
                Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
                    -Kind 'format!' -Text $match.Groups['text'].Value
            }
        }

        foreach ($match in [regex]::Matches($line, '\.(?<kind>t|t_args)\(\s*"(?<key>[^"]+)"')) {
            $key = $match.Groups['key'].Value
            if (-not $localeKeys.Contains($key)) {
                $missingKeys.Add([pscustomobject]@{
                        Path = $file.FullName
                        LineNumber = $lineNumber
                        Key = $key
                    })
            }
        }
    }

    $source = Get-Content -LiteralPath $file.FullName -Raw -Encoding UTF8
    $stringMatches = [regex]::Matches(
        $source,
        '"(?<text>(?:\\.|[^"\\\r\n])*)"'
    )
    foreach ($match in $stringMatches) {
        $text = $match.Groups['text'].Value
        if ($text -notmatch '\p{IsCJKUnifiedIdeographs}') {
            continue
        }

        $contextStart = [Math]::Max(0, $match.Index - 100)
        $contextLength = [Math]::Min(200, $source.Length - $contextStart)
        $context = $source.Substring($contextStart, $contextLength)
        if ($context -match 'i18n\s*:\s*raw') {
            continue
        }
        $isPresentationContext = $context -match '\.(child|label|placeholder|tooltip|title|description|header|set_title|prompt)\(' -or
            $context -match '(set_placeholder|set_text|toast::|open_confirm|push_async|format!\()'
        if (-not $isPresentationContext) {
            continue
        }

        $lineNumber = ($source.Substring(0, $match.Index) -split "`n").Count
        Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
            -Kind 'multiline/raw CJK literal' -Text $text
    }

    foreach ($match in [regex]::Matches(
        $source,
        'r(?<hashes>#{0,8})"(?<text>.*?)"\k<hashes>',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )) {
        $text = $match.Groups['text'].Value
        if ($text -notmatch '\p{IsCJKUnifiedIdeographs}') {
            continue
        }

        $contextStart = [Math]::Max(0, $match.Index - 100)
        $contextLength = [Math]::Min(200, $source.Length - $contextStart)
        $context = $source.Substring($contextStart, $contextLength)
        if ($context -match 'i18n\s*:\s*raw') {
            continue
        }
        $isPresentationContext = $context -match '\.(child|label|placeholder|tooltip|title|description|header|set_title|prompt)\(' -or
            $context -match '(set_placeholder|set_text|toast::|open_confirm|push_async|format!\()'
        if (-not $isPresentationContext) {
            continue
        }

        $lineNumber = ($source.Substring(0, $match.Index) -split "`n").Count
        Add-Candidate -Candidates $candidates -Path $file.FullName -LineNumber $lineNumber `
            -Kind 'raw CJK literal' -Text $text
    }

    foreach ($match in [regex]::Matches(
        $source,
        '\.(?<kind>t|t_args)\(\s*"(?<key>[^"]+)"',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )) {
        $key = $match.Groups['key'].Value
        if (-not $localeKeys.Contains($key)) {
            $lineNumber = ($source.Substring(0, $match.Index) -split "`n").Count
            $missingKeys.Add([pscustomobject]@{
                    Path = $file.FullName
                    LineNumber = $lineNumber
                    Key = $key
            })
        }
    }

    foreach ($match in [regex]::Matches(
        $source,
        '\bt!\(\s*"(?<key>[^"]+)"',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )) {
        $key = $match.Groups['key'].Value
        if (-not $localeKeys.Contains($key)) {
            $lineNumber = ($source.Substring(0, $match.Index) -split "`n").Count
            $missingKeys.Add([pscustomobject]@{
                    Path = $file.FullName
                    LineNumber = $lineNumber
                    Key = $key
                })
        }
    }

    foreach ($match in [regex]::Matches(
        $source,
        'crate::localized_text!\(\s*"(?<key>[^"]+)"',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )) {
        $key = $match.Groups['key'].Value
        if (-not $localeKeys.Contains($key)) {
            $lineNumber = ($source.Substring(0, $match.Index) -split "`n").Count
            $missingKeys.Add([pscustomobject]@{
                    Path = $file.FullName
                    LineNumber = $lineNumber
                    Key = $key
                })
        }
    }
}

if ($missingKeys.Count -gt 0) {
    Write-Host "Missing locale keys ($($missingKeys.Count)):" -ForegroundColor Red
    foreach ($missing in $missingKeys) {
        Write-Host ("  {0}:{1}: {2}" -f $missing.Path, $missing.LineNumber, $missing.Key)
    }
}
else {
    Write-Host 'No missing static locale keys found.' -ForegroundColor Green
}

if ($candidates.Count -gt 0) {
    Write-Host "Hard-coded UI string candidates ($($candidates.Count)):" -ForegroundColor Yellow
    foreach ($candidate in $candidates) {
        Write-Host ("  {0}:{1}: [{2}] {3}" -f $candidate.Path, $candidate.LineNumber, $candidate.Kind, $candidate.Text)
    }
}
else {
    Write-Host 'No hard-coded UI string candidates found.' -ForegroundColor Green
}

if ($Strict -and ($missingKeys.Count -gt 0 -or $candidates.Count -gt 0)) {
    throw 'UI localization findings detected.'
}

# Documentation guard for BMCBL.
# Verifies: bilingual crate READMEs, relative links, forbidden transient docs,
# and the stable docs layout.
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$failures = [System.Collections.Generic.List[string]]::new()

# 1. Forbidden transient/history docs.
$forbiddenPatterns = @(
    "docs/naming-audit.md",
    "docs/**/*naming-audit*",
    "docs/**/*rename*",
    "docs/**/*migration*",
    "docs/**/*todo*",
    "docs/validation/**",
    "docs/superpowers/**",
    "docs/**/20??-??-??*.md"
)
foreach ($pattern in $forbiddenPatterns) {
    Get-ChildItem -Path (Join-Path $root "docs") -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like (Join-Path $root $pattern) } |
        ForEach-Object {
            $failures.Add("$($_.FullName): forbidden documentation type")
        }
}
if (Test-Path (Join-Path $root "docs/naming-audit.md")) {
    $failures.Add("docs/naming-audit.md: rename-history docs are not allowed")
}

# 2. Every workspace library crate has English and Chinese READMEs.
$firstPartyDirs = @(
    "crates/bedrock-block-model", "crates/bedrock-brarchive", "crates/bedrock-leveldb",
    "crates/bedrock-render", "crates/bedrock-voxel", "crates/bedrock-world",
    "crates/bmcbl-plugin-api", "crates/bmcbl-plugin-macros",
    "crates/egpui", "crates/egpui-build", "crates/egpui-manifest",
    "crates/gpui", "crates/gpui-3d", "crates/gpui-hooks", "crates/gpui-hooks-macros",
    "crates/gpui-macros", "crates/gpui_tokio", "crates/lucide-gpui",
    "crates/easytier-bmcbl", "crates/nethernet"
)
Get-ChildItem (Join-Path $root "crates/nova-gfx") -Directory |
    Where-Object { $_.Name -ne "examples" } |
    ForEach-Object { $firstPartyDirs += $_.FullName.Substring($root.Length + 1).Replace("\", "/") }

foreach ($dir in $firstPartyDirs) {
    $base = Join-Path $root $dir
    if (-not (Test-Path $base)) { continue }
    foreach ($name in @("README.md", "README.zh-CN.md")) {
        if (-not (Test-Path (Join-Path $base $name))) {
            $failures.Add("${dir}/${name}: missing bilingual crate README")
        }
    }
}

# 3. Relative markdown links resolve.
$markdownFiles = Get-ChildItem $root -Recurse -File -Filter *.md | Where-Object {
    $_.FullName -notmatch "\\target\\" -and
    $_.FullName -notmatch "\\vendor\\" -and
    $_.FullName -notmatch "\\crates\\easytier\\" -and
    $_.FullName -notmatch "\\crates\\raknet\\"
}
$linkPattern = '\[[^\]]*\]\(([^)\s]+)\)'
foreach ($file in $markdownFiles) {
    $lineNumber = 0
    foreach ($line in Get-Content $file.FullName) {
        $lineNumber++
        foreach ($match in [regex]::Matches($line, $linkPattern)) {
            $target = $match.Groups[1].Value
            if ($target -match "^(https?:|mailto:|#)") { continue }
            $pathPart = ($target -split "#", 2)[0]
            if ([string]::IsNullOrWhiteSpace($pathPart)) { continue }
            $resolved = Join-Path $file.DirectoryName $pathPart
            if (-not (Test-Path $resolved)) {
                $failures.Add("$($file.FullName):${lineNumber}: broken link '$target'")
            }
        }
    }
}

# 4. Stable docs index exists.
foreach ($required in @("docs/README.md", "docs/README.en.md", "docs/ARCHITECTURE.md",
        "docs/ARCHITECTURE.en.md", "docs/BUILD_AND_TEST.md", "docs/BUILD_AND_TEST.en.md",
        "docs/CONTRIBUTING.md", "docs/CONTRIBUTING.en.md", "docs/CRATES.md",
        "docs/CRATES.en.md", "docs/NAMING.md", "docs/adr/0001-nova-gfx-gpui-boundary.md")) {
    if (-not (Test-Path (Join-Path $root $required))) {
        $failures.Add("${required}: required documentation is missing")
    }
}

if ($failures.Count -gt 0) {
    Write-Host "Documentation violations:" -ForegroundColor Red
    $failures | Select-Object -First 500 | ForEach-Object { Write-Host "  $_" }
    
    exit 1
}
Write-Host "Documentation check passed." -ForegroundColor Green

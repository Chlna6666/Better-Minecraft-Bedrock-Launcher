$ErrorActionPreference = "Stop"

$llvmBin = Join-Path $env:ProgramFiles "LLVM\bin"
$libclang = @(
    (Join-Path $llvmBin "libclang.dll"),
    (Join-Path $llvmBin "clang.dll")
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $libclang) {
    choco install llvm -y --no-progress
}
$libclang = @(
    (Join-Path $llvmBin "libclang.dll"),
    (Join-Path $llvmBin "clang.dll")
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $libclang) {
    throw "libclang.dll or clang.dll was not found under $llvmBin"
}
Add-Content -Path $env:GITHUB_PATH -Value $llvmBin
Add-Content -Path $env:GITHUB_ENV -Value "LIBCLANG_PATH=$llvmBin"

if (-not (Get-Command 7z -ErrorAction SilentlyContinue)) {
    $sevenZipDir = Join-Path $env:ProgramFiles "7-Zip"
    if (-not (Test-Path (Join-Path $sevenZipDir "7z.exe"))) {
        choco install 7zip -y --no-progress
    }
    Add-Content -Path $env:GITHUB_PATH -Value $sevenZipDir
    $env:PATH = "$sevenZipDir;$env:PATH"
}

$cacheRoot = Join-Path $env:RUNNER_TEMP "thunk-cache"
$vcRoot = Join-Path $cacheRoot "VC-LTL-5.2.2"
$yyRoot = Join-Path $cacheRoot "YY-Thunks-1.1.7"
New-Item -ItemType Directory -Force -Path $cacheRoot | Out-Null

if (-not (Test-Path (Join-Path $vcRoot "TargetPlatform"))) {
    $archive = Join-Path $cacheRoot "VC-LTL-Binary.7z"
    curl.exe -L --retry 8 --retry-all-errors --retry-delay 2 `
        -o $archive `
        "https://github.com/Chuyu-Team/VC-LTL5/releases/download/v5.2.2/VC-LTL-Binary.7z"
    & 7z x -aoa $archive "-o$vcRoot" | Out-Null
}
$vcPath = $vcRoot
if (-not (Test-Path (Join-Path $vcPath "TargetPlatform"))) {
    $child = Get-ChildItem -Directory $vcRoot -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($child -and (Test-Path (Join-Path $child.FullName "TargetPlatform"))) {
        $vcPath = $child.FullName
    }
}

if (-not (Test-Path (Join-Path $yyRoot "objs"))) {
    $archive = Join-Path $cacheRoot "YY-Thunks-Objs.zip"
    curl.exe -L --retry 8 --retry-all-errors --retry-delay 2 `
        -o $archive `
        "https://github.com/Chuyu-Team/YY-Thunks/releases/download/v1.1.7/YY-Thunks-Objs.zip"
    & 7z x -aoa $archive "-o$yyRoot" | Out-Null
}
$yyPath = $yyRoot
if (-not (Test-Path (Join-Path $yyPath "objs"))) {
    $child = Get-ChildItem -Directory $yyRoot -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($child -and (Test-Path (Join-Path $child.FullName "objs"))) {
        $yyPath = $child.FullName
    }
}

if (-not (Test-Path (Join-Path $vcPath "TargetPlatform"))) {
    throw "VC_LTL path invalid: $vcPath"
}
if (-not (Test-Path (Join-Path $yyPath "objs"))) {
    throw "YY_THUNKS path invalid: $yyPath"
}
Add-Content -Path $env:GITHUB_ENV -Value "VC_LTL=$vcPath"
Add-Content -Path $env:GITHUB_ENV -Value "YY_THUNKS=$yyPath"

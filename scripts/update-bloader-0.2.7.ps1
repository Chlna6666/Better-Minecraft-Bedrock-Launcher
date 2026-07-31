[CmdletBinding()]
param(
    [string]$RepositoryRoot = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Version = '0.2.7'
$ArchiveName = "BLoader-$Version-windows-x64.zip"
$ReleaseBase = "https://github.com/Chlna6666/BLoader/releases/download/v$Version"
$ExpectedDllSha256 = 'de046e7ef2518856dbd04ca8786b2234c593aa2c51a8a76913270afff8257344'
$ExpectedDllSize = 1344000
$Destination = Join-Path $RepositoryRoot 'assets\bin\BLoader.dll'
$VersionFile = Join-Path $RepositoryRoot 'assets\bin\BLoader.version'

$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("bmcbl-bloader-$Version-" + [Guid]::NewGuid().ToString('N'))
$ArchivePath = Join-Path $TempRoot $ArchiveName
$ArchiveHashPath = "$ArchivePath.sha256"
$ExtractPath = Join-Path $TempRoot 'release'

try {
    New-Item -ItemType Directory -Path $TempRoot -Force | Out-Null

    Write-Host "Downloading BLoader $Version release..."
    Invoke-WebRequest -Uri "$ReleaseBase/$ArchiveName" -OutFile $ArchivePath
    Invoke-WebRequest -Uri "$ReleaseBase/$ArchiveName.sha256" -OutFile $ArchiveHashPath

    $ExpectedArchiveHash = ((Get-Content $ArchiveHashPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    $ActualArchiveHash = (Get-FileHash $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualArchiveHash -ne $ExpectedArchiveHash) {
        throw "BLoader release archive SHA-256 mismatch: expected=$ExpectedArchiveHash actual=$ActualArchiveHash"
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $ExtractPath -Force
    $SourceDll = Join-Path $ExtractPath 'BLoader.dll'
    if (-not (Test-Path $SourceDll -PathType Leaf)) {
        throw "BLoader.dll was not found in the verified release archive"
    }

    $ActualDllHash = (Get-FileHash $SourceDll -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualDllHash -ne $ExpectedDllSha256) {
        throw "BLoader.dll SHA-256 mismatch: expected=$ExpectedDllSha256 actual=$ActualDllHash"
    }

    $ActualDllSize = (Get-Item $SourceDll).Length
    if ($ActualDllSize -ne $ExpectedDllSize) {
        throw "BLoader.dll size mismatch: expected=$ExpectedDllSize actual=$ActualDllSize"
    }

    New-Item -ItemType Directory -Path (Split-Path -Parent $Destination) -Force | Out-Null
    Copy-Item $SourceDll $Destination -Force

    @(
        "version=$Version"
        "sha256=$ExpectedDllSha256"
        "size=$ExpectedDllSize"
        "source=https://github.com/Chlna6666/BLoader/releases/tag/v$Version"
        'features=xuser-bridge,secure-pipe,query-api-impl-hook,pop-signature,user-switching'
        'scope=win32-gdk-only'
    ) | Set-Content -Path $VersionFile -Encoding UTF8

    Write-Host "Updated: $Destination"
    Write-Host "BLoader version: $Version"
    Write-Host "SHA-256: $ActualDllHash"
}
finally {
    Remove-Item $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
}

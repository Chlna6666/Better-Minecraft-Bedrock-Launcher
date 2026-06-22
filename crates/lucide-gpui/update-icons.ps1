param(
  [string] $Ref = "main",
  [switch] $Prune
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\\..")).Path
$tmpRoot = Join-Path $repoRoot ".tmp"
$upstreamDir = Join-Path $tmpRoot "lucide-upstream"
$targetDir = Join-Path $repoRoot "crates\\lucide-gpui\\icons"

if (!(Test-Path $tmpRoot)) {
  New-Item -ItemType Directory -Force $tmpRoot | Out-Null
}

if (!(Test-Path $upstreamDir)) {
  git clone --depth 1 https://github.com/lucide-icons/lucide $upstreamDir | Out-Null
} else {
  git -C $upstreamDir fetch --depth 1 origin $Ref | Out-Null
}

git -C $upstreamDir checkout --force $Ref | Out-Null

if ($Prune) {
  if (Test-Path $targetDir) {
    Remove-Item -Recurse -Force (Join-Path $targetDir "*.svg")
  }
}

if (!(Test-Path $targetDir)) {
  New-Item -ItemType Directory -Force $targetDir | Out-Null
}

Copy-Item (Join-Path $upstreamDir "icons\\*.svg") $targetDir -Force

Write-Host ("Synced icons to: {0}" -f $targetDir)


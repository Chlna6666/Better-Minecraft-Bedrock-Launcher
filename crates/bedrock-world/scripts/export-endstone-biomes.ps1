param(
    [string]$EndstoneRef = "main",
    [string]$ExpectedSemanticVersion,
    [string]$WorkingDirectory = (Join-Path ([IO.Path]::GetTempPath()) "bedrock-world-biome-export"),
    [string]$OutputPath = (Join-Path ([IO.Path]::GetTempPath()) "bedrock-world-biomes.json"),
    [int]$TimeoutSeconds = 240,
    [switch]$KeepWorktree
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows) {
    throw "Automatic Endstone biome export currently requires Windows because Endstone enables DevTools in its Windows package. On Linux, pass an externally generated Endstone biomes.json to update-biome-registry.ps1."
}

function Require-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command '$Name' was not found on PATH"
    }
}

function Write-Utf8NoBom {
    param([string]$Path, [string]$Text)
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function Replace-Exact {
    param([string]$Path, [string]$Old, [string]$New)
    $text = [IO.File]::ReadAllText($Path)
    $count = ([regex]::Matches($text, [regex]::Escape($Old))).Count
    if ($count -ne 1) {
        throw "Expected exactly one Endstone source anchor in '$Path', found $count. Refusing to apply a fuzzy patch."
    }
    Write-Utf8NoBom $Path ($text.Replace($Old, $New))
}

function Normalize-EndstoneMinecraftVersion {
    param([string]$Value)
    $value = $Value.Trim()
    if ($value -notmatch '^1\.') {
        $value = "1.$value"
    }
    return $value
}

Require-Command git
Require-Command python

$work = [IO.Path]::GetFullPath($WorkingDirectory)
$output = [IO.Path]::GetFullPath($OutputPath)
$sourceDir = Join-Path $work "endstone"
$venvDir = Join-Path $work "venv"
$serverDir = Join-Path $work "server"
$logDir = Join-Path $work "logs"

New-Item -ItemType Directory -Force -Path $work, $logDir | Out-Null
if (-not (Test-Path -LiteralPath (Join-Path $sourceDir ".git"))) {
    & git clone --filter=blob:none https://github.com/EndstoneMC/endstone.git $sourceDir
    if ($LASTEXITCODE -ne 0) { throw "Failed to clone Endstone" }
}

Push-Location $sourceDir
try {
    & git fetch --tags origin
    if ($LASTEXITCODE -ne 0) { throw "Failed to fetch Endstone refs" }
    & git reset --hard
    & git clean -fdx
    & git checkout --detach $EndstoneRef
    if ($LASTEXITCODE -ne 0) {
        & git checkout --detach "origin/$EndstoneRef"
        if ($LASTEXITCODE -ne 0) { throw "Unable to checkout Endstone ref '$EndstoneRef'" }
    }
    $endstoneCommit = (& git rev-parse HEAD).Trim()
}
finally {
    Pop-Location
}

$vanillaDataPath = Join-Path $sourceDir "src/endstone/core/devtools/vanilla_data.cpp"
$dedicatedServerPath = Join-Path $sourceDir "src/endstone/runtime/bedrock_hooks/dedicated_server.cpp"

$oldDumpBiomes = @'
void dumpBiomes(VanillaData &data, ::Level &level)
{
    auto &biomes = data.biomes;
    level.getBiomeRegistry().forEachBiome(
        [&biomes](const Biome &biome) { biomes[biome.getFullName()] = {{"id", biome.getId()}}; });
}
'@

$newDumpBiomes = @'
void dumpBiomes(VanillaData &data, ::Level &level)
{
    auto &biomes = data.biomes;
    level.getBiomeRegistry().forEachBiome([&biomes](const Biome &biome) {
        biomes[biome.getFullName()] = {
            {"id", biome.getId()},
            {"temperature", truncate(biome.temperature)},
            {"downfall", truncate(biome.downfall)},
            {"foliage_snow", truncate(biome.foliage_snow)},
            {"depth", truncate(biome.depth)},
            {"scale", truncate(biome.scale)},
            {"map_water_color_argb", biome.map_water_color},
            {"rain", biome.rain},
        };
    });
}
'@
Replace-Exact $vanillaDataPath $oldDumpBiomes $newDumpBiomes

$oldScheduler = @'
                    VanillaData data;
                    dumpBlockData(data, level);
                    dumpItemData(data, level);
                    dumpRecipes(data, level);
                    dumpBiomes(data, level);
                    entt::locator<VanillaData>::emplace(std::move(data));
                    ready = true;
'@
$newScheduler = @'
                    VanillaData data;
                    if (std::getenv("ENDSTONE_BIOME_EXPORT_PATH") != nullptr) {
                        dumpBiomes(data, level);
                    }
                    else {
                        dumpBlockData(data, level);
                        dumpItemData(data, level);
                        dumpRecipes(data, level);
                        dumpBiomes(data, level);
                    }
                    entt::locator<VanillaData>::emplace(std::move(data));
                    ready = true;
'@
Replace-Exact $vanillaDataPath $oldScheduler $newScheduler

$vanillaText = [IO.File]::ReadAllText($vanillaDataPath)
if (-not $vanillaText.Contains("#include <cstdlib>")) {
    Replace-Exact $vanillaDataPath "#include <unordered_map>" "#include <cstdlib>`n#include <unordered_map>"
}

$oldIncludes = @'
#include <iostream>

#include <entt/locator/locator.hpp>
'@
$newIncludes = @'
#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <thread>

#include <entt/locator/locator.hpp>
'@
Replace-Exact $dedicatedServerPath $oldIncludes $newIncludes

$oldDevtoolsInclude = @'
#include "endstone/core/devtools/devtools.h"
#include "endstone/core/logger_factory.h"
'@
$newDevtoolsInclude = @'
#include "endstone/core/devtools/devtools.h"
#ifdef ENDSTONE_WITH_DEVTOOLS
#include "endstone/core/devtools/vanilla_data.h"
#endif
#include "endstone/core/logger_factory.h"
'@
Replace-Exact $dedicatedServerPath $oldDevtoolsInclude $newDevtoolsInclude

$oldDevtoolsBlock = @'
#ifdef ENDSTONE_WITH_DEVTOOLS
    // DevTools
    std::thread thread(&endstone::core::devtools::render);
    thread.detach();
#endif
'@
$newDevtoolsBlock = @'
#ifdef ENDSTONE_WITH_DEVTOOLS
    if (const char *output_path = std::getenv("ENDSTONE_BIOME_EXPORT_PATH");
        output_path != nullptr && *output_path != '\0') {
        std::thread([output = std::filesystem::path(output_path)] {
            using namespace std::chrono_literals;
            for (std::size_t attempt = 0; attempt < 2400; ++attempt) {
                if (auto *data = endstone::core::devtools::VanillaData::get(); data != nullptr) {
                    if (const auto parent = output.parent_path(); !parent.empty()) {
                        std::filesystem::create_directories(parent);
                    }
                    auto temporary = output;
                    temporary += ".tmp";
                    {
                        std::ofstream file(temporary, std::ios::out | std::ios::trunc);
                        file << data->biomes.dump(2);
                        file.flush();
                        if (!file) {
                            return;
                        }
                    }
                    std::error_code error;
                    std::filesystem::remove(output, error);
                    error.clear();
                    std::filesystem::rename(temporary, output, error);
                    return;
                }
                std::this_thread::sleep_for(100ms);
            }
        }).detach();
    }
    else {
        std::thread thread(&endstone::core::devtools::render);
        thread.detach();
    }
#endif
'@
Replace-Exact $dedicatedServerPath $oldDevtoolsBlock $newDevtoolsBlock

if (-not (Test-Path -LiteralPath (Join-Path $venvDir "Scripts/python.exe"))) {
    & python -m venv $venvDir
    if ($LASTEXITCODE -ne 0) { throw "Failed to create Endstone exporter virtual environment" }
}

$venvPython = Join-Path $venvDir "Scripts/python.exe"
$endstoneExe = Join-Path $venvDir "Scripts/endstone.exe"

& $venvPython -m pip install --disable-pip-version-check --upgrade pip
if ($LASTEXITCODE -ne 0) { throw "Failed to update pip in exporter environment" }
& $venvPython -m pip install --disable-pip-version-check --force-reinstall $sourceDir
if ($LASTEXITCODE -ne 0) { throw "Failed to build/install patched Endstone exporter" }

$endstoneMinecraftVersion = Normalize-EndstoneMinecraftVersion (
    & $venvPython -c "import endstone; print(endstone.__minecraft_version__)"
)
if ($LASTEXITCODE -ne 0) { throw "Failed to read Endstone target Minecraft version" }
if ($ExpectedSemanticVersion -and $endstoneMinecraftVersion -ne $ExpectedSemanticVersion) {
    throw "Endstone ref '$EndstoneRef' targets Minecraft $endstoneMinecraftVersion, expected $ExpectedSemanticVersion"
}

New-Item -ItemType Directory -Force -Path $serverDir | Out-Null
if (Test-Path -LiteralPath $output) { Remove-Item -Force -LiteralPath $output }
if (Test-Path -LiteralPath "$output.tmp") { Remove-Item -Force -LiteralPath "$output.tmp" }

$oldExportPath = $env:ENDSTONE_BIOME_EXPORT_PATH
$env:ENDSTONE_BIOME_EXPORT_PATH = $output
$stdoutPath = Join-Path $logDir "endstone.stdout.log"
$stderrPath = Join-Path $logDir "endstone.stderr.log"
$process = $null

try {
    $process = Start-Process `
        -FilePath $endstoneExe `
        -ArgumentList @("--server-folder", $serverDir, "--no-confirm", "--no-interactive") `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $output) {
            $info = Get-Item -LiteralPath $output
            if ($info.Length -gt 2) { break }
        }
        if ($process.HasExited) {
            throw "Endstone exited before biome export completed. See '$stdoutPath' and '$stderrPath'."
        }
        Start-Sleep -Milliseconds 250
    }

    if (-not (Test-Path -LiteralPath $output)) {
        throw "Timed out waiting for Endstone biome export. See '$stdoutPath' and '$stderrPath'."
    }
}
finally {
    if ($process -and -not $process.HasExited) {
        & taskkill /PID $process.Id /T /F 2>$null | Out-Null
    }
    $env:ENDSTONE_BIOME_EXPORT_PATH = $oldExportPath
}

$exported = Get-Content -LiteralPath $output -Raw | ConvertFrom-Json -AsHashtable -Depth 100
if ($exported -isnot [System.Collections.IDictionary] -or $exported.Count -eq 0) {
    throw "Endstone exporter produced an empty or invalid biome object"
}

$sidecar = [ordered]@{
    endstone_repository = "EndstoneMC/endstone"
    endstone_ref = $EndstoneRef
    endstone_commit = $endstoneCommit
    endstone_minecraft_version = $endstoneMinecraftVersion
}
$sidecarPath = "$output.source.json"
Write-Utf8NoBom $sidecarPath (($sidecar | ConvertTo-Json -Depth 10) + [Environment]::NewLine)

Write-Host "Endstone biome export completed:"
Write-Host "  Endstone:  $EndstoneRef @ $endstoneCommit"
Write-Host "  Minecraft: $endstoneMinecraftVersion"
Write-Host "  Biomes:    $($exported.Count)"
Write-Host "  Output:    $output"
Write-Host "  Source:    $sidecarPath"

if (-not $KeepWorktree) {
    # The next invocation resets the source checkout before patching. Keeping the venv/server cache
    # dramatically reduces subsequent refresh time without making generated data depend on it.
}

param(
    [ValidateSet("release", "preview")]
    [string]$Channel = "release",

    [string]$Version = "latest",
    [string]$BiomesJson,
    [string]$EndstoneRef = "main",
    [string]$ProtocolDocsRef,
    [string]$WorkingDirectory = (Join-Path ([IO.Path]::GetTempPath()) "bedrock-world-biome-update")
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$bedrockServerDataBase = "https://raw.githubusercontent.com/EndstoneMC/bedrock-server-data/v2"
$protocolDocsRawBase = "https://raw.githubusercontent.com/EndstoneMC/protocol-docs"
$githubHeaders = @{
    "User-Agent" = "bedrock-world-biome-registry-updater"
    "Accept" = "application/vnd.github+json"
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

function Get-BedrockServerMetadata {
    param([string]$RequestedChannel, [string]$RequestedVersion, [string]$Work)

    $versions = Invoke-RestMethod -Uri "$bedrockServerDataBase/versions.json" -Headers $githubHeaders
    $channelData = $versions.$RequestedChannel
    if ($null -eq $channelData) {
        throw "bedrock-server-data does not contain channel '$RequestedChannel'"
    }

    $semanticVersion = if ($RequestedVersion -eq "latest") {
        [string]$channelData.latest
    }
    else {
        $RequestedVersion
    }
    if ($semanticVersion -notin @($channelData.versions)) {
        throw "bedrock-server-data channel '$RequestedChannel' does not contain version '$semanticVersion'"
    }

    $metadataUri = "$bedrockServerDataBase/$RequestedChannel/$semanticVersion/metadata.json"
    $metadataText = (Invoke-WebRequest -Uri $metadataUri -Headers $githubHeaders -UseBasicParsing).Content
    $metadata = $metadataText | ConvertFrom-Json
    $binaryUrl = if ($IsWindows) {
        [string]$metadata.binary.windows.url
    }
    else {
        [string]$metadata.binary.linux.url
    }
    if ($binaryUrl -notmatch 'bedrock-server-(?<build>\d+\.\d+\.\d+\.\d+)\.zip$') {
        throw "Unable to extract exact BDS build from '$binaryUrl'"
    }

    $metadataPath = Join-Path $Work "bds-metadata.json"
    Write-Utf8NoBom $metadataPath ($metadataText.TrimEnd() + [Environment]::NewLine)
    return [ordered]@{
        semantic_version = $semanticVersion
        build_version = $Matches.build
        metadata_path = $metadataPath
    }
}

function Get-ProtocolDocsMetadata {
    param([string]$MinecraftBuild, [string]$RequestedRef, [string]$Work)

    $candidateRefs = @()
    if ($RequestedRef) {
        $candidateRefs = @($RequestedRef)
    }
    else {
        $branches = Invoke-RestMethod `
            -Uri "https://api.github.com/repos/EndstoneMC/protocol-docs/branches?per_page=100" `
            -Headers $githubHeaders
        $candidateRefs = @(
            $branches |
                ForEach-Object { [string]$_.name } |
                Where-Object { $_ -match '^r\d+_u\d+$' } |
                Sort-Object {
                    if ($_ -match '^r(?<release>\d+)_u(?<update>\d+)$') {
                        ([int]$Matches.release * 10000) + [int]$Matches.update
                    }
                    else { 0 }
                } -Descending
        )
    }

    foreach ($ref in $candidateRefs) {
        try {
            $readme = (Invoke-WebRequest -Uri "$protocolDocsRawBase/$ref/README.md" -Headers $githubHeaders -UseBasicParsing).Content
        }
        catch {
            if ($RequestedRef) { throw }
            continue
        }

        if ($readme -notmatch '(?m)^- \*\*Minecraft Version:\*\* (?<version>[0-9.]+)(?: \([^)]+\))?\s*$') {
            if ($RequestedRef) {
                throw "protocol-docs '$ref' README has no recognized Minecraft Version line"
            }
            continue
        }
        $documentedVersion = $Matches.version
        if ($documentedVersion -ne $MinecraftBuild) {
            if ($RequestedRef) {
                throw "protocol-docs '$ref' describes Minecraft $documentedVersion, expected $MinecraftBuild"
            }
            continue
        }
        if ($readme -notmatch '(?m)^- \*\*Network Version:\*\* (?<network>[0-9]+)\s*$') {
            throw "protocol-docs '$ref' README has no recognized Network Version line"
        }
        $networkVersion = [uint32]$Matches.network

        $schema = Invoke-RestMethod `
            -Uri "$protocolDocsRawBase/$ref/types/BiomeDefinitionData.json" `
            -Headers $githubHeaders
        $idField = @($schema.fields | Where-Object { [string]$_.name -eq "id" })
        if ($idField.Count -ne 1 -or [string]$idField[0].type -ne "uint16") {
            throw "protocol-docs '$ref' no longer describes BiomeDefinitionData.id as exactly one uint16 field"
        }

        $readmePath = Join-Path $Work "protocol-docs-README.md"
        Write-Utf8NoBom $readmePath ($readme.TrimEnd() + [Environment]::NewLine)
        return [ordered]@{
            ref = $ref
            network_version = $networkVersion
            readme_path = $readmePath
        }
    }

    throw "No protocol-docs branch exactly matches BDS build $MinecraftBuild. Refusing to guess a network version."
}

Require-Command cargo
New-Item -ItemType Directory -Force -Path $WorkingDirectory | Out-Null
$work = [IO.Path]::GetFullPath($WorkingDirectory)
$server = Get-BedrockServerMetadata $Channel $Version $work
$protocol = Get-ProtocolDocsMetadata $server.build_version $ProtocolDocsRef $work

$biomePath = $BiomesJson
$endstoneCommit = $null
if (-not $biomePath) {
    if (-not $IsWindows) {
        throw "Automatic Endstone runtime export is currently Windows-only. Pass -BiomesJson with a real runtime export on this platform."
    }
    $biomePath = Join-Path $work "biomes.json"
    $exportScript = Join-Path $PSScriptRoot "export-endstone-biomes.ps1"
    & $exportScript `
        -EndstoneRef $EndstoneRef `
        -ExpectedSemanticVersion $server.semantic_version `
        -WorkingDirectory (Join-Path $work "endstone-export") `
        -OutputPath $biomePath
    $sourcePath = "$biomePath.source.json"
    if (-not (Test-Path -LiteralPath $sourcePath)) {
        throw "Endstone exporter did not write source metadata '$sourcePath'"
    }
    $source = Get-Content -LiteralPath $sourcePath -Raw | ConvertFrom-Json -AsHashtable
    if ($source.ContainsKey("endstone_commit")) {
        $endstoneCommit = [string]$source["endstone_commit"]
    }
}
else {
    $biomePath = (Resolve-Path -LiteralPath $biomePath).Path
}

$resolvedEndstoneRef = if ($endstoneCommit) { $endstoneCommit } else { $EndstoneRef }
$crateManifest = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot "../Cargo.toml"))
$toolArgs = @(
    "run",
    "--manifest-path", $crateManifest,
    "--no-default-features",
    "--bin", "bedrock-world-tool",
    "--",
    "biome", "update",
    "--input", $biomePath,
    "--minecraft-version", [string]$server.build_version,
    "--network-version", [string]$protocol.network_version,
    "--bds-metadata", [string]$server.metadata_path,
    "--protocol-readme", [string]$protocol.readme_path,
    "--channel", $Channel,
    "--endstone-ref", $resolvedEndstoneRef,
    "--protocol-ref", [string]$protocol.ref
)

& cargo @toolArgs
if ($LASTEXITCODE -ne 0) {
    throw "bedrock-world-tool failed to update the biome registry"
}

& cargo run `
    --manifest-path $crateManifest `
    --no-default-features `
    --bin bedrock-world-tool `
    -- biome verify
if ($LASTEXITCODE -ne 0) {
    throw "bedrock-world-tool failed to verify the generated registry"
}

Write-Host ""
Write-Host "Biome registry update completed:"
Write-Host "  channel:          $Channel"
Write-Host "  requested:        $Version"
Write-Host "  BDS semantic:     $($server.semantic_version)"
Write-Host "  exact BDS build:  $($server.build_version)"
Write-Host "  network version:  $($protocol.network_version)"
Write-Host "  protocol-docs:    $($protocol.ref)"
Write-Host "  Endstone source:  $resolvedEndstoneRef"
Write-Host "  biome source:     $biomePath"

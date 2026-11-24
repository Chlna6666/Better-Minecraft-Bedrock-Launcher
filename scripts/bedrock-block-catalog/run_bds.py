#!/usr/bin/env python3
"""Run a disposable official Linux BDS and export its script-generated structures.

Only the official Mojang download API and CDN are accepted. Server binaries,
worlds and player information are never committed to the repository.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import queue
import re
import shutil
import stat
import subprocess
import sys
import threading
import time
import urllib.parse
import urllib.request
import zipfile

LINKS_API = "https://net-secondary.web.minecraft-services.net/api/v1.0/download/links"
PACK_ID = "eba1036d-209a-4e49-af43-1510862d8ad1"
WORLD_NAME = "bmcbl_catalog"
# Historical acquisition is capability-based, not PDB-based.
# SHA-1 references are from Bedrock-OSS/BDS-Versions. We still compute
# SHA-256 for every downloaded archive. Download bytes ONLY from Mojang.
# To process a new historical build, add its verified exact archive digest.
HISTORICAL_SHA1 = {
    "1.19.70.02": "f56479e8e0eef4157ee1cc1f818f75061e6241c4",
    "1.19.80.02": "d18271ed0a03818dc0fa12e8ef4eddb6d8eee9cf",
    "1.20.0.01": "e26c25d0098485bf5691ee026501481ca0f13fae",
    "1.20.10.01": "c5d3e1f16cfc5adb203fbcdfe6c3aa2e4a6b4234",
    "1.20.50.03": "6081ffa46c77c73179766374dc7a551eb14abdf3",
    "1.20.80.05": "980fcfac32265d56f2c6ec4362a6fe344cd1b030",
    "1.21.0.03": "f74cd64c14bc1d850350626bd26755305cb42e46",
    "1.21.20.03": "06be13f05da2fc05e93ab2d5cac3df5f51490691",
    "1.21.30.03": "f49fdd1cf5d1475100ce3038d05dbe4c99229ad6",
    "1.21.50.10": "e2849cdcfb0172712bfffbe36a23f06b02a838a6",
    "1.21.60.10": "4b8236e92dc4e2c6ea2d251549c6ca46ad3f7e54",
    "1.21.70.04": "296aab6abc7c43860dfc20390bd3bcc8efa7239a",
    "1.21.80.3": "30ec90fbde8054d8b8e3bfbdb3c5edaa8dccbd2b",
    "1.21.100.7": "c68f38ff0c75110b086f629a30fd95b86fbfc2df",
    "1.21.130.4": "9f7cdf8bbeabe4b92efb4229f2eb6017e86a4a93",
}
# Exact major/minor Script API track by Minecraft release. Each profile is
# *tested* against BDS at runtime, not assumed fully capable of Structure I/O.
API_PROFILES = (
    ((1, 19, 70), "1.0.0"),
    ((1, 19, 80), "1.1.0"),
    ((1, 20, 0), "1.2.0"),
    ((1, 20, 10), "1.3.0"),
    ((1, 20, 20), "1.4.0"),
    ((1, 20, 30), "1.5.0"),
    ((1, 20, 40), "1.6.0"),
    ((1, 20, 50), "1.7.0"),
    ((1, 20, 60), "1.8.0"),
    ((1, 20, 70), "1.9.0"),
    ((1, 20, 80), "1.10.0"),
    ((1, 21, 0), "1.11.0"),
    ((1, 21, 10), "1.12.0"),
    ((1, 21, 20), "1.13.0"),
    ((1, 21, 30), "1.14.0"),
    ((1, 21, 40), "1.15.0"),
    ((1, 21, 50), "1.16.0"),
    ((1, 21, 60), "1.17.0"),
    ((1, 21, 70), "1.18.0"),
    ((1, 21, 80), "1.19.0"),
    ((1, 21, 90), "2.0.0"),
    ((1, 21, 100), "2.1.0"),
    ((1, 21, 110), "2.2.0"),
    ((1, 21, 120), "2.3.0"),
    ((1, 21, 130), "2.4.0"),
    ((1, 26, 0), "2.5.0"),
    ((1, 26, 10), "2.6.0"),
    ((1, 26, 20), "2.7.0"),
    ((1, 26, 30), "2.8.0"),
    ((1, 26, 40), "2.9.0"),
    ((1, 26, 50), "2.10.0"),
    ((1, 26, 60), "2.11.0"),
)
OFFICIAL_SERVER = "https://www.minecraft.net/bedrockdedicatedserver/bin-linux/"


def version_numbers(version: str) -> tuple[int, int, int, int]:
    if not re.fullmatch(r"[0-9]+(?:\.[0-9]+){3}", version):
        raise ValueError("BDS version must have four numeric components")
    return tuple(map(int, version.split(".")))


def script_profile(version: str) -> str:
    series = version_numbers(version)[:3]
    if series < (1, 19, 70):
        raise ValueError("The stable @minecraft/server API started in 1.19.70; earlier versions require a different collector")
    match = next((api for minimum, api in reversed(API_PROFILES) if series >= minimum), None)
    if match is None:
        raise ValueError("Unsupported BDS Script API version")
    return match


def discover(version: str) -> tuple[str, str]:
    if version != "latest":
        version_numbers(version)
        if version not in HISTORICAL_SHA1:
            raise ValueError(
                "Historical BDS not pinned: add its exact version and SHA-1 to "
                "HISTORICAL_SHA1 before acquisition"
            )
        return version, OFFICIAL_SERVER + "bedrock-server-" + version + ".zip"
    request = urllib.request.Request(
        LINKS_API, headers={"User-Agent": "BMCBL-Block-Catalog/1.0", "Accept": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=35) as response:
        payload = json.load(response)
    links = payload["result"]["links"]
    urls = [item["downloadUrl"] for item in links if item.get("downloadType") == "serverBedrockLinux"]
    if len(urls) != 1:
        raise RuntimeError("Mojang download API did not provide one Linux release")
    url = urls[0]
    match = re.search(r"/bedrock-server-(\d+(?:\.\d+){3})\.zip$", url)
    parsed = urllib.parse.urlparse(url)
    if not match or parsed.scheme != "https" or parsed.netloc != "www.minecraft.net" or (
        not parsed.path.startswith("/bedrockdedicatedserver/bin-linux/")
    ):
        raise RuntimeError("Unexpected official BDS download URL: " + url)
    actual = match.group(1)
    return actual, url


def download(url: str, target: Path, expected_sha1: str | None) -> str:
    target.parent.mkdir(parents=True, exist_ok=True)
    # curl handles Mojang's CDN HTTP/2 issues more consistently than urllib.
    subprocess.run([
        "curl", "--fail", "--location", "--retry", "5", "--retry-all-errors",
        "--connect-timeout", "25", "--max-time", "240", "--silent", "--show-error",
        "--user-agent", "Mozilla/5.0 BMCBL-Block-Catalog/1.0",
        "--output", str(target), url,
    ], check=True)
    if target.stat().st_size < 1_000_000:
        raise RuntimeError("Downloaded BDS archive is unexpectedly small")
    sha256 = hashlib.sha256()
    sha1 = hashlib.sha1(usedforsecurity=False)
    with target.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            sha256.update(chunk)
            sha1.update(chunk)
    if expected_sha1 and sha1.hexdigest().lower() != expected_sha1.lower():
        target.unlink(missing_ok=True)
        raise RuntimeError("Historic BDS archive digest differs from pinned metadata")
    return sha256.hexdigest()


def extract(archive: Path, destination: Path) -> None:
    with zipfile.ZipFile(archive) as zipped:
        for member in zipped.infolist():
            path = PurePosixPath(member.filename)
            if path.is_absolute() or ".." in path.parts:
                raise RuntimeError("Unsafe archive path: " + member.filename)
            # Disallow zip symlinks, no writes outside the disposable server dir.
            mode = member.external_attr >> 16
            if stat.S_ISLNK(mode):
                raise RuntimeError("Archive contains a symbolic link: " + member.filename)
            if not member.is_dir() and member.file_size > 400 * 1024 * 1024:
                raise RuntimeError("Unexpected oversized BDS archive member")
        zipped.extractall(destination)
    binary = destination / "bedrock_server"
    if not binary.is_file():
        raise RuntimeError("Linux BDS archive does not contain bedrock_server")
    binary.chmod(binary.stat().st_mode | stat.S_IXUSR)


def _nearest_palette_label(version: str) -> str | None:
    """Pick the committed runtime-palette label at or below `version`."""
    palette = Path(__file__).resolve().parents[2] / "crates/bedrock-world/data/block-catalog/palettes"
    if not palette.is_dir():
        return None
    def key(label: str) -> tuple[int, ...]:
        return tuple(int(part) for part in re.findall(r"\d+", label))
    wanted = key(version)
    labels = sorted((p.stem for p in palette.glob("*.nbt")), key=key)
    eligible = [label for label in labels if key(label) <= wanted]
    return eligible[-1] if eligible else None


def write_fallback_block_ids(server: Path, pack: Path, version: str) -> int:
    """Provide explicitly *incomplete* block-name hints from shipped BDS JSON.

    This is for old APIs lacking BlockTypes.getAll. It is never a proof of
    complete block registry enumeration.
    """
    candidates: set[str] = set()
    paths = [server / "definitions" / "blocks.json"]
    paths += list((server / "behavior_packs").glob("vanilla*/blocks.json"))
    for path in paths:
        if not path.is_file():
            continue
        try:
            source = json.loads(path.read_text(encoding="utf-8-sig"))
        except (ValueError, UnicodeError):
            continue
        if not isinstance(source, dict):
            continue
        blocks = source.get("blocks", source)
        if not isinstance(blocks, dict):
            continue
        for name in blocks:
            if isinstance(name, str) and name.startswith("minecraft:"):
                candidates.add(name)

    # Best available name set for APIs lacking BlockTypes.getAll (e.g. 1.20.80):
    # the committed bds-mod-mapping runtime palette for the SAME game version.
    # It supplies candidate NAMES only; BDS Script API still validates each one
    # and the persisted Structure NBT remains the authority on states/version.
    palette_source = "bds_resource_hints"
    palette = Path(__file__).resolve().parents[2] / "crates/bedrock-world/data/block-catalog/palettes"
    label = _nearest_palette_label(version)
    if label and (palette / f"{label}.nbt").is_file():
        try:
            sys.path.insert(0, str(Path(__file__).resolve().parent / "pdb"))
            from parse_runtime_palette import parse_states
            states, _ = parse_states(palette / f"{label}.nbt")
            palette_names = {state["name"] for state in states}
            if len(palette_names) > len(candidates):
                candidates = palette_names
                palette_source = "runtime_palette_names"
        except (ImportError, ValueError, KeyError) as error:
            print(f"palette name hints unavailable: {error}", flush=True)

    target = pack / "scripts" / "block_ids.js"
    target.write_text(
        f"// Block-name candidates from {palette_source}; NOT authoritative for states.\n"
        + "export const fallbackBlockIds = "
        + json.dumps(sorted(candidates), ensure_ascii=False)
        + ";\n", encoding="utf-8",
    )
    return len(candidates)


def configure(server: Path, pack: Path, version: str, api_version: str) -> None:
    target = server / "behavior_packs" / "bmcbl-block-catalog"
    shutil.copytree(pack, target)
    manifest = target / "manifest.json"
    document = json.loads(manifest.read_text(encoding="utf-8"))
    document["header"]["min_engine_version"] = list(version_numbers(version)[:3])
    document["dependencies"] = [{"module_name": "@minecraft/server", "version": api_version}]
    manifest.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    count = write_fallback_block_ids(server, target, version)
    print(f"BDS resource fallback block-name hints: {count}", flush=True)
    properties = server / "server.properties"
    content = properties.read_text(encoding="utf-8") if properties.exists() else ""
    updates = {
        "level-name": WORLD_NAME,
        "content-log-console-output-enabled": "true",
        "online-mode": "false",
        "allow-list": "false",
        "max-players": "1",
        "view-distance": "4",
        "tick-distance": "4",
        "server-port": "19162",
        "server-portv6": "19163",
        "difficulty": "peaceful",
    }
    lines = []
    for line in content.splitlines():
        key = line.partition("=")[0].strip()
        if key in updates and not line.lstrip().startswith("#"):
            continue
        lines.append(line)
    lines.extend(f"{key}={value}" for key, value in updates.items())
    properties.write_text("\n".join(lines) + "\n", encoding="utf-8")
    world = server / "worlds" / WORLD_NAME
    world.mkdir(parents=True, exist_ok=True)
    (world / "world_behavior_packs.json").write_text(
        json.dumps([{"pack_id": PACK_ID, "version": [1, 0, 0]}]) + "\n",
        encoding="utf-8",
    )


def run_server(server: Path, log_path: Path, timeout: int) -> dict:
    environment = os.environ.copy()
    environment["LD_LIBRARY_PATH"] = str(server)
    process = subprocess.Popen(
        ["./bedrock_server"], cwd=server, env=environment,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        encoding="utf-8", errors="replace", bufsize=1
    )
    events: queue.Queue[str | None] = queue.Queue()

    def reader() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            events.put(line)
        events.put(None)

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    started_at = time.monotonic()
    deadline = started_at + timeout
    script_seen = False
    result = None
    try:
        with log_path.open("w", encoding="utf-8") as output:
            while time.monotonic() < deadline:
                if not script_seen and time.monotonic() - started_at > 75:
                    raise RuntimeError(
                        "BDS started but the behavior-pack script emitted no capture markers "
                        "within 75 seconds; incompatible module/event or failed world startup"
                    )
                try:
                    line = events.get(timeout=1)
                except queue.Empty:
                    if process.poll() is not None:
                        break
                    continue
                if line is None:
                    break
                output.write(line)
                output.flush()
                if "BMCBL-DUMP-" in line:
                    script_seen = True
                # Keep the CI console concise while preserving the full diagnostic log.
                if "BMCBL-DUMP-" in line or "ERROR" in line.upper():
                    print(line.rstrip(), flush=True)
                if "BMCBL-DUMP-FAIL " in line:
                    raise RuntimeError("BDS script exporter reported failure")
                if "BMCBL-DUMP-END " in line:
                    result = json.loads(line.split("BMCBL-DUMP-END ", 1)[1].strip())
                    break
            if result is None:
                raise RuntimeError("BDS export did not finish; inspect the uploaded server log")
            assert process.stdin is not None
            process.stdin.write("stop\n")
            process.stdin.flush()
            try:
                process.wait(timeout=45)
            except subprocess.TimeoutExpired as exc:
                raise RuntimeError("BDS did not flush the world database after stop") from exc
            if process.returncode != 0:
                raise RuntimeError("BDS exited with code " + str(process.returncode))
            return result
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=20)
        thread.join(timeout=3)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workdir", required=True, type=Path)
    parser.add_argument("--metadata", required=True, type=Path)
    parser.add_argument("--log", required=True, type=Path)
    parser.add_argument("--version", default="latest")
    parser.add_argument("--timeout", type=int, default=900)
    args = parser.parse_args()

    if args.workdir.exists() and any(args.workdir.iterdir()):
        raise RuntimeError("Refusing to reuse a nonempty BDS workdir: " + str(args.workdir))
    args.workdir.mkdir(parents=True, exist_ok=True)
    version, url = discover(args.version)
    api_version = script_profile(version)
    archive = args.workdir.parent / ("bmcbl-bedrock-" + version + ".zip")
    digest = download(url, archive, HISTORICAL_SHA1.get(version))
    extract(archive, args.workdir)
    configure(args.workdir, Path(__file__).parent / "behavior_pack", version, api_version)
    args.log.parent.mkdir(parents=True, exist_ok=True)
    stats = run_server(args.workdir, args.log, args.timeout)
    method = stats.get("mode", "script_api_structure")
    if method not in ("script_api_structure", "script_api_probe"):
        raise RuntimeError("BDS exporter reported an unrecognized capture mode")
    if stats.get("blocks", 0) < 1 or (method == "script_api_structure" and (
        stats.get("blocks", 0) < 1000 or stats.get("placed", 0) < 10000
    )):
        raise RuntimeError("BDS exporter reported an incomplete capture: " + repr(stats))
    args.metadata.parent.mkdir(parents=True, exist_ok=True)
    args.metadata.write_text(
        json.dumps({"game_version": version, "bds_sha256": digest, "source_url": url,
                    "capture_method": method, "api_module": api_version,
                    "stats": stats}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if method == "script_api_probe":
        probes = []
        for line in args.log.read_text(encoding="utf-8").splitlines():
            if "BMCBL-DUMP-PROBE " not in line:
                continue
            probes.append(json.loads(line.split("BMCBL-DUMP-PROBE ", 1)[1]))
        if len(probes) != stats["blocks"]:
            raise RuntimeError("Partial API probe does not contain every reported block")
        report = args.metadata.with_name("bmcbl-api-probe.json")
        report.write_text(json.dumps({
            "game_version": version, "api_module": api_version,
            "warning": "API-only observations: no authoritative persisted NBT types, numeric IDs or block-state version",
            "blocks": probes,
        }, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
        print(f"BMCBL {version}: API-only probe exported {len(probes)} blocks; full binary catalog NOT generated")
    else:
        print(f"BMCBL capture completed: BDS {version}, {stats['blocks']} blocks, "
              f"{stats['placed']} written permutations; SHA256 {digest}")
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            file.write("method=" + method + "\n")
            file.write("game_version=" + version + "\n")


if __name__ == "__main__":
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print("BMCBL BDS export failed:", error, file=sys.stderr)
        raise SystemExit(1) from error

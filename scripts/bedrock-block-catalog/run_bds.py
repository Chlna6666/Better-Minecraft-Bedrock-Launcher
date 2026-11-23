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


def discover(version: str) -> tuple[str, str]:
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
    if version != "latest" and version != actual:
        raise RuntimeError("Only the latest official release is available from the discovery API; requested "
                           + version + ", found " + actual)
    return actual, url


def download(url: str, target: Path) -> str:
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
    sha = hashlib.sha256()
    with target.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


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


def configure(server: Path, pack: Path) -> None:
    target = server / "behavior_packs" / "bmcbl-block-catalog"
    shutil.copytree(pack, target)
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
    deadline = time.monotonic() + timeout
    result = None
    try:
        with log_path.open("w", encoding="utf-8") as output:
            while time.monotonic() < deadline:
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
    archive = args.workdir.parent / ("bmcbl-bedrock-" + version + ".zip")
    digest = download(url, archive)
    extract(archive, args.workdir)
    configure(args.workdir, Path(__file__).parent / "behavior_pack")
    args.log.parent.mkdir(parents=True, exist_ok=True)
    stats = run_server(args.workdir, args.log, args.timeout)
    if stats.get("blocks", 0) < 1000 or stats.get("placed", 0) < 10000:
        raise RuntimeError("BDS exporter reported an incomplete capture: " + repr(stats))
    args.metadata.parent.mkdir(parents=True, exist_ok=True)
    args.metadata.write_text(
        json.dumps({"game_version": version, "bds_sha256": digest, "source_url": url,
                    "capture_method": "script_api_structure", "stats": stats},
                   indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"BMCBL capture completed: BDS {version}, {stats['blocks']} blocks, "
          f"{stats['placed']} written permutations; SHA256 {digest}")


if __name__ == "__main__":
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print("BMCBL BDS export failed:", error, file=sys.stderr)
        raise SystemExit(1) from error

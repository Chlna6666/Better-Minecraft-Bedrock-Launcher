#!/usr/bin/env python3
"""Fetch blockstate/model JSON from the latest stable Minecraft: Java Edition client.

The default source is Mojang's official Piston version manifest. Only stable
``release`` versions are accepted. The client JAR is SHA-1/size verified and
cached; extraction is limited to vanilla blockstates and block models so this
script can feed ``bedrock-block-model`` without carrying the full Java client.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
import tempfile
import time
import uuid
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


VERSION_MANIFEST_URL = (
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
)
PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ROOT = PROJECT_ROOT / "target" / "java-model-cache"
DEFAULT_OUTPUT = DEFAULT_ROOT / "current"
DEFAULT_CACHE = DEFAULT_ROOT / "downloads"
USER_AGENT = "BMCBL bedrock-block-model java-model-fetcher/1.0"
CHUNK_SIZE = 1024 * 1024
OUTPUT_SCHEMA = 1
ASSET_PREFIXES = {
    "blockstates": "assets/minecraft/blockstates/",
    "block_models": "assets/minecraft/models/block/",
}


class FetchError(RuntimeError):
    """Raised when Mojang metadata/client assets cannot be validated."""


def request(url: str, *, timeout: float):
    return urlopen(
        Request(
            url,
            headers={
                "User-Agent": USER_AGENT,
                "Accept": "application/json,application/java-archive,*/*;q=0.1",
            },
        ),
        timeout=timeout,
    )


def retry(operation, *, retries: int, label: str):
    last_error: Exception | None = None
    for attempt in range(retries + 1):
        try:
            return operation()
        except (HTTPError, URLError, TimeoutError, OSError) as error:
            last_error = error
            if attempt >= retries:
                break
            delay = min(2**attempt, 8)
            print(
                f"{label} failed ({error}); retrying in {delay}s "
                f"[{attempt + 1}/{retries}]",
                file=sys.stderr,
            )
            time.sleep(delay)
    raise FetchError(f"{label} failed after {retries + 1} attempts: {last_error}")


def fetch_bytes(url: str, *, timeout: float, retries: int, label: str) -> bytes:
    def once() -> bytes:
        with request(url, timeout=timeout) as response:
            return response.read()

    return retry(once, retries=retries, label=label)


def verify_sha1(payload: bytes, expected: str, *, label: str) -> None:
    actual = hashlib.sha1(payload).hexdigest()
    if actual.lower() != expected.lower():
        raise FetchError(f"{label} SHA-1 mismatch: expected {expected}, got {actual}")


def parse_json(payload: bytes, *, label: str) -> dict[str, Any]:
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise FetchError(f"invalid {label} JSON: {error}") from error
    if not isinstance(value, dict):
        raise FetchError(f"invalid {label} JSON: root must be an object")
    return value


def load_version_manifest(*, timeout: float, retries: int) -> dict[str, Any]:
    return parse_json(
        fetch_bytes(
            VERSION_MANIFEST_URL,
            timeout=timeout,
            retries=retries,
            label="version manifest download",
        ),
        label="version manifest",
    )


def select_release(manifest: dict[str, Any], requested: str) -> dict[str, Any]:
    latest = manifest.get("latest")
    versions = manifest.get("versions")
    if not isinstance(latest, dict) or not isinstance(versions, list):
        raise FetchError("version manifest is missing latest/versions")

    if requested in {"latest", "latest-release", "release"}:
        version_id = latest.get("release")
        if not isinstance(version_id, str) or not version_id:
            raise FetchError("version manifest does not declare latest.release")
    else:
        version_id = requested

    for entry in versions:
        if not isinstance(entry, dict) or entry.get("id") != version_id:
            continue
        release_type = entry.get("type")
        if release_type != "release":
            raise FetchError(
                f"version {version_id!r} is {release_type!r}, not a stable release"
            )
        if not isinstance(entry.get("url"), str):
            raise FetchError(f"version {version_id!r} has no metadata URL")
        return entry

    raise FetchError(f"stable release {version_id!r} was not found in Mojang manifest")


def load_version_metadata(
    entry: dict[str, Any], *, timeout: float, retries: int
) -> dict[str, Any]:
    url = entry["url"]
    payload = fetch_bytes(
        url,
        timeout=timeout,
        retries=retries,
        label=f"version metadata download ({entry['id']})",
    )
    expected_sha1 = entry.get("sha1")
    if isinstance(expected_sha1, str) and expected_sha1:
        verify_sha1(payload, expected_sha1, label=f"version metadata {entry['id']}")
    metadata = parse_json(payload, label=f"version metadata {entry['id']}")
    if metadata.get("id") != entry.get("id"):
        raise FetchError(
            f"version metadata ID mismatch: expected {entry.get('id')!r}, "
            f"got {metadata.get('id')!r}"
        )
    if metadata.get("type") != "release":
        raise FetchError(
            f"version metadata {entry['id']!r} is not marked as a stable release"
        )
    return metadata


def client_download(metadata: dict[str, Any]) -> dict[str, Any]:
    downloads = metadata.get("downloads")
    client = downloads.get("client") if isinstance(downloads, dict) else None
    if not isinstance(client, dict):
        raise FetchError(f"version {metadata.get('id')!r} has no client download")

    url = client.get("url")
    sha1 = client.get("sha1")
    size = client.get("size")
    if not isinstance(url, str) or not url:
        raise FetchError("client download URL is missing")
    if not isinstance(sha1, str) or len(sha1) != 40:
        raise FetchError("client download SHA-1 is missing or invalid")
    if not isinstance(size, int) or size <= 0:
        raise FetchError("client download size is missing or invalid")
    return {"url": url, "sha1": sha1.lower(), "size": size}


def file_sha1(path: Path) -> tuple[str, int]:
    digest = hashlib.sha1()
    size = 0
    with path.open("rb") as source:
        while chunk := source.read(CHUNK_SIZE):
            digest.update(chunk)
            size += len(chunk)
    return digest.hexdigest(), size


def cache_is_valid(path: Path, *, expected_sha1: str, expected_size: int) -> bool:
    if not path.is_file():
        return False
    if path.stat().st_size != expected_size:
        return False
    actual_sha1, actual_size = file_sha1(path)
    return actual_size == expected_size and actual_sha1.lower() == expected_sha1.lower()


def download_client(
    client: dict[str, Any],
    cache_dir: Path,
    *,
    timeout: float,
    retries: int,
    force: bool,
) -> Path:
    cache_dir.mkdir(parents=True, exist_ok=True)
    destination = cache_dir / f"client-{client['sha1']}.jar"
    if not force and cache_is_valid(
        destination,
        expected_sha1=client["sha1"],
        expected_size=client["size"],
    ):
        print(f"using verified client cache: {destination}")
        return destination

    if destination.exists():
        destination.unlink()

    def once() -> Path:
        temporary = destination.with_name(
            f".{destination.name}.tmp-{uuid.uuid4().hex}"
        )
        digest = hashlib.sha1()
        size = 0
        try:
            with request(client["url"], timeout=timeout) as response, temporary.open(
                "wb"
            ) as output:
                while True:
                    chunk = response.read(CHUNK_SIZE)
                    if not chunk:
                        break
                    output.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
            actual_sha1 = digest.hexdigest()
            if size != client["size"]:
                raise FetchError(
                    f"client size mismatch: expected {client['size']}, got {size}"
                )
            if actual_sha1.lower() != client["sha1"].lower():
                raise FetchError(
                    f"client SHA-1 mismatch: expected {client['sha1']}, "
                    f"got {actual_sha1}"
                )
            temporary.replace(destination)
            return destination
        finally:
            temporary.unlink(missing_ok=True)

    try:
        return retry(once, retries=retries, label="client JAR download")
    except FetchError:
        destination.unlink(missing_ok=True)
        raise


def safe_asset_path(name: str) -> Path:
    if "\\" in name:
        raise FetchError(f"unsafe path in client JAR: {name!r}")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in path.parts):
        raise FetchError(f"unsafe path in client JAR: {name!r}")
    return Path(*path.parts)


def extract_model_assets(client_jar: Path, output: Path) -> dict[str, int]:
    counts = {"blockstates": 0, "block_models": 0, "bytes": 0}
    try:
        with zipfile.ZipFile(client_jar) as archive:
            selected: list[tuple[zipfile.ZipInfo, str]] = []
            for info in archive.infolist():
                if info.is_dir() or not info.filename.endswith(".json"):
                    continue
                category = next(
                    (
                        key
                        for key, prefix in ASSET_PREFIXES.items()
                        if info.filename.startswith(prefix)
                    ),
                    None,
                )
                if category is not None:
                    selected.append((info, category))

            selected.sort(key=lambda item: item[0].filename)
            for info, category in selected:
                relative = safe_asset_path(info.filename)
                destination = output / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(info, "r") as source, destination.open("wb") as target:
                    shutil.copyfileobj(source, target, length=CHUNK_SIZE)
                counts[category] += 1
                counts["bytes"] += info.file_size
    except (zipfile.BadZipFile, OSError) as error:
        raise FetchError(f"failed to extract client JAR {client_jar}: {error}") from error

    if counts["blockstates"] == 0 or counts["block_models"] == 0:
        raise FetchError(
            "client JAR did not contain vanilla blockstates and block models; "
            "the Java asset layout may have changed"
        )
    return counts


def count_json_files(path: Path) -> int:
    return sum(1 for candidate in path.rglob("*.json") if candidate.is_file())


def output_is_current(output: Path, *, version_id: str, client_sha1: str) -> bool:
    manifest_path = output / "manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        return False

    if not isinstance(manifest, dict) or manifest.get("schema") != OUTPUT_SCHEMA:
        return False
    version = manifest.get("version")
    client = manifest.get("client")
    extracted = manifest.get("extracted")
    if not isinstance(version, dict) or version.get("id") != version_id:
        return False
    if not isinstance(client, dict) or client.get("sha1") != client_sha1:
        return False
    if not isinstance(extracted, dict):
        return False

    blockstates = output / "assets" / "minecraft" / "blockstates"
    models = output / "assets" / "minecraft" / "models" / "block"
    expected_blockstates = extracted.get("blockstates")
    expected_models = extracted.get("block_models")
    if not isinstance(expected_blockstates, int) or not isinstance(expected_models, int):
        return False
    return (
        blockstates.is_dir()
        and models.is_dir()
        and count_json_files(blockstates) == expected_blockstates
        and count_json_files(models) == expected_models
    )


def build_output_manifest(
    entry: dict[str, Any], metadata: dict[str, Any], client: dict[str, Any], counts: dict[str, int]
) -> dict[str, Any]:
    return {
        "schema": OUTPUT_SCHEMA,
        "source": {
            "version_manifest": VERSION_MANIFEST_URL,
            "version_metadata": entry["url"],
        },
        "version": {
            "id": entry["id"],
            "type": entry.get("type"),
            "release_time": entry.get("releaseTime"),
            "metadata_time": entry.get("time"),
            "metadata_sha1": entry.get("sha1"),
        },
        "client": {
            "url": client["url"],
            "sha1": client["sha1"],
            "size": client["size"],
        },
        "extracted": {
            "blockstates": counts["blockstates"],
            "block_models": counts["block_models"],
            "bytes": counts["bytes"],
            "roots": list(ASSET_PREFIXES.values()),
        },
        "java_version": metadata.get("javaVersion"),
    }


def atomic_replace_directory(staged: Path, destination: Path) -> None:
    backup = destination.with_name(f".{destination.name}.backup-{uuid.uuid4().hex}")
    had_destination = destination.exists()
    try:
        if had_destination:
            destination.replace(backup)
        staged.replace(destination)
    except Exception:
        if destination.exists() and not had_destination:
            shutil.rmtree(destination, ignore_errors=True)
        if backup.exists() and not destination.exists():
            backup.replace(destination)
        raise
    finally:
        if backup.exists():
            shutil.rmtree(backup, ignore_errors=True)


def fetch_models(
    *,
    requested_version: str,
    output: Path,
    cache_dir: Path,
    timeout: float,
    retries: int,
    force: bool,
) -> dict[str, Any]:
    manifest = load_version_manifest(timeout=timeout, retries=retries)
    entry = select_release(manifest, requested_version)
    metadata = load_version_metadata(entry, timeout=timeout, retries=retries)
    client = client_download(metadata)

    if not force and output_is_current(
        output, version_id=entry["id"], client_sha1=client["sha1"]
    ):
        existing = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
        print(f"Java block models are already current: {entry['id']} ({output})")
        return existing

    client_jar = download_client(
        client,
        cache_dir,
        timeout=timeout,
        retries=retries,
        force=force,
    )

    output.parent.mkdir(parents=True, exist_ok=True)
    staged = Path(
        tempfile.mkdtemp(prefix=f".{output.name}.tmp-", dir=str(output.parent))
    )
    try:
        counts = extract_model_assets(client_jar, staged)
        output_manifest = build_output_manifest(entry, metadata, client, counts)
        (staged / "manifest.json").write_text(
            json.dumps(output_manifest, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        atomic_replace_directory(staged, output)
    finally:
        if staged.exists():
            shutil.rmtree(staged, ignore_errors=True)

    print(
        f"fetched Java {entry['id']} block assets: "
        f"{counts['blockstates']} blockstates, {counts['block_models']} models "
        f"({counts['bytes']} bytes) -> {output}"
    )
    return output_manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--version",
        default="latest-release",
        help="stable Java version ID; default: latest-release from Mojang manifest",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help=f"extraction root; default: {DEFAULT_OUTPUT}",
    )
    parser.add_argument(
        "--cache-dir",
        type=Path,
        default=DEFAULT_CACHE,
        help=f"verified client JAR cache; default: {DEFAULT_CACHE}",
    )
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--retries", type=int, default=3)
    parser.add_argument(
        "--force",
        action="store_true",
        help="redownload/reextract even when the verified cache/output is current",
    )
    arguments = parser.parse_args()

    if arguments.timeout <= 0:
        parser.error("--timeout must be greater than zero")
    if arguments.retries < 0:
        parser.error("--retries must be zero or greater")

    try:
        fetch_models(
            requested_version=arguments.version,
            output=arguments.output.resolve(),
            cache_dir=arguments.cache_dir.resolve(),
            timeout=arguments.timeout,
            retries=arguments.retries,
            force=arguments.force,
        )
    except FetchError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

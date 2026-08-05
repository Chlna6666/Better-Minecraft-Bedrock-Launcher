from __future__ import annotations

import json
import shutil
import urllib.request
import zipfile
from pathlib import Path

PACKAGE_DIR = Path(__file__).resolve().parent
SAMPLES_DIR = PACKAGE_DIR / "bedrock-samples"
VERSION_FILE = SAMPLES_DIR / "version.json"

_RELEASE_API = "https://api.github.com/repos/Mojang/bedrock-samples/releases/latest"
_USER_AGENT = "BMCBL entity icon generator"


def _fetch_json(url: str) -> dict:
    request = urllib.request.Request(
        url, headers={"User-Agent": _USER_AGENT}
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def resource_pack_root() -> Path:
    """Ensure the newest Mojang bedrock-samples release resources are local."""
    SAMPLES_DIR.mkdir(parents=True, exist_ok=True)
    release = _fetch_json(_RELEASE_API)
    tag = release["tag_name"]
    cached_tag = None
    if VERSION_FILE.exists():
        cached_tag = json.loads(VERSION_FILE.read_text(encoding="utf-8")).get("tag")
    cached_root = SAMPLES_DIR / tag / "resource_pack"
    if cached_tag == tag and cached_root.is_dir():
        return cached_root

    assets = release.get("assets", [])
    asset = next(
        (item for item in assets if item["name"].endswith("-full.zip")),
        next((item for item in assets if item["name"].endswith("-min.zip")), None),
    )
    if asset is None:
        raise RuntimeError("bedrock-samples release has no resource archive")

    dest_dir = SAMPLES_DIR / tag
    dest_dir.mkdir(parents=True, exist_ok=True)
    archive_path = dest_dir / asset["name"]
    if not archive_path.exists():
        request = urllib.request.Request(
            asset["browser_download_url"],
            headers={"User-Agent": _USER_AGENT},
        )
        with urllib.request.urlopen(request, timeout=600) as response:
            with archive_path.open("wb") as output:
                shutil.copyfileobj(response, output)
    with zipfile.ZipFile(archive_path) as archive:
        archive.extractall(dest_dir)

    (SAMPLES_DIR / "version.json").write_text(
        json.dumps({"tag": tag}), encoding="utf-8"
    )
    root = dest_dir / "resource_pack"
    return root

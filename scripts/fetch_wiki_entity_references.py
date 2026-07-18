#!/usr/bin/env python3
"""Fetch the entity sprites from the Minecraft Wiki creature table.

The files are local comparison references only. They are written below
``target/wiki_entity_reference`` and are never embedded into the application.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import json
import re
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from urllib.request import Request, urlopen


DEFAULT_PAGE = "https://zh.minecraft.wiki/w/%E7%94%9F%E7%89%A9"
PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = PROJECT_ROOT / "target" / "wiki_entity_reference"
ENTITY_SPRITE_PATTERN = re.compile(
    r"(?:(?:https?:)?//zh\.minecraft\.wiki)?/images/EntitySprite_[^\"'<>\s]+?\.png(?:\?[^\"'<>\s]*)?",
    re.IGNORECASE,
)


def fetch(url: str) -> bytes:
    request = Request(
        url,
        headers={
            "User-Agent": "BMCBL wiki-reference-fetcher/1.0",
            "Accept": "text/html,image/png,image/*;q=0.8,*/*;q=0.1",
        },
    )
    with urlopen(request, timeout=30) as response:
        return response.read()


def image_urls(page_url: str) -> list[str]:
    source = fetch(page_url).decode("utf-8", errors="replace")
    urls: set[str] = set()
    for match in ENTITY_SPRITE_PATTERN.findall(source):
        normalized = urljoin(page_url, html.unescape(match))
        if normalized.startswith("//"):
            normalized = "https:" + normalized
        urls.add(normalized)
    return sorted(urls)


def output_name(url: str) -> str:
    name = Path(unquote(urlsplit(url).path)).name
    return name.lower().replace(" ", "_")


def fetch_references(page_url: str, output: Path) -> dict[str, object]:
    urls = image_urls(page_url)
    output.mkdir(parents=True, exist_ok=True)
    entries: list[dict[str, object]] = []
    for url in urls:
        destination = output / output_name(url)
        payload = fetch(url)
        destination.write_bytes(payload)
        entries.append(
            {
                "file": destination.name,
                "url": url,
                "sha256": hashlib.sha256(payload).hexdigest(),
                "bytes": len(payload),
            }
        )
    manifest = {
        "source_page": page_url,
        "count": len(entries),
        "entries": entries,
    }
    (output / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--page", default=DEFAULT_PAGE)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    arguments = parser.parse_args()
    manifest = fetch_references(arguments.page, arguments.output.resolve())
    print(f"fetched {manifest['count']} Wiki entity sprites into {arguments.output}")


if __name__ == "__main__":
    main()

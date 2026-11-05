#!/usr/bin/env python3
"""Public release-note generator shared by GitHub Releases and CHANGELOG.md."""
from __future__ import annotations

import argparse
from collections import OrderedDict
from dataclasses import dataclass
import re
import subprocess

COMMIT_RE = re.compile(r"^(?P<kind>[a-z]+)(?:\((?P<scope>[^)]+)\))?(?P<breaking>!)?:\s*(?P<text>.+)$", re.I)
SKIP_CI_RE = re.compile(r"\s*\[skip ci\]\s*$", re.I)
PUBLIC_KINDS = {"feat": "Added", "fix": "Fixed", "perf": "Performance"}
INTERNAL_SCOPES = {"ci", "test", "tests", "docs", "doc", "bench", "benchmark", "lint", "lints", "format", "changelog", "build"}
PACKAGING_SCOPES = {"packaging", "package", "installer", "distribution", "release", "windows", "linux", "macos", "flatpak", "appimage", "rpm", "deb"}
SECURITY_RE = re.compile(r"\b(?:cve-\d{4}-\d+|security|vulnerabilit(?:y|ies))\b", re.I)
PUBLIC_MARKER_RE = re.compile(r"\s*\[release-note\]\s*", re.I)
SECTION_ORDER = ("Added", "Fixed", "Performance", "Compatibility")


@dataclass(frozen=True)
class Change:
    hash: str
    text: str
    category: str
    scope: str
    date: str


def parse_commit(hash_: str, subject: str, date: str = "") -> Change | None:
    subject = SKIP_CI_RE.sub("", subject).strip()
    match = COMMIT_RE.match(subject)
    if not match:
        return None
    kind = match["kind"].lower()
    scope = (match["scope"] or "general").lower()
    description = match["text"].strip()
    marked = bool(PUBLIC_MARKER_RE.search(description))
    description = PUBLIC_MARKER_RE.sub("", description).strip()
    if not description or kind in {"ci", "docs", "test", "style"} or scope in INTERNAL_SCOPES:
        return None
    if kind in PUBLIC_KINDS:
        category = PUBLIC_KINDS[kind]
    elif kind == "build" and (scope in PACKAGING_SCOPES or marked):
        category = "Compatibility"
    elif kind == "chore" and (SECURITY_RE.search(description) or marked):
        category = "Compatibility"
    elif kind in {"refactor", "revert", "build"} and (marked or bool(match["breaking"])):
        category = "Compatibility"
    else:
        return None
    return Change(hash_, description, category, scope, date)


def collect_commits(since: str | None, until: str) -> list[Change]:
    revision = f"{since}..{until}" if since else until
    command = ["git", "log", "--no-merges", "--format=%h%x1f%s%x1f%ad", "--date=short"]
    if not since:
        command += ["-n", "100"]
    command.append(revision)
    history = subprocess.check_output(command, text=True, encoding="utf-8")
    changes: list[Change] = []
    for line in history.splitlines():
        parts = line.split("\x1f", 2)
        if len(parts) != 3:
            continue
        change = parse_commit(*parts)
        if change:
            changes.append(change)
    return changes


def commit_sha(reference: str) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", "--verify", f"{reference}^{{commit}}"],
        text=True, encoding="utf-8"
    ).strip()


def render_summary(changes: list[Change], *, max_groups: int = 12, max_items: int = 2,
                   changelog: bool = False) -> str:
    sections: dict[str, OrderedDict[str, list[Change]]] = {
        category: OrderedDict() for category in SECTION_ORDER
    }
    for change in changes:
        group = sections[change.category].setdefault(change.scope, [])
        if not any(existing.text.casefold() == change.text.casefold() for existing in group):
            group.append(change)

    lines: list[str] = []
    if changelog:
        lines += ["### Commit Summary", "", "Automatically generated from user-facing changes.", ""]
    for category, scopes in sections.items():
        if not scopes:
            continue
        lines += [f"### {category}", ""]
        for scope, group in list(scopes.items())[:max_groups]:
            lines.append(f"- **{scope}**")
            for change in group[:max_items]:
                suffix = f", {change.date}" if changelog and change.date else ""
                lines.append(f"  - {change.text} ({chr(96)}{change.hash}{chr(96)}{suffix})")
            extra = len(group) - max_items
            if extra > 0:
                lines.append(f"  - {extra} additional changes in this module")
        other_scopes = list(scopes.items())[max_groups:]
        if other_scopes:
            other_count = sum(len(group) for _, group in other_scopes)
            lines.append(f"- {other_count} additional changes across {len(other_scopes)} other modules")
        lines.append("")
    if not any(sections.values()):
        lines += ["No user-facing changes in this range.", ""]
    return "\n".join(lines).rstrip()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--since", help="Previous release tag or commit (exclusive)")
    parser.add_argument("--until", default="HEAD", help="Release commit (inclusive)")
    parser.add_argument("--format", choices=("release", "changelog"), default="release")
    parser.add_argument("--repo", help="GitHub repository owner/name, for comparison link")
    args = parser.parse_args()

    changelog = args.format == "changelog"
    notes = render_summary(collect_commits(args.since, args.until), changelog=changelog)
    if args.repo and args.since:
        # Immutable SHA comparison: the nightly baseline tag is moved after publication.
        base, head = commit_sha(args.since), commit_sha(args.until)
        notes += f"\n\n[View full commit history](https://github.com/{args.repo}/compare/{base}...{head})"
    print(notes)


if __name__ == "__main__":
    main()

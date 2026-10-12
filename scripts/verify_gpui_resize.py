#!/usr/bin/env python3
"""Verify final settled Win32 client, GPUI viewport and GPU swapchain dimensions.

Run after resizing has stopped and the GPU has had time to present the final frame.
No external dependencies. --self-test exercises the checker without Windows.
"""
from __future__ import annotations

import argparse
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path

EVENT = re.compile(r"\bgpui_resize\s+stage=(native|viewport|gpu|gpu_queued)\b(.*)")
FIELD = re.compile(r"([a-z_]+)=([^\s]+)")
EXTENT = re.compile(r"^(\d+)x(\d+)$")


@dataclass
class Stages:
    native: dict[str, str] | None = None
    viewport: dict[str, str] | None = None
    gpu: dict[str, str] | None = None
    queued: dict[str, str] | None = None
    events: int = 0


def parse(lines: list[str]) -> dict[int, Stages]:
    windows: dict[int, Stages] = {}
    for line in lines:
        match = EVENT.search(line)
        if not match:
            continue
        stage, body = match.groups()
        fields = dict(FIELD.findall(body))
        try:
            ident = int(fields["window_id"])
        except (KeyError, ValueError):
            continue
        # The GPU worker can run a preparation command before first Draw binds WindowId.
        if ident <= 0:
            continue
        record = windows.setdefault(ident, Stages())
        setattr(record, {"gpu_queued": "queued"}.get(stage, stage), fields)
        record.events += 1
    return windows


def extent(fields: dict[str, str], key: str = "physical_px") -> tuple[int, int]:
    value = fields.get(key, "")
    match = EXTENT.fullmatch(value)
    if not match:
        raise ValueError(f"missing/invalid {key}={value!r}")
    return int(match[1]), int(match[2])


def validate(windows: dict[int, Stages], minimum: int) -> list[str]:
    failures: list[str] = []
    checked = 0
    for ident, stages in sorted(windows.items()):
        if stages.native is None:
            continue  # Static sibling windows need not receive WM_SIZE.
        checked += 1
        if stages.viewport is None or stages.gpu is None:
            failures.append(
                f"window {ident}: missing stage(s): "
                f"viewport={stages.viewport is not None} gpu={stages.gpu is not None}"
            )
            continue
        try:
            native_px = extent(stages.native)
            viewport_px = extent(stages.viewport)
            gpu_px = extent(stages.gpu)
            event_px = extent(stages.native, "event_px")
        except ValueError as error:
            failures.append(f"window {ident}: {error}")
            continue
        if any(abs(a - b) > 1 for a, b in zip(native_px, event_px)):
            failures.append(
                f"window {ident}: stale winit WM_SIZE event {event_px}, Win32 client {native_px}"
            )
        if any(abs(a - b) > 1 for a, b in zip(native_px, viewport_px)):
            failures.append(
                f"window {ident}: native {native_px} != GPUI viewport {viewport_px}"
            )
        if native_px != gpu_px:
            queued = extent(stages.queued) if stages.queued else None
            failures.append(
                f"window {ident}: native {native_px} != committed GPU {gpu_px}; "
                f"latest GPU queued {queued}"
            )
        if not any(x.startswith(f"window {ident}:") for x in failures):
            print(
                f"OK window {ident}: native / viewport / swapchain "
                f"{native_px}, {stages.events} events"
            )
    if checked < minimum:
        failures.append(
            f"only {checked} resized window(s) traced; expected at least {minimum}"
        )
    return failures


class CheckerTests(unittest.TestCase):
    def test_resize_converges_with_cross_dpi_and_intermediate_sizes(self):
        logs = [
            "gpui_resize stage=native window_id=4 physical_px=800x600 event_px=800x600 scale=1.0",
            "gpui_resize stage=viewport window_id=4 physical_px=800x600 scale=1.0",
            "gpui_resize stage=gpu window_id=4 physical_px=800x600",
            "gpui_resize stage=native window_id=4 physical_px=1250x875 event_px=1250x875 scale=1.25",
            "gpui_resize stage=viewport window_id=4 physical_px=1250x875 scale=1.25",
            "gpui_resize stage=gpu window_id=4 physical_px=1250x875",
        ]
        self.assertFalse(validate(parse(logs), 1))

    def test_gpu_stalls_are_detected(self):
        logs = [
            "gpui_resize stage=native window_id=5 physical_px=1200x800 event_px=1200x800",
            "gpui_resize stage=viewport window_id=5 physical_px=1200x800",
            "gpui_resize stage=gpu_queued window_id=5 physical_px=1200x800",
            "gpui_resize stage=gpu window_id=5 physical_px=900x600",
        ]
        self.assertTrue(
            any("committed GPU" in error for error in validate(parse(logs), 1))
        )

    def test_reentrant_native_event_missing_viewport_detected(self):
        lines = [
            "gpui_resize stage=native window_id=1 physical_px=1440x900 event_px=1440x900"
        ]
        self.assertTrue(
            any("missing stage" in error for error in validate(parse(lines), 1))
        )

    def test_multi_window_isolation(self):
        lines = []
        for ident, size in [(8, "1000x600"), (9, "1280x720")]:
            for stage in ("native", "viewport", "gpu"):
                lines.append(
                    f"gpui_resize stage={stage} window_id={ident} "
                    f"physical_px={size} event_px={size}"
                )
        self.assertFalse(validate(parse(lines), 2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "log_file",
        nargs="?",
        type=Path,
        help="BMCBL latest.log recorded with BMCBL_GPUI_RESIZE_TRACE=1",
    )
    parser.add_argument(
        "--min-windows",
        type=int,
        default=1,
        help="number of windows to resize deliberately",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(CheckerTests)
        sys.exit(
            not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful()
        )
    if args.log_file is None:
        parser.error("provide log_file or --self-test")
    stages = parse(
        args.log_file.read_text(encoding="utf-8", errors="replace").splitlines()
    )
    failed = validate(stages, args.min_windows)
    for failure in failed:
        print("FAIL", failure, file=sys.stderr)
    sys.exit(1 if failed else 0)

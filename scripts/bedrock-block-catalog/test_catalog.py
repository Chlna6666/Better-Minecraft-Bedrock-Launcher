"""Pure Python offline tests for historical Script API profiles and catalog diffs."""
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest
import json

ROOT = Path(__file__).resolve().parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    instance = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(instance)
    return instance


runner = module("run_bds", "run_bds.py")
diff = module("diff_catalog", "diff_catalog.py")


def varint(value):
    result = bytearray()
    while value > 127:
        result.append(value & 127 | 128)
        value >>= 7
    result.append(value)
    return result


def pack_manifest(blocks):
    result = bytearray(b"BMCBLP01")
    result += varint(len(blocks))
    for name, (digest, default_index) in sorted(blocks.items()):
        encoded = name.encode("utf8")
        result += varint(len(encoded))
        result += encoded
        result += int(digest, 16).to_bytes(16, "little")
        result += varint(default_index)
    return result


class Profiles(unittest.TestCase):
    def test_minimum_stable_module(self):
        self.assertEqual("1.0.0", runner.script_profile("1.19.70.02"))
        self.assertEqual("1.2.0", runner.script_profile("1.20.0.01"))
        self.assertEqual("1.9.0", runner.script_profile("1.20.70.05"))
        self.assertEqual("1.10.0", runner.script_profile("1.20.80.05"))
        self.assertEqual("1.11.0", runner.script_profile("1.21.0.03"))
        self.assertEqual("1.13.0", runner.script_profile("1.21.20.03"))
        self.assertEqual("2.0.0", runner.script_profile("1.21.90.03"))
        self.assertEqual("2.11.0", runner.script_profile("1.26.66.1"))

    def test_historical_discovery_uses_pinned_archive_without_latest_api(self):
        version, url = runner.discover("1.19.70.02")
        self.assertEqual(version, "1.19.70.02")
        self.assertEqual(url, "https://www.minecraft.net/bedrockdedicatedserver/bin-linux/bedrock-server-1.19.70.02.zip")

    def test_pre_stable_not_misidentified(self):
        with self.assertRaises(ValueError):
            runner.script_profile("1.19.60.04")

    def test_cross_name_hash_equality_not_auto_alias(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "manifests").mkdir()
            first = {"minecraft:legacy_name": ("ab" * 16, 0)}
            second = {"minecraft:new_name": ("ab" * 16, 0)}
            (root / "manifests" / "a.bin").write_bytes(pack_manifest(first))
            (root / "manifests" / "b.bin").write_bytes(pack_manifest(second))
            (root / "index.json").write_text(json.dumps({
                "format_version": 1,
                "versions": {
                    "1.20.0.01": {"manifest": "a", "storage_version": 1},
                    "1.21.0.03": {"manifest": "b", "storage_version": 2},
                },
            }), encoding="utf-8")
            report = diff.compare(root, "1.20.0.01", "1.21.0.03")
            self.assertEqual(1, report["counts"]["unverified_rename_candidates"])
            self.assertFalse(report["rename_candidates_not_authoritative"][0]["verified"])
            self.assertEqual(["minecraft:legacy_name"], report["removed"])
            self.assertEqual(["minecraft:new_name"], report["added"])


if __name__ == "__main__":
    unittest.main()

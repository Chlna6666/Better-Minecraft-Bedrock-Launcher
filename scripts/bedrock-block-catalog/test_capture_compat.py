"""Tests for historical capture compatibility helpers."""
from pathlib import Path
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from run_bds import _nearest_palette_label, write_fallback_block_ids


class PaletteNameFallbackTests(unittest.TestCase):
    def test_nearest_label_at_or_below(self):
        # These labels must resolve to the committed runtime palette for the
        # same major version, never a newer one.
        expected = {
            "1.20.80.05": "1.20.70.24_beta",
            "1.21.20.03": "1.21.0.25_beta",
            "1.21.80.3": "1.21.70.25_beta",
            "1.21.100.7": "1.21.80.27_beta",
        }
        for version, label in expected.items():
            self.assertEqual(_nearest_palette_label(version), label, version)

    def test_fallback_uses_palette_names_when_available(self):
        with tempfile.TemporaryDirectory() as tmp:
            pack = Path(tmp) / "pack"
            (pack / "scripts").mkdir(parents=True)
            count = write_fallback_block_ids(Path(tmp) / "server", pack, "1.20.80.05")
            text = (pack / "scripts" / "block_ids.js").read_text(encoding="utf-8")
            self.assertIn("runtime_palette_names", text)
            self.assertIn("minecraft:stone", text)
            self.assertGreater(count, 1000)

    def test_behavior_pack_defers_structure_capability(self):
        # Static guard: the capability test must include a tick-based retry and
        # a unique id per attempt, otherwise 1.21.x EngineError persists.
        js = (HERE / "behavior_pack/scripts/main.js").read_text(encoding="utf-8")
        self.assertIn("system.currentTick + 100", js)
        self.assertIn('"bmcbl:capability_test_" + attempt', js)
        self.assertIn("attempts: attempt", js)


if __name__ == "__main__":
    unittest.main()

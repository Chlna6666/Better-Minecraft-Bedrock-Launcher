"""Regression tests for public release-note selection and grouping."""
import importlib.util
from pathlib import Path
import sys
import unittest

module_path = Path(__file__).resolve().parents[1] / "release_notes.py"
spec = importlib.util.spec_from_file_location("release_notes", module_path)
release_notes = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = release_notes
spec.loader.exec_module(release_notes)


class ReleaseNotesTests(unittest.TestCase):
    def test_public_changes_and_internal_scopes(self):
        for subject in ("feat(capes): add cape previews", "fix(map-viewer): correct materials",
                        "perf(gpui-3d): reuse GPU buffers"):
            self.assertIsNotNone(release_notes.parse_commit("abc1234", subject))
        for subject in ("docs: update README", "docs(changelog): archive release",
                        "test(world3d): add audits", "style(gpui): rustfmt",
                        "ci: change runner", "chore(bloader): rebuild DLL",
                        "refactor(models): internal API changes", "fix(ci): correct workflow", "fix(build): fix CI compilation",
                        "feat(bench): add benchmark", "unstructured note"):
            self.assertIsNone(release_notes.parse_commit("abc1234", subject))

    def test_compatibility_and_security(self):
        self.assertEqual("Compatibility", release_notes.parse_commit(
            "abc", "build(linux): add RPM support").category)
        self.assertEqual("Compatibility", release_notes.parse_commit(
            "abc", "chore(deps): fix CVE-2026-12345").category)
        self.assertIsNone(release_notes.parse_commit("abc", "docs: update [release-note]"))
        self.assertIsNotNone(release_notes.parse_commit(
            "abc", "refactor(ui): improve accessibility [release-note]"))

    def test_grouping_and_deduplication(self):
        changes = [release_notes.parse_commit(hash_, subject) for hash_, subject in [
            ("a", "perf(world3d): reuse textures"),
            ("b", "perf(world3d): reuse textures"),
            ("c", "perf(world3d): improve uploads"),
            ("d", "perf(world3d): cache geometry"),
            ("e", "feat(capes): support elytra")]]
        summary = release_notes.render_summary(changes, max_items=2)
        self.assertEqual(1, summary.count("reuse textures"))
        self.assertIn("1 additional changes in this module", summary)
        self.assertIn("support elytra", summary)
        self.assertNotIn(chr(96) + "b" + chr(96), summary)

    def test_empty_and_suffix(self):
        self.assertIn("No user-facing changes", release_notes.render_summary([]))
        self.assertEqual("correct layout", release_notes.parse_commit(
            "abc", "fix(ui): correct layout [skip ci]").text)


if __name__ == "__main__":
    unittest.main()

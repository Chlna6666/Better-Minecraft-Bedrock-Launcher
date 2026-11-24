"""Tests for the id-registry garbage collector."""
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


class GarbageCollectorTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.base = self.root / "id-registry"
        (self.base / "objects").mkdir(parents=True)
        (self.base / "evidence").mkdir(parents=True)

    def blob(self, sub: str, content: bytes) -> str:
        digest = hashlib.sha256(content).hexdigest()
        name = digest + ".bin" if sub == "objects" else digest + ".json.zlib"
        (self.base / sub / name).write_bytes(content)
        return digest

    def write_index(self, object_digest: str, evidence_digest: str):
        index = {
            "format_version": 1,
            "versions": {"1.12.0.28": {"registries": {
                "scope": {"object_sha256": object_digest, "evidence_sha256": evidence_digest}
            }}},
        }
        (self.base / "index.json").write_text(json.dumps(index))

    def run_gc(self, apply: bool):
        cmd = [sys.executable, str(HERE / "gc_id_registry.py"), "--catalog", str(self.root)]
        if apply:
            cmd.append("--apply")
        return subprocess.run(cmd, capture_output=True, text=True)

    def test_dry_run_keeps_everything(self):
        obj = self.blob("objects", b"live-object")
        ev = self.blob("evidence", b"live-evidence")
        stale = self.blob("objects", b"stale-object")
        self.write_index(obj, ev)
        result = self.run_gc(apply=False)
        self.assertEqual(result.returncode, 0)
        self.assertTrue((self.base / "objects" / (stale + ".bin")).is_file())
        self.assertIn("WOULD DELETE", result.stdout)

    def test_apply_removes_only_unreferenced_blobs(self):
        obj = self.blob("objects", b"live-object")
        ev = self.blob("evidence", b"live-evidence")
        stale = self.blob("objects", b"stale-object")
        stale_ev = self.blob("evidence", b"stale-evidence")
        self.write_index(obj, ev)
        result = self.run_gc(apply=True)
        self.assertEqual(result.returncode, 0)
        self.assertTrue((self.base / "objects" / (obj + ".bin")).is_file())
        self.assertTrue((self.base / "evidence" / (ev + ".json.zlib")).is_file())
        self.assertFalse((self.base / "objects" / (stale + ".bin")).is_file())
        self.assertFalse((self.base / "evidence" / (stale_ev + ".json.zlib")).is_file())

    def test_hash_mismatch_is_never_deleted(self):
        obj = self.blob("objects", b"live-object")
        ev = self.blob("evidence", b"live-evidence")
        self.write_index(obj, ev)
        corrupt = self.base / "objects" / (("f" * 64) + ".bin")
        corrupt.write_bytes(b"not matching its name")
        result = self.run_gc(apply=True)
        self.assertEqual(result.returncode, 0)
        self.assertTrue(corrupt.is_file())
        self.assertIn("hash mismatch", result.stderr)


if __name__ == "__main__":
    unittest.main()

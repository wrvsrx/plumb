"""Run after cargo build -p plumb --bin plumb --example migrate_resource_links."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parent.parent
PLUMB = os.environ.get("PLUMB_BIN", str(REPO / "target/debug/plumb"))
HELPER = os.environ.get("PLUMB_MIGRATION_HELPER", str(REPO / "target/debug/examples/migrate_resource_links"))


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "notes"
        self.root.mkdir()
        self.preview = self.base / "preview"
        self.note = self.root / "note.plumb"
        self.original = '`image{{头像} `={src {static/图 像.PNG}}}\r\n`file{Manual `={src static/manual.pdf}}\r\n`file{Audio `={src static/song.MP3}}\r\n'
        self.note.write_bytes(self.original.encode())

    def command(self, *args):
        return subprocess.run([sys.executable, str(REPO / "scripts/migrate-resource-links.py"),
                               "--plumb", PLUMB, "--helper", HELPER, *map(str, args)], capture_output=True, text=True)

    def make_preview(self):
        result = self.command("--root", self.root, "--output", self.preview)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_preview_apply_exclusions_backups_and_idempotence(self):
        for excluded in [".nsfw", ".git", ".stversions"]:
            directory = self.root / excluded
            directory.mkdir()
            (directory / "invalid.plumb").write_text("`broken{")
        (self.root / "outside.plumb").symlink_to(self.root / ".nsfw/invalid.plumb")
        self.make_preview()
        self.assertEqual(self.note.read_bytes(), self.original.encode())
        plan = json.loads((self.preview / "plan.json").read_text())
        self.assertEqual(list(plan["inventory"]), ["note.plumb"])
        self.assertEqual(plan["changes"][0]["references"], 3)
        self.assertEqual((self.preview / "before/note.plumb").read_bytes(), self.original.encode())
        result = self.command("--apply", self.preview)
        self.assertEqual(result.returncode, 0, result.stderr)
        after = self.note.read_bytes()
        self.assertEqual(after.count(b"`+{embed}"), 2)
        self.assertEqual(after.count(b"`->{"), 3)
        self.assertIn(b"\r\n", after)
        self.assertNotIn(b"`={src", after)
        second = self.base / "second"
        result = self.command("--root", self.root, "--output", second)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads((second / "plan.json").read_text())["changes"], [])

    def test_stale_plan_refuses_all_writes(self):
        self.make_preview()
        changed = self.original + "Concurrent edit\n"
        self.note.write_bytes(changed.encode())
        result = self.command("--apply", self.preview)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("notes changed", result.stderr)
        self.assertEqual(self.note.read_bytes(), changed.encode())

    def test_modified_preview_refuses_all_writes(self):
        self.make_preview()
        (self.preview / "after/note.plumb").write_text("changed preview\n")
        result = self.command("--apply", self.preview)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.note.read_bytes(), self.original.encode())

    def test_invalid_input_creates_no_plan_and_changes_no_notes(self):
        (self.root / "broken.plumb").write_text("`file{no-source}\n")
        result = self.command("--root", self.root, "--output", self.preview)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.preview.exists())
        self.assertEqual(self.note.read_bytes(), self.original.encode())

    def test_old_tool_is_rejected_before_preview(self):
        old_tool = self.base / "old-plumb"
        old_tool.write_text('#!/usr/bin/env python3\nimport json\nprint(json.dumps({"blocks": [{"c": [{"t": "Span"}]}, {"c": [{"t": "Span"}]}]}))\n')
        old_tool.chmod(0o700)
        result = self.command("--root", self.root, "--output", self.preview, "--plumb", old_tool)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("update it before migrating", result.stderr)
        self.assertFalse(self.preview.exists())
        self.assertEqual(self.note.read_bytes(), self.original.encode())


if __name__ == "__main__":
    unittest.main()

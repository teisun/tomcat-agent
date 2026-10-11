"""Offline CLI regressions; run with a Python environment containing PyYAML.

python3 -m unittest discover -s tests -p test_script_tools.py -v
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ScriptToolsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="tomcat-script-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def invoke(self, script, *arguments, env=None):
        return subprocess.run(
            [sys.executable, str(ROOT / script), *map(str, arguments)],
            cwd=self.root,
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )

    def test_creator_counts_utf8_header_bytes_at_the_boundary(self):
        skill = self.root / "sample"
        skill.mkdir()
        prefix = "---\nname: sample\ndescription: usable\n# " + "界" * 1000
        suffix = "\n---\n"
        padding = 4096 - len((prefix + suffix).encode("utf-8"))
        for extra in (0, 1):
            with self.subTest(extra_bytes=extra):
                header = prefix + "a" * (padding + extra) + suffix
                self.assertEqual(len(header.encode("utf-8")), 4096 + extra)
                (skill / "SKILL.md").write_text(header + "Useful instructions.\n", encoding="utf-8")
                result = self.invoke("assets/skills/skill-creator/scripts/quick_validate.py", skill)
                if extra == 0:
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                else:
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("4096 UTF-8 bytes", result.stdout)

    def test_creator_description_limit_counts_utf8_bytes(self):
        skill = self.root / "sample"
        skill.mkdir()
        for extra in (0, 1):
            with self.subTest(extra_bytes=extra):
                description = "界" * 341 + "a" * (1 + extra)
                self.assertEqual(len(description.encode("utf-8")), 1024 + extra)
                text = f"---\nname: sample\ndescription: {description}\n---\nInstructions.\n"
                self.assertLess(len(text.encode("utf-8")), 4096)
                (skill / "SKILL.md").write_text(text, encoding="utf-8")
                result = self.invoke("assets/skills/skill-creator/scripts/quick_validate.py", skill)
                self.assertEqual(result.returncode, 0 if extra == 0 else 1, result.stdout + result.stderr)

    def test_cache_metrics_read_current_wire_and_legacy_error_fields(self):
        diag = self.root / "diag.log"
        diag.write_text("phase=llm_usage prompt_tokens=100 cache_read_tokens=50\n", encoding="utf-8")
        events = self.root / "events.jsonl"
        for field in ("isError", "is_error"):
            with self.subTest(field=field):
                events.write_text(
                    "".join(
                        json.dumps({"event": {"type": "tool_execution_end", field: failed}}) + "\n"
                        for failed in (True, False)
                    ),
                    encoding="utf-8",
                )
                result = self.invoke(
                    "scripts/analyze_prompt_cache_baseline.py",
                    "--diag", diag, "--events", events, "--allow-short",
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(
                    json.loads(result.stdout)["tool_calls"],
                    {"total": 2, "rejected": 1, "rejection_rate": 0.5},
                )

    def replay(self, parts):
        transcript = self.root / "session.jsonl"
        transcript.write_text(
            json.dumps({
                "type": "message", "id": "user-1",
                "message": {"role": "user", "content": parts},
            }) + "\n",
            encoding="utf-8",
        )
        return self.invoke(
            "scripts/replay-turn-payload.py", transcript,
            "--turn-number", "1", "--model", "gpt-5-fixture",
            "--base-url", "https://example.invalid", "--dry-run",
            "--env-file", self.root / "absent.env",
            "--api-key-env", "TOMCAT_REPLAY_TEST_KEY",
            env={**os.environ, "TOMCAT_REPLAY_TEST_KEY": "fixture-only"},
        )

    def test_replay_rejects_unresolved_stored_media_instead_of_dropping_it(self):
        for kind in ("input_image_ref", "input_file"):
            with self.subTest(kind=kind):
                result = self.replay([
                    {"type": "input_text", "text": "Inspect the attachment."},
                    {"type": kind, "blob_sha": "a" * 64, "mime_type": "image/png"},
                ])
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("attachment", result.stderr.lower())
                self.assertIn("reference", result.stderr.lower())
                self.assertNotIn('"request_shape"', result.stdout)

    def test_replay_keeps_legacy_inline_media_and_redacts_bytes(self):
        encoded = "c2VudGluZWwtcGl4ZWxz"
        result = self.replay([
            {"type": "input_text", "text": "Inspect both."},
            {"type": "input_image", "mime_type": "image/png", "image_b64": encoded},
            {"type": "input_file", "filename": "sample.pdf", "file_b64": encoded},
        ])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout)["request_shape"]["shape"],
            ["message:user[input_text,input_image,input_file]"],
        )
        self.assertNotIn(encoded, result.stdout)
        self.assertIn("redacted", result.stdout)


if __name__ == "__main__":
    unittest.main()

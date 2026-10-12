"""Tests for tools/citation-harness.py, run as a subprocess the way CI and e2e tests use it."""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "citation-harness.py"
A = "http://127.0.0.1:41001/rivers"
B = "http://127.0.0.1:41001/lakes"

GOOD = f"""The Nile is about 6,650 km long [Rivers]({A}).
Lake Baikal is the deepest lake on Earth [Lakes]({B}).
"""
UNCITED = f"""The Nile is about 6,650 km long [Rivers]({A}).
Lake Baikal holds about a fifth of the world's fresh surface water.
"""


def harness(*args, stdin=None):
    done = subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
    )
    report = json.loads(done.stdout) if done.stdout else None
    return done.returncode, report, done.stderr


class CitationHarnessTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def file(self, name, text):
        path = Path(self.tmp.name) / name
        path.write_text(text, encoding="utf-8")
        return str(path)

    def test_fully_cited_answer_reports_zero_percent(self):
        code, report, _ = harness(
            "--answer", self.file("good.md", GOOD), "--visited", A, "--visited", B,
            "--max-uncited-ratio", "0",
        )
        self.assertEqual(code, 0)
        self.assertEqual(report["claims"], 2)
        self.assertEqual(report["uncited_ratio"], 0.0)

    def test_uncited_claim_is_measured_and_fails_a_zero_threshold(self):
        visited = self.file("visited.json", json.dumps([{"tab_id": "t", "url": A}, {"url": B}]))
        code, report, _ = harness("--answer", "-", "--visited-file", visited, stdin=UNCITED)
        self.assertEqual(code, 0, "measuring alone never fails")
        self.assertEqual(report["uncited"], 1)
        self.assertEqual(report["uncited_ratio"], 0.5)
        self.assertIn("fresh surface water", report["uncited_claims"][0])
        code, _, _ = harness(
            "--answer", "-", "--visited-file", visited, "--max-uncited-ratio", "0", stdin=UNCITED
        )
        self.assertEqual(code, 1)

    def test_link_to_an_unvisited_page_is_uncited(self):
        code, report, _ = harness("--answer", self.file("good.md", GOOD), "--visited", A)
        self.assertEqual(code, 0)
        self.assertEqual(report["uncited"], 1)

    def test_unreadable_input_is_exit_2(self):
        code, _, err = harness("--answer", str(Path(self.tmp.name) / "missing.md"))
        self.assertEqual(code, 2)
        self.assertIn("cannot read input", err)


if __name__ == "__main__":
    unittest.main()

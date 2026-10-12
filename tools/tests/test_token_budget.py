"""Tests for tools/token-budget.py (hyphenated file name, so it is loaded by path)."""

import contextlib
import copy
import importlib.util
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "token-budget.py"
spec = importlib.util.spec_from_file_location("token_budget", SCRIPT)
tb = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = tb
spec.loader.exec_module(tb)

REAL = tb.PROFILE_DIR / "browser-agent.open.json"


def run(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = tb.main(list(argv))
    return code, out.getvalue(), err.getvalue()


class TokenBudgetTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.real = json.loads(REAL.read_text(encoding="utf-8"))

    def write(self, name, payload):
        path = Path(self.tmp.name) / f"{name}.open.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        return path

    def test_real_profiles_pass_the_10k_budget(self):
        code, out, _ = run("--json")
        self.assertEqual(code, 0, out)
        report = json.loads(out)
        self.assertEqual(report["limit"], 10_000)
        self.assertEqual(set(report["profiles"]), {"browser-agent", "ultrabrowse"})
        for name, result in report["profiles"].items():
            self.assertLessEqual(result["total"], 10_000, name)
            # A real profile always ships a prompt and tools; zero means the payload was not read.
            self.assertGreater(result["system"], 0, name)
            self.assertGreater(result["tools"], 0, name)

    def test_inflated_profile_fails_with_nonzero_exit(self):
        inflated = copy.deepcopy(self.real)
        for tool in inflated["tools"]:
            tool["description"] += " Extra guidance that bloats the prompt." * 200
        code, out, _ = run("--payload", str(self.write("inflated", inflated)))
        self.assertEqual(code, 1)
        self.assertIn("inflated:", out)
        self.assertIn("OVER BUDGET", out)

    def test_a_single_oversized_system_part_also_fails(self):
        inflated = copy.deepcopy(self.real)
        inflated["system_prompt_parts"].append("x" * 31_000)
        code, out, _ = run("--payload", str(self.write("long-prompt", inflated)))
        self.assertEqual(code, 1, out)

    def test_limit_boundary_is_inclusive(self):
        total = tb.measure(self.real)["total"]
        path = str(self.write("edge", self.real))
        self.assertEqual(run("--payload", path, "--limit", str(total))[0], 0)
        self.assertEqual(run("--payload", path, "--limit", str(total - 1))[0], 1)

    def test_estimate_rounds_up_partial_tokens(self):
        self.assertEqual(tb.estimate(""), 0)
        self.assertEqual(tb.estimate("a"), 1)
        self.assertEqual(tb.estimate("abcd"), 2)

    def test_non_ascii_bytes_count_as_whole_tokens(self):
        # Byte-level BPE may spend a token per byte on Hangul; 3 bytes per syllable here.
        self.assertEqual(tb.estimate("안녕"), 6)
        self.assertEqual(tb.estimate("abc안"), 4)

    def test_unusable_payload_is_exit_2(self):
        broken = Path(self.tmp.name) / "broken.open.json"
        broken.write_text("{}", encoding="utf-8")
        code, _, err = run("--payload", str(broken))
        self.assertEqual(code, 2)
        self.assertIn("cannot measure", err)
        code, _, _ = run("--payload", str(Path(self.tmp.name) / "missing.open.json"))
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()

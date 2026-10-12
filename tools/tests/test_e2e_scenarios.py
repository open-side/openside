import importlib.util
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("e2e_scenarios", ROOT / "tools/e2e-scenarios.py")
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


class ScenarioRunnerTest(unittest.TestCase):
    def source(self, root, target, names):
        path = root / "crates/fixture-e2e/tests" / f"{target}.rs"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(f"#[tokio::test]\nasync fn {name}() {{}}" for name in names))

    def test_one_cargo_invocation_per_crate_exactly_excludes_unlisted_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.source(root, "first", ["selected", "selected_extra"])
            self.source(root, "second", ["other", "not_selected"])
            rows = [RUNNER.Scenario("1 First", "fixture-e2e", "first", "selected"),
                    RUNNER.Scenario("2 Second", "fixture-e2e", "second", "other")]
            commands = RUNNER.cargo_commands(rows, root)
        self.assertEqual(list(commands), ["fixture-e2e"])
        command = commands["fixture-e2e"]
        self.assertEqual(command[:6], ["cargo", "test", "-j", "8", "-p", "fixture-e2e"])
        self.assertEqual(command[6:10], ["--test", "first", "--test", "second"])
        self.assertIn("--exact", command)
        self.assertLess(command.index("--no-fail-fast"), command.index("--"))
        self.assertEqual(command[command.index("--skip"):], ["--skip", "not_selected", "--skip", "selected_extra"])

    def test_missing_listed_test_fails_before_cargo_can_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.source(root, "first", ["actual"])
            row = RUNNER.Scenario("1 Missing", "fixture-e2e", "first", "missing")
            with self.assertRaisesRegex(ValueError, "listed test does not exist"):
                RUNNER.cargo_commands([row], root)

    def test_duplicate_names_across_targets_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.source(root, "first", ["same"])
            self.source(root, "second", ["same"])
            rows = [RUNNER.Scenario("1", "fixture-e2e", "first", "same"),
                    RUNNER.Scenario("2", "fixture-e2e", "second", "same")]
            with self.assertRaisesRegex(ValueError, "ambiguous test names"):
                RUNNER.cargo_commands(rows, root)

    def test_printed_skips_and_ignored_tests_never_become_pass(self):
        output = "test skipped ... SKIP: missing Chromium\nok\ntest ignored_case ... ignored\ntest real ... ok\n"
        self.assertEqual(RUNNER.outcomes(output), {"skipped": "FAIL", "ignored_case": "FAIL", "real": "PASS"})
        before_name = "SKIP: no reachable compositor IPC\ntest skipped ... ok\ntest real ... ok\n"
        self.assertEqual(RUNNER.outcomes(before_name), {"skipped": "FAIL", "real": "PASS"})

    def test_failed_and_unfinished_tests_are_not_successes(self):
        output = "test failed ... \nthread panicked\nFAILED\ntest interrupted ... \n"
        self.assertEqual(RUNNER.outcomes(output), {"failed": "FAIL"})

    def test_manifest_has_ten_scenarios_and_every_real_test_exists(self):
        rows = RUNNER.scenarios()
        self.assertEqual({int(row.label.split()[0]) for row in rows}, set(range(1, 11)))
        self.assertEqual(list(RUNNER.cargo_commands(rows)), ["openside-e2e"])

    def test_empty_or_duplicate_manifest_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "manifest.md"
            path.write_text("# Empty\n")
            with self.assertRaisesRegex(ValueError, "empty or missing"):
                RUNNER.scenarios(path)
            path.write_text("| Scenario | Crate | Target | Test |\n| --- | --- | --- | --- |\n"
                            "| 1 | fixture-e2e | first | selected |\n"
                            "| 1 | fixture-e2e | first | selected |\n")
            with self.assertRaisesRegex(ValueError, "listed more than once"):
                RUNNER.scenarios(path)


if __name__ == "__main__":
    unittest.main()

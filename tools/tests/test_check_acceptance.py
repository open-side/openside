"""Tests for tools/check-acceptance.py (the file name has a hyphen, so it is loaded by path)."""

import contextlib
import csv
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "check-acceptance.py"
spec = importlib.util.spec_from_file_location("check_acceptance", SCRIPT)
ca = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = ca
spec.loader.exec_module(ca)

HEADER = ca.HEADER


class Repo:
    """A throwaway repo root plus helpers to write files, csv and evidence."""

    def __init__(self, test: unittest.TestCase):
        tmp = tempfile.TemporaryDirectory()
        test.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.results = self.root / "results"
        self.results.mkdir()
        self.csv = self.root / "acceptance.csv"

    def write(self, rel: str, text: str, executable: bool = False) -> None:
        p = self.root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
        if executable:
            p.chmod(0o755)

    def rows(self, rows: list[list[str]], header: list[str] | None = None) -> None:
        with self.csv.open("w", newline="") as fh:
            w = csv.writer(fh)
            w.writerow(header or HEADER)
            w.writerows(rows)

    def run(self, *extra: str, results: bool = False) -> tuple[int, str]:
        argv = ["--root", str(self.root), "--csv", str(self.csv), *extra]
        if results:
            argv += ["--results", str(self.results)]
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = ca.main(argv)
        return code, buf.getvalue()


def row(rid: str, status: str, test_path: str = "", note: str = "") -> list[str]:
    return [rid, "01", "", "criterion", test_path, status, note]


class HeaderAndStatus(unittest.TestCase):
    def test_wrong_header_is_rejected(self):
        r = Repo(self)
        r.rows([row("X-1", "unverified")], header=["id", "source", "status"])
        code, out = r.run()
        self.assertEqual(code, 1)
        self.assertIn("ERROR 1 -: header must be exactly", out)

    def test_unknown_status_is_an_error_naming_row_and_id(self):
        r = Repo(self)
        r.rows([row("X-1", "unverified"), row("X-2", "done")])
        code, out = r.run()
        self.assertEqual(code, 1)
        self.assertIn("ERROR 3 X-2: invalid status 'done'", out)

    def test_counts_and_clean_exit(self):
        r = Repo(self)
        r.rows([row("X-1", "unverified"), row("X-2", "out-of-scope"), row("X-3", "blocked-external")])
        code, out = r.run()
        self.assertEqual(code, 0, out)
        self.assertRegex(out, r"unverified\s+1")
        self.assertRegex(out, r"out-of-scope\s+1")
        self.assertIn("0 error(s)", out)

    def test_json_output_is_machine_readable(self):
        r = Repo(self)
        r.rows([row("X-1", "unverified"), row("X-2", "bogus")])
        code, out = r.run("--json")
        data = json.loads(out)
        self.assertEqual(code, 1)
        self.assertFalse(data["ok"])
        self.assertEqual(data["errors"][0]["id"], "X-2")
        self.assertEqual(data["counts"]["unverified"], 1)


class DefinitionDetection(unittest.TestCase):
    def check(self, r: Repo, test_path: str) -> tuple[int, str]:
        r.rows([row("T-1", "pass", test_path)])
        return r.run()

    def test_rust_test_attributes_and_window(self):
        r = Repo(self)
        r.write(
            "crates/a/src/lib.rs",
            "#[test]\nfn plain() {}\n\n"
            '#[tokio::test(flavor = "multi_thread")]\n#[ignore_not_really]\n\nasync fn tokio_case() {}\n'
            "#[test_case(1)]\nfn cased() {}\n"
            "fn helper() {}\n"
            "#[test]\n#[a]\n#[b]\n#[c]\n#[d]\n#[e]\n#[f]\nfn too_far() {}\n"
            '// #[test]\nfn commented_attr() {}\n'
            'const S: &str = "#[test]";\nfn string_attr() {}\n',
        )
        for name in ("plain", "tokio_case", "cased"):
            code, out = self.check(r, f"crates/a/src/lib.rs::{name}")
            self.assertEqual(code, 0, f"{name}: {out}")
        for name in ("helper", "too_far", "commented_attr", "string_attr", "missing"):
            code, out = self.check(r, f"crates/a/src/lib.rs::{name}")
            self.assertEqual(code, 1, name)
            self.assertIn("no test definition found", out)

    def test_rust_proptest_block_and_char_literals(self):
        r = Repo(self)
        r.write(
            "crates/a/src/lib.rs",
            "fn outside() {}\n"
            "proptest! {\n"
            "    #![proptest_config(ProptestConfig::with_cases(8))]\n"
            "    fn inside(x in 0u8..10) { let c = '}'; let s = \"}\"; assert!(c != 'x' && !s.is_empty()); }\n"
            "    fn also_inside(x in 0u8..10) {}\n"
            "}\n"
            "fn after_block() {}\n",
        )
        for name in ("inside", "also_inside"):
            self.assertEqual(self.check(r, f"crates/a/src/lib.rs::{name}")[0], 0, name)
        for name in ("outside", "after_block"):
            self.assertEqual(self.check(r, f"crates/a/src/lib.rs::{name}")[0], 1, name)

    def test_rust_async_fn_and_pub(self):
        r = Repo(self)
        r.write("crates/a/tests/t.rs", "#[tokio::test]\npub async fn it_works() {}\n")
        self.assertEqual(self.check(r, "crates/a/tests/t.rs::it_works")[0], 0)

    def test_python_param_suffix_is_stripped_and_async_accepted(self):
        r = Repo(self)
        r.write("agent/tests/test_x.py", "def test_a(x):\n    pass\n\nasync def test_b():\n    pass\n")
        self.assertEqual(self.check(r, "agent/tests/test_x.py::test_a[True]")[0], 0)
        self.assertEqual(self.check(r, "agent/tests/test_x.py::test_b")[0], 0)
        code, out = self.check(r, "agent/tests/test_x.py::test_c[1]")
        self.assertEqual(code, 1)
        self.assertIn("no test definition found", out)

    def test_typescript_titles_and_describe_chain(self):
        r = Repo(self)
        r.write(
            "web/src/a.test.ts",
            "describe('outer suite', () => {\n  it('does the thing', () => {});\n"
            "  test(\"other \\\"quoted\\\" thing\", () => {});\n  it.skip('skipped one', () => {});\n});\n",
        )
        ok = [
            "does the thing",
            "outer suite > does the thing",
            'other \\"quoted\\" thing',
        ]
        for name in ok:
            self.assertEqual(self.check(r, f"web/src/a.test.ts::{name}")[0], 0, name)
        for name in ("skipped one", "wrong suite > does the thing", "outer suite"):
            self.assertEqual(self.check(r, f"web/src/a.test.ts::{name}")[0], 1, name)

    def test_missing_file_and_bare_path_executability(self):
        r = Repo(self)
        r.write("tools/run.sh", "#!/bin/sh\n", executable=True)
        r.write("tools/plain.sh", "#!/bin/sh\n")
        self.assertEqual(self.check(r, "tools/run.sh")[0], 0)
        code, out = self.check(r, "tools/plain.sh")
        self.assertEqual(code, 1)
        self.assertIn("executable", out)
        code, out = self.check(r, "tools/nope.sh")
        self.assertIn("file does not exist", out)

    def test_pass_row_needs_test_path_and_entries_are_split(self):
        r = Repo(self)
        r.write("crates/a/src/lib.rs", "#[test]\nfn one() {}\n#[test]\nfn two() {}\n")
        self.assertEqual(self.check(r, "")[0], 1)
        self.assertEqual(self.check(r, "crates/a/src/lib.rs::one ;  crates/a/src/lib.rs::two;")[0], 0)
        code, out = self.check(r, "crates/a/src/lib.rs::one; crates/a/src/lib.rs::three")
        self.assertEqual(code, 1)
        self.assertIn("three: no test definition found", out)


CARGO_LOG = """\
   Compiling whatever v0.1.0
     Running unittests src/lib.rs (target/debug/deps/openside_policy-6ae58775a560a068)

running 2 tests
test tests::default_mode_is_guard ... ok
test glob::tests::bad_case ... FAILED
test tests::slow_case ... ignored, takes too long

test result: FAILED. 1 passed; 1 failed; 1 ignored

     Running unittests src/main.rs (target/debug/deps/openside_e2e-1111111111111111)
test unit_in_e2e_crate ... ok

     Running tests/daemon.rs (target/debug/deps/daemon-3c4d)
test sidecar_case ... ok
test only_in_daemon ... ok

     Running tests/other.rs (target/debug/deps/other-5e6f)
test sidecar_case ... FAILED
test shared_name ... ok

   Doc-tests openside_policy
test src/lib.rs - doc (line 3) ... ok
"""


class CargoMapping(unittest.TestCase):
    def setUp(self):
        self.r = Repo(self)
        self.r.write("crates/openside-policy/src/lib.rs", "#[test]\nfn default_mode_is_guard() {}\n#[test]\nfn bad_case() {}\n#[test]\nfn slow_case() {}\n")
        self.r.write("crates/openside-e2e/src/main.rs", "#[test]\nfn unit_in_e2e_crate() {}\n")
        self.r.write("crates/openside-e2e/tests/daemon.rs", "#[test]\nfn sidecar_case() {}\n#[test]\nfn only_in_daemon() {}\n")
        self.r.write("crates/openside-e2e/tests/other.rs", "#[test]\nfn sidecar_case() {}\n#[test]\nfn shared_name() {}\n")
        self.r.write("crates/other-crate/src/lib.rs", "#[test]\nfn default_mode_is_guard() {}\n")
        (self.r.results / "cargo-test.log").write_text(CARGO_LOG)

    def verdict(self, entry: str) -> tuple[int, str]:
        self.r.rows([row("C-1", "pass", entry)])
        return self.r.run(results=True)

    def test_unit_ref_matches_crate_binary_with_module_prefix(self):
        code, out = self.verdict("crates/openside-policy/src/lib.rs::default_mode_is_guard")
        self.assertEqual(code, 0, out)

    def test_unit_ref_of_other_crate_does_not_borrow_results(self):
        code, out = self.verdict("crates/other-crate/src/lib.rs::default_mode_is_guard")
        self.assertEqual(code, 1)
        self.assertIn("not found in results", out)

    def test_failed_and_ignored_are_not_pass(self):
        code, out = self.verdict("crates/openside-policy/src/lib.rs::bad_case")
        self.assertEqual(code, 1)
        self.assertIn("not passed (cargo reports FAILED)", out)
        code, out = self.verdict("crates/openside-policy/src/lib.rs::slow_case")
        self.assertEqual(code, 1)
        self.assertIn("ignored", out)

    def test_crate_openside_e2e_is_not_confused_with_integration_binary_e2e(self):
        code, out = self.verdict("crates/openside-e2e/src/main.rs::unit_in_e2e_crate")
        self.assertEqual(code, 0, out)
        # `daemon` integration results must not satisfy a unit ref of the same crate.
        code, out = self.verdict("crates/openside-e2e/src/main.rs::only_in_daemon")
        self.assertEqual(code, 1)

    def test_integration_ref_uses_its_own_section_only(self):
        code, _ = self.verdict("crates/openside-e2e/tests/daemon.rs::sidecar_case")
        self.assertEqual(code, 0)
        code, out = self.verdict("crates/openside-e2e/tests/other.rs::sidecar_case")
        self.assertEqual(code, 1)
        self.assertIn("FAILED", out)
        code, out = self.verdict("crates/openside-e2e/tests/daemon.rs::shared_name")
        self.assertEqual(code, 1, "shared_name only passed in other.rs")

    def test_doc_tests_are_not_attributed_to_previous_section(self):
        self.r.write("crates/openside-policy/src/lib.rs", "#[test]\nfn doc() {}\n")
        code, _ = self.verdict("crates/openside-policy/src/lib.rs::doc")
        self.assertEqual(code, 1)


class JunitMapping(unittest.TestCase):
    XML = (
        '<?xml version="1.0"?><testsuites><testsuite>'
        '<testcase classname="tests.test_runtime" name="test_auth[True]"/>'
        '<testcase classname="tests.test_runtime" name="test_auth[False]"><failure message="x"/></testcase>'
        '<testcase classname="tests.test_runtime" name="test_all[1]"/>'
        '<testcase classname="tests.test_runtime" name="test_all[2]"/>'
        '<testcase classname="tests.providers.test_sources" name="test_plain"/>'
        '<testcase classname="tests.test_runtime" name="test_skipped"><skipped/></testcase>'
        '<testcase classname="tests.test_runtime.TestKlass" name="test_method"/>'
        "</testsuite></testsuites>"
    )

    def setUp(self):
        self.r = Repo(self)
        self.r.write(
            "agent/tests/test_runtime.py",
            "def test_auth(p):\n    pass\n\ndef test_all(p):\n    pass\n\ndef test_skipped():\n    pass\n\n"
            "class TestKlass:\n    def test_method(self):\n        pass\n",
        )
        self.r.write("agent/tests/providers/test_sources.py", "def test_plain():\n    pass\n")
        (self.r.results / "pytest.xml").write_text(self.XML)

    def verdict(self, *entries: str) -> tuple[int, str]:
        self.r.rows([row("P-1", "pass", "; ".join(entries))])
        return self.r.run(results=True)

    def test_exact_param_pass_and_fail(self):
        self.assertEqual(self.verdict("agent/tests/test_runtime.py::test_auth[True]")[0], 0)
        code, out = self.verdict("agent/tests/test_runtime.py::test_auth[False]")
        self.assertEqual(code, 1)
        self.assertIn("not passed", out)

    def test_unparametrised_entry_requires_every_parametrization_to_pass(self):
        self.assertEqual(self.verdict("agent/tests/test_runtime.py::test_all")[0], 0)
        code, out = self.verdict("agent/tests/test_runtime.py::test_auth")
        self.assertEqual(code, 1)
        self.assertIn("not passed", out)

    def test_nested_package_class_and_skip(self):
        self.assertEqual(self.verdict("agent/tests/providers/test_sources.py::test_plain")[0], 0)
        self.assertEqual(self.verdict("agent/tests/test_runtime.py::TestKlass::test_method")[0], 0)
        self.assertEqual(self.verdict("agent/tests/test_runtime.py::test_skipped")[0], 1)


class VitestAndScripts(unittest.TestCase):
    def test_vitest_suffix_mapping_and_status(self):
        r = Repo(self)
        r.write(
            "web/src/a.test.ts",
            "describe('suite', () => { it('works', () => {}); it('breaks', () => {}); });\n",
        )
        payload = {
            "testResults": [
                {
                    "name": "/ci/checkout/web/src/a.test.ts",
                    "assertionResults": [
                        {"title": "works", "fullName": "suite works", "ancestorTitles": ["suite"], "status": "passed"},
                        {"title": "breaks", "fullName": "suite breaks", "ancestorTitles": ["suite"], "status": "failed"},
                    ],
                }
            ]
        }
        (r.results / "web.vitest.json").write_text(json.dumps(payload))
        r.rows([row("W-1", "pass", "web/src/a.test.ts::suite > works")])
        self.assertEqual(r.run(results=True)[0], 0)
        r.rows([row("W-1", "pass", "web/src/a.test.ts::breaks")])
        code, out = r.run(results=True)
        self.assertEqual(code, 1)
        self.assertIn("vitest reports non-passed", out)

    def test_scripts_txt_pass_and_fail_lines(self):
        r = Repo(self)
        r.write("tools/ok.sh", "#!/bin/sh\n", executable=True)
        r.write("tools/bad.sh", "#!/bin/sh\n", executable=True)
        r.write("tools/never.sh", "#!/bin/sh\n", executable=True)
        (r.results / "scripts.txt").write_text("PASS tools/ok.sh\nFAIL tools/bad.sh\n")
        r.rows([row("S-1", "pass", "tools/ok.sh; tools/bad.sh; tools/never.sh")])
        code, out = r.run(results=True)
        self.assertEqual(code, 1)
        self.assertNotIn("tools/ok.sh:", out)
        self.assertIn("tools/bad.sh: not passed", out)
        self.assertIn("tools/never.sh: not found in results", out)


class MissingResults(unittest.TestCase):
    def test_missing_results_directory(self):
        r = Repo(self)
        r.write("crates/a/src/lib.rs", "#[test]\nfn one() {}\n")
        r.rows([row("M-1", "pass", "crates/a/src/lib.rs::one")])
        r.results.rmdir()
        code, out = r.run(results=True)
        self.assertEqual(code, 1)
        self.assertIn("does not exist", out)
        self.assertIn("ERROR 2 M-1: crates/a/src/lib.rs::one: not found in results", out)

    def test_empty_results_directory_is_an_error(self):
        r = Repo(self)
        r.rows([row("M-1", "unverified")])
        code, out = r.run(results=True)
        self.assertEqual(code, 1)
        self.assertIn("no recognised evidence files", out)

    def test_pass_rows_without_results_flag_are_static_only(self):
        r = Repo(self)
        r.write("crates/a/src/lib.rs", "#[test]\nfn one() {}\n")
        r.rows([row("M-1", "pass", "crates/a/src/lib.rs::one")])
        self.assertEqual(r.run()[0], 0)


class FinalMode(unittest.TestCase):
    NOTE = "needs hardware; 사용자 할 일: run on a GPU box"

    def many(self, unverified: int, exceptions: int, passes: int, note: str):
        rows = [row(f"U-{i}", "unverified") for i in range(unverified)]
        rows += [row(f"E-{i}", "out-of-scope", note=note) for i in range(exceptions)]
        rows += [row(f"P-{i}", "pass", "crates/a/src/lib.rs::one") for i in range(passes)]
        return rows

    def setUp(self):
        self.r = Repo(self)
        self.r.write("crates/a/src/lib.rs", "#[test]\nfn one() {}\n")

    def test_final_requires_zero_unverified(self):
        self.r.rows(self.many(1, 0, 9, ""))
        code, out = self.r.run("--final")
        self.assertEqual(code, 1)
        self.assertIn("1 unverified row(s) remain", out)

    def test_exception_limit_is_floor_of_ten_percent(self):
        self.r.rows(self.many(0, 2, 18, self.NOTE))  # 20 rows -> limit 2
        self.assertEqual(self.r.run("--final")[0], 0)
        self.r.rows(self.many(0, 2, 17, self.NOTE))  # 19 rows -> limit 1
        code, out = self.r.run("--final")
        self.assertEqual(code, 1)
        self.assertIn("exceed the limit of 1", out)

    def test_exception_rows_need_reason_and_user_action_marker(self):
        for note, fragment in (
            ("no marker here", "needs a reason followed by"),
            ("사용자 할 일: do something", "no reason before"),
            ("blocked by vendor; User action: ", "no action after"),
        ):
            self.r.rows(self.many(0, 1, 19, note))
            code, out = self.r.run("--final")
            self.assertEqual(code, 1, note)
            self.assertIn(fragment, out)
        self.r.rows(self.many(0, 1, 19, "blocked by vendor; user action: ask vendor"))
        self.assertEqual(self.r.run("--final")[0], 0)

    def test_notes_are_not_checked_without_final(self):
        self.r.rows(self.many(1, 1, 8, "no marker"))
        self.assertEqual(self.r.run()[0], 0)


class ListScripts(unittest.TestCase):
    def test_only_bare_entries_of_pass_rows_are_listed_once(self):
        r = Repo(self)
        r.rows(
            [
                row("L-1", "pass", "tools/a.sh; crates/x/src/lib.rs::t"),
                row("L-2", "pass", "tools/a.sh;tools/b.sh"),
                row("L-3", "unverified", "tools/c.sh"),
            ]
        )
        self.assertEqual(ca.bare_script_entries(r.csv), ["tools/a.sh", "tools/b.sh"])
        code, out = r.run("--list-scripts")
        self.assertEqual((code, out.split()), (0, ["tools/a.sh", "tools/b.sh"]))


class RealRepoSmoke(unittest.TestCase):
    def test_script_is_executable(self):
        self.assertTrue(os.access(SCRIPT, os.X_OK))


if __name__ == "__main__":
    unittest.main()

"""The ignored mobile journey must supply real evidence from both phone profiles."""

from __future__ import annotations

import importlib.util
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "qa" / "mobile_remote.py"
SPEC = importlib.util.spec_from_file_location("mobile_remote_gate", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def evidence(transport: str = "relay") -> str:
    test = MODULE.TESTS[transport]
    return (
        f"test {test} ... "
        f"MOBILE_REMOTE {transport} iphone PASS qr_pixels pair\n"
        f"MOBILE_REMOTE {transport} android PASS qr_pixels pair\n"
        "ok\n"
        "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out\n"
    )


def test_mobile_gate_requires_both_profile_markers_and_executed_rust_test():
    assert MODULE.passed(evidence(), 0, "relay")
    assert not MODULE.passed(evidence().replace("MOBILE_REMOTE relay android PASS", "NOT_RUN"), 0, "relay")
    assert not MODULE.passed(evidence().replace("1 passed; 0 failed; 0 ignored", "0 passed; 0 failed; 1 ignored"), 0, "relay")
    assert not MODULE.passed(evidence() + "SKIP: no browser\n", 0, "relay")
    assert not MODULE.passed(evidence(), 1, "relay")
    assert not MODULE.passed(evidence("cloudflare"), 0, "relay")


def test_mobile_gate_records_two_runs_and_keeps_captures_private(tmp_path, monkeypatch):
    calls = []

    class Result:
        stdout = evidence()
        returncode = 0

    def execute(command, **kwargs):
        calls.append((command, kwargs))
        return Result()

    monkeypatch.setattr(MODULE.subprocess, "run", execute)
    assert MODULE.run(tmp_path, 2, "relay") == 0
    assert len(calls) == 2
    for repetition, (command, kwargs) in enumerate(calls, start=1):
        assert command[command.index("-j") + 1] == "2"
        assert "--ignored" in command and "--test-threads=1" in command
        assert kwargs["env"]["OPENSIDE_MOBILE_CAPTURES"] == str(tmp_path / "mobile-remote-captures" / f"run{repetition}")
        assert (tmp_path / f"mobile-remote-relay-run{repetition}.log").is_file()
    suite = MODULE.ET.parse(tmp_path / "mobile-remote-relay.xml").getroot()
    assert suite.attrib["tests"] == "2" and suite.attrib["failures"] == "0"
    assert len(suite.findall("testcase")) == 2


def test_mobile_gate_second_failure_cannot_be_hidden_by_first_pass(tmp_path, monkeypatch):
    results = iter([(evidence(), 0), ("missing execution\n", 0)])

    def execute(*_args, **_kwargs):
        output, code = next(results)
        return MODULE.subprocess.CompletedProcess([], code, stdout=output)

    monkeypatch.setattr(MODULE.subprocess, "run", execute)
    assert MODULE.run(tmp_path, 2, "relay") == 1
    suite = MODULE.ET.parse(tmp_path / "mobile-remote-relay.xml").getroot()
    assert suite.attrib["failures"] == "1"
    assert len(suite.findall("testcase/failure")) == 1

"""Focused gate evidence must come from checks that actually completed."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import sys
from types import ModuleType
from unittest.mock import patch
import xml.etree.ElementTree as ET

import pytest

ROOT = Path(__file__).resolve().parents[2]


def load(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RESPONSIVE = load("responsive_gate", "tools/qa/responsive.py")
# These tests exercise XML aggregation, not the harness's websocket transport.
with patch.dict(sys.modules, {"websockets": ModuleType("websockets")}):
    HEADED = load("headed_gate", "tools/qa/headed_ux.py")

SUCCESS = (
    "     Running tests/responsive.rs (target/debug/deps/responsive-123abc)\n"
    'RESPONSIVE {"experiment":"cold_warm","data":[]}\n'
    "test cold_and_warm_first_browser_action ... ok\n"
    "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out\n"
)


@pytest.mark.parametrize("output, returncode, expected", [
    (SUCCESS, 0, 0),
    (SUCCESS, 1, 1),
    (SUCCESS + "SKIP: Chromium missing\n", 0, 1),
    (SUCCESS.replace("1 passed", "0 passed"), 0, 1),
    (SUCCESS.replace("RESPONSIVE ", ""), 0, 1),
    ("", 0, 1),
])
def test_responsive_runner_rejects_failed_missing_and_skipped_checks(tmp_path, monkeypatch, output, returncode, expected):
    commands = []

    def execute(command, **kwargs):
        commands.append(command)
        return subprocess.CompletedProcess(command, returncode, output)

    monkeypatch.setattr(RESPONSIVE.subprocess, "run", execute)
    assert RESPONSIVE.run(tmp_path, 4) == expected
    command = commands[0]
    assert command[command.index("-j") + 1] == "4"
    assert "--ignored" in command and "--exact" in command
    assert "--test-threads=2" in command
    assert command[command.index("--test") + 1] == "responsive"
    assert (tmp_path / "responsive.log").read_text() == output
    case = ET.parse(tmp_path / "responsive.xml").find("testcase")
    assert case.get("name") == RESPONSIVE.TEST
    assert (case.find("failure") is None) == (expected == 0)


def prewarm_results():
    return [{"check": name, "pass": True, "detail": "observed"} for name in [
        "prewarm prepares exactly one agent tab before a message",
        "prewarm opens no compositor window",
        "prewarm leaves the foreground window focused",
        "active window never changed by the agent",
    ]]


@pytest.mark.parametrize("missing, failed, error", [(False, False, None), (True, False, None), (False, True, None), (False, False, "interrupted")])
def test_headed_junit_requires_every_prewarm_assertion(tmp_path, missing, failed, error):
    results = prewarm_results()
    if missing:
        results.pop()
    if failed:
        results[0]["pass"] = False
    path = tmp_path / "headed-pytest.xml"
    HEADED.write_prewarm_junit(path, results, error, 1.25)
    case = ET.parse(path).find("testcase")
    assert case.get("classname") == "tools.qa.headed_ux"
    assert case.get("name") == "prewarm_checks"
    assert (case.find("failure") is None) == (not missing and not failed and error is None)

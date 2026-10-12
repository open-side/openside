"""Consumer-level schema and one-command real-process benchmark checks."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

import psutil
import pytest
from jsonschema import ValidationError

BENCH = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCH))

from fixtures import TASKS  # noqa: E402
from runner import daemon_process, discover_browser, summarize, validate_result  # noqa: E402


@pytest.mark.parametrize("live_model, orchestration", [(None, "single"), ("openai:fixture", None)])
def test_daemon_configuration_selects_single_only_for_scripts(monkeypatch, live_model, orchestration):
    configurations = []

    def capture_launch(*_args, **kwargs):
        config = Path(kwargs["env"]["XDG_CONFIG_HOME"]) / "openside/config.toml"
        configurations.append(tomllib.loads(config.read_text()))
        raise RuntimeError("launch intercepted")

    monkeypatch.setattr(subprocess, "Popen", capture_launch)
    with pytest.raises(RuntimeError, match="launch intercepted"):
        with daemon_process(Path("unused"), Path("unused"), Path(sys.executable), live_model, "http://127.0.0.1:12345"):
            pytest.fail("The configuration check must not launch a daemon")
    assert configurations[0]["agent"].get("orchestration") == orchestration


def valid_task():
    return {
        "task_id": "fixture", "category": "InteractionTests", "task": "Submit a local form",
        "outcome": "success", "graderVerdict": "pass", "graderReasoning": "Fixture state verified",
        "agentStatus": "success", "sessionId": "test-session",
        "model": {"provider": "scripted", "modelId": "script", "thinkingLevel": "off", "fastMode": False},
        "provider": "scripted", "durationMs": 1, "graderDurationMs": 0,
        "tabCount": 1, "messageCount": 2,
        "artifacts": {"stdout": "stdout.txt", "stderr": "stderr.txt", "cliEvents": "events.jsonl", "trajectory": "trajectory.txt", "result": "result.json"},
        "openside": {"suite": "openside-local-web-v1", "mode": "scripted", "orchestration": "single", "grader": "fixture-state-v1",
                     "steps": 1, "tokens": {"input": 8, "output": 4, "source": "scripted-function-model-estimate"},
                     "browserBytes": 64, "ticketsResolved": 0},
    }


@pytest.mark.parametrize("mutation", [
    lambda result: result.pop("task_id"),
    lambda result: result.update(outcome="looks-good"),
    lambda result: result["openside"].pop("orchestration"),
    lambda result: result["openside"].update(orchestration="unknown"),
    lambda result: result.update(durationMs=-1),
    lambda result: result["openside"].update(browserBytes="unknown"),
    lambda result: result["model"].update(thinkingLevel="unrecorded"),
    lambda result: result["artifacts"].pop("trajectory"),
])
def test_malformed_result_fails_json_schema_validation(mutation):
    result = valid_task()
    validate_result(result)
    mutation(result)
    with pytest.raises(ValidationError):
        validate_result(result)


def test_summary_counts_failures_and_impossible_tasks_without_fabricating_success():
    results = [valid_task() for _ in range(4)]
    for result, outcome, verdict in zip(
        results, ("success", "failure", "impossible", "error"), ("pass", "fail", "impossible", "error"), strict=True
    ):
        result.update(outcome=outcome, graderVerdict=verdict)
        validate_result(result)
    summary = summarize(results, results[0]["model"], 10)
    validate_result(summary)
    assert summary["passed"] == summary["failed"] == summary["impossible"] == summary["errors"] == 1
    assert summary["passRate"] == 0.25
    assert summary["passRateExcludingImpossible"] == pytest.approx(1 / 3)
    assert summary["openside"]["tokens"] == {"input": 32, "output": 16, "source": "scripted-function-model-estimate"}
    assert summary["openside"]["orchestration"] == "single"
    summary["timestamp"] = "not-a-timestamp"
    with pytest.raises(ValidationError):
        validate_result(summary)


def test_live_result_and_summary_record_front_orchestration():
    result = valid_task()
    result["openside"].update(mode="live", orchestration="front")
    result["openside"]["tokens"]["source"] = "provider-usage"
    validate_result(result)
    summary = summarize([result], result["model"], 10)
    validate_result(summary)
    assert summary["openside"]["mode"] == "live"
    assert summary["openside"]["orchestration"] == "front"


def test_one_command_reproduces_scripted_suite_and_validates_aside_schema(tmp_path):
    if os.name != "posix":
        pytest.skip("The daemon benchmark uses Unix sockets; Windows support is not implemented")
    try:
        browser = discover_browser()
    except RuntimeError as reason:
        pytest.skip(str(reason))
    result_dir = tmp_path / "results"
    before = {(process.pid, process.create_time()) for process in psutil.process_iter()}
    environment = os.environ.copy()
    environment.pop("OPENSIDE_LIVE_MODEL", None)
    environment["OPENSIDE_BROWSER"] = str(browser)
    completed = subprocess.run(
        [str(BENCH / "run.sh"), "--output", str(result_dir)], cwd=tmp_path,
        env=environment, capture_output=True, text=True, timeout=300,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    assert "5/5 passed; validated results:" in completed.stdout
    summary = json.loads((result_dir / "run_summary.json").read_text())
    validate_result(summary)
    assert summary["total"] == summary["ran"] == summary["passed"] == len(TASKS)
    assert summary["failed"] == summary["errors"] == summary["skipped"] == 0
    assert summary["openside"]["mode"] == "scripted"
    assert summary["openside"]["orchestration"] == "single"
    assert summary["openside"]["ticketsResolved"] == 1
    totals = {"steps": 0, "browserBytes": 0, "input": 0, "output": 0}
    for task in TASKS:
        directory = result_dir / task.task_id
        result = json.loads((directory / "result.json").read_text())
        validate_result(result)
        assert result["graderVerdict"] == "pass"
        assert result["outcome"] == "success"
        assert result["task_id"] == task.task_id
        assert result["model"] == {"provider": "scripted", "modelId": "script", "thinkingLevel": "off", "fastMode": False}
        assert result["openside"]["browserBytes"] > 0
        assert result["openside"]["orchestration"] == "single"
        assert result["durationMs"] > 0
        for artifact in result["artifacts"].values():
            assert (directory / artifact).is_file()
        events = [json.loads(line) for line in (directory / "events.jsonl").read_text().splitlines()]
        started = [event for event in events if event["kind"] == "tool.started"]
        ended = [event for event in events if event["kind"] == "tool.ended"]
        assert len(started) == result["openside"]["steps"] >= len(task.codes)
        started_ids = {event["tool_call_id"] for event in started}
        ended_ids = {event["tool_call_id"] for event in ended}
        assert started_ids <= ended_ids
        # Browser downloads emit notifications, not an additional agent tool invocation.
        for event in ended:
            if event["tool_call_id"] not in started_ids:
                assert task.task_id == "download"
                assert event["result"] == {"browser_event": "download.finished"}
                assert any(
                    earlier["kind"] == "tool.updated"
                    and earlier["tool_call_id"] == event["tool_call_id"]
                    and earlier["update"]["browser_event"] == "download.started"
                    and earlier["seq"] < event["seq"]
                    for earlier in events
                )
        assert not any(event["is_error"] for event in ended)
        assert any(event["kind"] == "assistant.final" and event["text"] == f"Completed {task.task_id}." for event in events)
        for field, event_field in (("input", "input_tokens"), ("output", "output_tokens")):
            count = sum(event[event_field] for event in events if event["kind"] == "usage")
            assert result["openside"]["tokens"][field] == count > 0
            totals[field] += count
        for field in ("steps", "browserBytes"):
            totals[field] += result["openside"][field]
        if task.task_id == "login":
            opened = [event for event in events if event["kind"] == "ticket.opened"]
            assert len(opened) == 1 and opened[0]["ticket"]["kind"] == "info"
            assert any(event["kind"] == "ticket.resolved" for event in events)
            assert result["openside"]["ticketsResolved"] == 1
    for field in ("steps", "browserBytes"):
        assert summary["openside"][field] == totals[field]
    for field in ("input", "output"):
        assert summary["openside"]["tokens"][field] == totals[field]
    survivors = []
    for process in psutil.process_iter():
        try:
            if (process.pid, process.create_time()) in before or process.status() == psutil.STATUS_ZOMBIE:
                continue
            command = process.cmdline()
            if any("openside-bench-" in argument for argument in command):
                survivors.append(process.pid)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    assert not survivors, f"Benchmark daemon/browser processes survived: {survivors}"
